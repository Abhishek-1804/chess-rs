//! Online play between two chess-rs apps, peer to peer over [iroh].
//!
//! # How a game code finds the other player
//!
//! There is no chess-rs server. The host draws a random 8-character code and
//! derives its iroh secret key from it (BLAKE3 key derivation), so its endpoint
//! id, which is the matching public key, is a pure function of the code. The
//! joiner derives the same id from the code and dials it. iroh's address lookup
//! (n0's free public DNS/pkarr service) resolves the id to the host's current
//! addresses and home relay. iroh then connects the two apps directly
//! (hole-punching through home routers), or through a public relay when that
//! fails. The code is the only thing the players exchange.
//!
//! # Wire protocol
//!
//! One bidirectional QUIC stream carrying UTF-8 lines:
//!
//! | Direction | Line | Meaning |
//! |---|---|---|
//! | joiner → host | `HELLO chess-rs/1` | protocol version check |
//! | host → joiner | `START <w\|b> <fen>` | the joiner's colour and the start position |
//! | host → joiner | `ERR <message>` | refused (wrong version, game already full) |
//! | either way | `MOVE <ply> <uci>` | a move; `ply` counts from 0 so both sides agree on the order |
//! | either way | `RESIGN`, `DRAW?`, `DRAW!`, `DRAW-NO` | resign, offer / accept / decline a draw |
//! | either way | `BYE` | the player left the game |
//!
//! The UI owns the game and validates every incoming move ([`apply_remote_move`]).
//!
//! [iroh]: https://www.iroh.computer

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chess_engine::{Color, Game, Move, parse_move};
use iroh::address_lookup::memory::MemoryLookup;
use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr};
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::sync::mpsc as tokio_mpsc;

/// QUIC application protocol id. Bump the version on incompatible changes.
pub const ALPN: &[u8] = b"chess-rs/online/1";
const HELLO: &str = "HELLO chess-rs/1";

/// Letters and digits that can't be confused with each other (no 0/O, 1/I/L).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;
const KEY_CONTEXT: &str = "chess-rs 2026 online game code v1";

/// How long a joiner keeps looking for the host before giving up.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(60);
/// Pause between attempts while the host's address isn't published yet.
const RETRY_EVERY: Duration = Duration::from_secs(2);
/// Longest line we accept from the peer.
const MAX_LINE: usize = 512;

// ---------------------------------------------------------------- game codes

/// A game code such as `K7QM-X2PA`. Stored without the dash, upper case.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GameCode(String);

impl GameCode {
    /// A fresh random code (about 40 bits, from the OS random generator).
    pub fn random() -> Self {
        let mut code = String::with_capacity(CODE_LEN);
        let n = CODE_ALPHABET.len();
        let limit = 256 - 256 % n; // reject biased bytes
        while code.len() < CODE_LEN {
            for b in SecretKey::generate().to_bytes() {
                if (b as usize) < limit && code.len() < CODE_LEN {
                    code.push(CODE_ALPHABET[b as usize % n] as char);
                }
            }
        }
        GameCode(code)
    }

    /// Parse what a player typed: case, spaces and dashes don't matter.
    pub fn parse(input: &str) -> Result<Self, String> {
        let code: String =
            input.chars().filter(|c| !c.is_whitespace() && *c != '-').map(|c| c.to_ascii_uppercase()).collect();
        if code.is_empty() {
            return Err("enter the game code your opponent gave you".into());
        }
        if let Some(bad) = code.chars().find(|c| !CODE_ALPHABET.contains(&(*c as u8)) || !c.is_ascii()) {
            return Err(format!("'{bad}' can't appear in a game code (codes never use 0, 1, I, L or O)"));
        }
        if code.chars().count() != CODE_LEN {
            return Err(format!("a game code has {CODE_LEN} characters, like K7QM-X2PA"));
        }
        Ok(GameCode(code))
    }

    /// The host's iroh secret key, derived from the code.
    fn secret_key(&self) -> SecretKey {
        SecretKey::from_bytes(&blake3::derive_key(KEY_CONTEXT, self.0.as_bytes()))
    }

    /// The host's endpoint id (public key), which the joiner dials.
    pub fn endpoint_id(&self) -> EndpointId {
        self.secret_key().public()
    }
}

impl std::fmt::Display for GameCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (a, b) = self.0.split_at(CODE_LEN / 2);
        write!(f, "{a}-{b}")
    }
}

impl std::fmt::Debug for GameCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GameCode({self})")
    }
}

// ---------------------------------------------------------------- API

/// Which network to play over.
#[derive(Clone, Debug, Default)]
pub enum Network {
    /// The internet, through n0's public relays and address lookup.
    #[default]
    Public,
    /// Loopback only, with addresses shared through an in-memory lookup.
    /// Lets tests run two players in one process without internet.
    Local(MemoryLookup),
}

impl Network {
    pub fn local() -> Self {
        Network::Local(MemoryLookup::new())
    }
}

/// What the UI asks the network thread to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetCmd {
    /// Send our move. `ply` is its index in the game (0 = first move).
    Move {
        ply: usize,
        uci: String,
    },
    Resign,
    OfferDraw,
    AcceptDraw,
    DeclineDraw,
    /// Leave the game (or stop waiting for an opponent).
    Leave,
}

/// What happened on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetEvent {
    /// Host: ready, waiting for the opponent to enter the code.
    Waiting,
    /// Host: the network isn't reachable yet (no relay after a while).
    /// Still waiting; it may just be slow.
    NoInternet,
    /// Joiner: looking for the host's game.
    Searching,
    /// The opponent is connected and the game begins.
    Started {
        my_color: Color,
        fen: String,
    },
    /// The opponent played a move (validate with [`apply_remote_move`]).
    Move {
        ply: usize,
        uci: String,
    },
    Resigned,
    DrawOffered,
    DrawAccepted,
    DrawDeclined,
    /// The opponent left the game.
    Left,
    /// The connection dropped.
    Lost(String),
    /// Couldn't start or join the game.
    Failed(String),
}

impl NetEvent {
    /// The session is over after this event.
    pub fn is_final(&self) -> bool {
        matches!(self, NetEvent::Left | NetEvent::Lost(_) | NetEvent::Failed(_))
    }
}

/// A running online session: hosting a game, or joining one. Networking runs
/// on its own thread, so nothing here blocks.
pub struct OnlineHandle {
    code: GameCode,
    tx: tokio_mpsc::UnboundedSender<NetCmd>,
    rx: mpsc::Receiver<NetEvent>,
    thread: Option<JoinHandle<()>>,
}

impl OnlineHandle {
    /// Host a new game under a fresh random code. `opponent` is the colour the
    /// joiner plays; `fen` is the start position.
    pub fn host(network: Network, opponent: Color, fen: String, wake: impl Fn() + Send + 'static) -> Self {
        let code = GameCode::random();
        let key = code.secret_key();
        Self::spawn(code, wake, move |events, cmds| host(network, key, opponent, fen, events, cmds))
    }

    /// Join the game with this code.
    pub fn join(network: Network, code: GameCode, wake: impl Fn() + Send + 'static) -> Self {
        let id = code.endpoint_id();
        Self::spawn(code, wake, move |events, cmds| join(network, id, events, cmds))
    }

    fn spawn<F, Fut>(code: GameCode, wake: impl Fn() + Send + 'static, run: F) -> Self
    where
        F: FnOnce(Events, tokio_mpsc::UnboundedReceiver<NetCmd>) -> Fut + Send + 'static,
        Fut: Future<Output = ()>,
    {
        let (tx, cmds) = tokio_mpsc::unbounded_channel();
        let (events_tx, rx) = mpsc::channel();
        let events = Events { tx: events_tx, wake: Box::new(wake) };
        let thread = std::thread::Builder::new()
            .name("chess-online".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => return events.send(NetEvent::Failed(format!("could not start networking: {e}"))),
                };
                rt.block_on(run(events, cmds));
            })
            .expect("spawn online thread");
        OnlineHandle { code, tx, rx, thread: Some(thread) }
    }

    pub fn code(&self) -> &GameCode {
        &self.code
    }

    pub fn send(&self, cmd: NetCmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn try_recv(&self) -> Option<NetEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, d: Duration) -> Option<NetEvent> {
        self.rx.recv_timeout(d).ok()
    }

    /// Leave the game and wait (briefly) for the goodbye to go out.
    pub fn shutdown(&mut self) {
        self.send(NetCmd::Leave);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for OnlineHandle {
    fn drop(&mut self) {
        // Don't block the UI: the thread says goodbye and exits on its own.
        self.send(NetCmd::Leave);
    }
}

/// Apply the opponent's move to `game`, checking that it arrived in order,
/// on their turn, and is legal.
pub fn apply_remote_move(game: &mut Game, my_color: Color, ply: usize, uci: &str) -> Result<Move, String> {
    if ply != game.ply_count() {
        return Err(format!("out-of-sync move {uci} (move {} received, expected {})", ply + 1, game.ply_count() + 1));
    }
    if game.side_to_move() == my_color {
        return Err(format!("opponent sent {uci} on your turn"));
    }
    let m = parse_move(game.position(), uci).map_err(|e| format!("opponent sent an illegal move {uci}: {e}"))?;
    game.play(m)?;
    Ok(m)
}

// ---------------------------------------------------------------- network thread

struct Events {
    tx: mpsc::Sender<NetEvent>,
    wake: Box<dyn Fn() + Send>,
}

impl Events {
    fn send(&self, e: NetEvent) {
        let _ = self.tx.send(e);
        (self.wake)();
    }
}

type Cmds = tokio_mpsc::UnboundedReceiver<NetCmd>;

async fn bind(network: &Network, key: SecretKey, accept: bool) -> Result<Endpoint, String> {
    let builder = match network {
        Network::Public => Endpoint::builder(presets::N0),
        Network::Local(lookup) => Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .address_lookup(lookup.clone())
            .bind_addr("127.0.0.1:0")
            .map_err(|e| e.to_string())?,
    };
    let builder = builder.secret_key(key);
    let builder = if accept { builder.alpns(vec![ALPN.to_vec()]) } else { builder };
    let ep = builder.bind().await.map_err(|e| format!("could not start networking: {e}"))?;
    if let Network::Local(lookup) = network
        && accept
    {
        let addrs = ep.bound_sockets().into_iter().map(TransportAddr::Ip).collect();
        lookup.add_endpoint_info(EndpointAddr { id: ep.id(), addrs });
    }
    Ok(ep)
}

async fn host(network: Network, key: SecretKey, opponent: Color, fen: String, events: Events, mut cmds: Cmds) {
    let ep = match bind(&network, key, true).await {
        Ok(ep) => ep,
        Err(e) => return events.send(NetEvent::Failed(e)),
    };
    events.send(NetEvent::Waiting);
    if matches!(network, Network::Public) {
        // Warn (but keep waiting) if no relay is reachable: likely offline.
        let online = ep.online();
        tokio::pin!(online);
        tokio::select! {
            _ = &mut online => {}
            _ = tokio::time::sleep(Duration::from_secs(15)) => events.send(NetEvent::NoInternet),
        }
    }
    let colour = if opponent == Color::White { "w" } else { "b" };
    let session = loop {
        tokio::select! {
            incoming = ep.accept() => {
                let Some(incoming) = incoming else {
                    events.send(NetEvent::Failed("network stopped".into()));
                    return;
                };
                // Handshake errors (port scanners, a wrong ALPN) just mean: keep waiting.
                let Ok(conn) = incoming.await else { continue };
                match tokio::time::timeout(Duration::from_secs(15), greet_joiner(&conn, colour, &fen)).await {
                    Ok(Ok(session)) => break session,
                    _ => conn.close(1u32.into(), b"handshake failed"),
                }
            }
            cmd = cmds.recv() => {
                if matches!(cmd, None | Some(NetCmd::Leave)) {
                    ep.close().await;
                    return;
                }
            }
        }
    };
    events.send(NetEvent::Started { my_color: opponent.flip(), fen });
    // Turn away anyone else who tries the code while the game is on.
    let busy = ep.clone();
    let turn_away = tokio::spawn(async move {
        while let Some(incoming) = busy.accept().await {
            if let Ok(conn) = incoming.await
                && let Ok((mut send, _recv)) = conn.accept_bi().await
            {
                let _ = send.write_all(b"ERR that game has already started\n").await;
                let _ = send.finish();
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    });
    play(session, &events, &mut cmds).await;
    turn_away.abort();
    close_endpoint(&ep).await;
}

async fn greet_joiner(conn: &Connection, colour: &str, fen: &str) -> Result<Session, String> {
    let (mut send, recv) = conn.accept_bi().await.map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(recv).lines();
    let hello = read_line(&mut lines).await?.ok_or("closed")?;
    if hello != HELLO {
        let _ = send.write_all(b"ERR your chess-rs version is not compatible with the host's; update both\n").await;
        let _ = send.finish();
        return Err(format!("unexpected greeting {hello:?}"));
    }
    send.write_all(format!("START {colour} {fen}\n").as_bytes()).await.map_err(|e| e.to_string())?;
    Ok(Session { conn: conn.clone(), send, lines })
}

async fn join(network: Network, host_id: EndpointId, events: Events, mut cmds: Cmds) {
    let ep = match bind(&network, SecretKey::generate(), false).await {
        Ok(ep) => ep,
        Err(e) => return events.send(NetEvent::Failed(e)),
    };
    events.send(NetEvent::Searching);
    let started = Instant::now();
    let mut last_error: String;
    let conn = loop {
        tokio::select! {
            res = tokio::time::timeout(Duration::from_secs(20), ep.connect(host_id, ALPN)) => match res {
                Ok(Ok(conn)) => break conn,
                Ok(Err(e)) => last_error = e.to_string(),
                Err(_) => last_error = "timed out".into(),
            },
            cmd = cmds.recv() => {
                if matches!(cmd, None | Some(NetCmd::Leave)) {
                    ep.close().await;
                    return;
                }
                continue;
            }
        }
        if started.elapsed() >= JOIN_TIMEOUT {
            events.send(NetEvent::Failed(format!(
                "no game found for that code. Check the code, and that your opponent is still waiting ({last_error})"
            )));
            ep.close().await;
            return;
        }
        // The host's address may not be published yet; try again shortly.
        tokio::select! {
            _ = tokio::time::sleep(RETRY_EVERY) => {}
            cmd = cmds.recv() => {
                if matches!(cmd, None | Some(NetCmd::Leave)) {
                    ep.close().await;
                    return;
                }
            }
        }
    };
    let start = async {
        let (mut send, recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
        send.write_all(format!("{HELLO}\n").as_bytes()).await.map_err(|e| e.to_string())?;
        let mut lines = BufReader::new(recv).lines();
        let line = read_line(&mut lines).await?.ok_or("the host closed the connection")?;
        match line.split_once(' ') {
            Some(("START", rest)) => {
                let (colour, fen) = rest.split_once(' ').ok_or("bad START line")?;
                let my_color = match colour {
                    "w" => Color::White,
                    "b" => Color::Black,
                    _ => return Err(format!("bad colour {colour:?}")),
                };
                Game::from_fen(fen).map_err(|e| format!("host sent a bad position: {e}"))?;
                Ok((Session { conn: conn.clone(), send, lines }, my_color, fen.to_string()))
            }
            Some(("ERR", msg)) => Err(msg.to_string()),
            _ => Err(format!("unexpected reply {line:?}")),
        }
    };
    match tokio::time::timeout(Duration::from_secs(15), start).await {
        Ok(Ok((session, my_color, fen))) => {
            events.send(NetEvent::Started { my_color, fen });
            play(session, &events, &mut cmds).await;
        }
        Ok(Err(e)) => events.send(NetEvent::Failed(format!("could not join: {e}"))),
        Err(_) => events.send(NetEvent::Failed("could not join: the host did not answer".into())),
    }
    close_endpoint(&ep).await;
}

struct Session {
    conn: Connection,
    send: SendStream,
    lines: Lines<BufReader<RecvStream>>,
}

async fn read_line(lines: &mut Lines<BufReader<RecvStream>>) -> Result<Option<String>, String> {
    match lines.next_line().await {
        Ok(Some(l)) if l.len() > MAX_LINE => Err("line too long".into()),
        Ok(l) => Ok(l.map(|l| l.trim_end().to_string())),
        Err(e) => Err(e.to_string()),
    }
}

/// Relay moves both ways until someone leaves or the connection drops.
async fn play(mut s: Session, events: &Events, cmds: &mut Cmds) {
    loop {
        tokio::select! {
            line = read_line(&mut s.lines) => {
                let event = match line {
                    Ok(Some(l)) => match parse_line(&l) {
                        Some(e) => e,
                        None => continue, // unknown line from a newer version: ignore
                    },
                    Ok(None) => NetEvent::Lost("the opponent's connection closed".into()),
                    Err(e) => NetEvent::Lost(format!("connection lost: {e}")),
                };
                let done = event.is_final();
                events.send(event);
                if done {
                    s.conn.close(0u32.into(), b"bye");
                    return;
                }
            }
            cmd = cmds.recv() => {
                let cmd = cmd.unwrap_or(NetCmd::Leave);
                let line = match &cmd {
                    NetCmd::Move { ply, uci } => format!("MOVE {ply} {uci}"),
                    NetCmd::Resign => "RESIGN".into(),
                    NetCmd::OfferDraw => "DRAW?".into(),
                    NetCmd::AcceptDraw => "DRAW!".into(),
                    NetCmd::DeclineDraw => "DRAW-NO".into(),
                    NetCmd::Leave => "BYE".into(),
                };
                if let Err(e) = s.send.write_all(format!("{line}\n").as_bytes()).await {
                    if cmd != NetCmd::Leave {
                        events.send(NetEvent::Lost(format!("connection lost: {e}")));
                    }
                    return;
                }
                if cmd == NetCmd::Leave {
                    let _ = s.send.finish();
                    // Give the goodbye a moment to arrive before closing.
                    let _ = tokio::time::timeout(Duration::from_secs(2), s.send.stopped()).await;
                    s.conn.close(0u32.into(), b"bye");
                    return;
                }
            }
        }
    }
}

fn parse_line(line: &str) -> Option<NetEvent> {
    let mut parts = line.split_whitespace();
    Some(match parts.next()? {
        "MOVE" => {
            let ply = parts.next()?.parse().ok()?;
            let uci = parts.next()?.to_string();
            NetEvent::Move { ply, uci }
        }
        "RESIGN" => NetEvent::Resigned,
        "DRAW?" => NetEvent::DrawOffered,
        "DRAW!" => NetEvent::DrawAccepted,
        "DRAW-NO" => NetEvent::DrawDeclined,
        "BYE" => NetEvent::Left,
        _ => return None,
    })
}

async fn close_endpoint(ep: &Endpoint) {
    let _ = tokio::time::timeout(Duration::from_secs(3), ep.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_normalise() {
        for _ in 0..200 {
            let c = GameCode::random();
            let shown = c.to_string();
            assert_eq!(shown.len(), CODE_LEN + 1);
            assert_eq!(GameCode::parse(&shown).unwrap(), c);
            assert_eq!(GameCode::parse(&shown.to_lowercase().replace('-', " ")).unwrap(), c);
        }
        assert_ne!(GameCode::random(), GameCode::random());
    }

    #[test]
    fn bad_codes_are_explained() {
        assert!(GameCode::parse("").unwrap_err().contains("enter"));
        assert!(GameCode::parse("ABCD-EFG").unwrap_err().contains("8 characters"));
        assert!(GameCode::parse("ABCD-EFG0").unwrap_err().contains("'0'"));
        assert!(GameCode::parse("ABCD-EFGO").unwrap_err().contains("'O'"));
    }

    #[test]
    fn same_code_same_host_id() {
        let c = GameCode::parse("K7QM-X2PA").unwrap();
        assert_eq!(c.endpoint_id(), GameCode::parse("k7qmx2pa").unwrap().endpoint_id());
        assert_ne!(c.endpoint_id(), GameCode::parse("K7QM-X2PB").unwrap().endpoint_id());
    }

    #[test]
    fn protocol_lines() {
        assert_eq!(parse_line("MOVE 12 e7e8q"), Some(NetEvent::Move { ply: 12, uci: "e7e8q".into() }));
        assert_eq!(parse_line("DRAW?"), Some(NetEvent::DrawOffered));
        assert_eq!(parse_line("BYE"), Some(NetEvent::Left));
        assert_eq!(parse_line("MOVE x e2e4"), None);
        assert_eq!(parse_line("FUTURE-THING 1"), None);
    }

    #[test]
    fn remote_moves_are_checked() {
        let mut g = Game::new();
        // We are Black: White's e4 at ply 0 is fine.
        apply_remote_move(&mut g, Color::Black, 0, "e2e4").unwrap();
        // Wrong ply, our turn, illegal.
        assert!(apply_remote_move(&mut g, Color::Black, 5, "e7e5").unwrap_err().contains("out-of-sync"));
        assert!(apply_remote_move(&mut g, Color::Black, 1, "e7e5").unwrap_err().contains("your turn"));
        g.play(parse_move(g.position(), "e5").unwrap()).unwrap();
        assert!(apply_remote_move(&mut g, Color::Black, 2, "e1e3").unwrap_err().contains("illegal"));
    }
}
