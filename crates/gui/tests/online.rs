//! Online play: two players in one process over loopback (no internet), using
//! the real iroh transport and protocol. `public_internet_game` (ignored by
//! default; `mise run test:online`) does the same through n0's public relays and
//! address lookup, exactly as two players on different networks would.

use std::time::Duration;

use chess_engine::{Color, Game, GameResult, START_FEN, parse_move};
use chess_rs::online::{GameCode, NetCmd, NetEvent, Network, OnlineHandle, apply_remote_move};

const WAIT: Duration = Duration::from_secs(30);

fn next(h: &OnlineHandle) -> NetEvent {
    h.recv_timeout(WAIT).expect("no network event within 30s")
}

/// Host and join, returning both handles once the game has started.
fn connect(network: Network, joiner: Color) -> (OnlineHandle, OnlineHandle) {
    let host = OnlineHandle::host(network.clone(), joiner, START_FEN.into(), || {});
    if matches!(network, Network::Public) {
        assert_eq!(next(&host), NetEvent::Waiting);
    }
    let code = GameCode::parse(&host.code().to_string().to_lowercase()).unwrap();
    let guest = OnlineHandle::join(network, code, || {});
    let mut host_started = None;
    let mut guest_started = None;
    while host_started.is_none() || guest_started.is_none() {
        if host_started.is_none()
            && let Some(e) = host.recv_timeout(Duration::from_millis(50))
        {
            match e {
                NetEvent::Started { my_color, fen } => host_started = Some((my_color, fen)),
                NetEvent::Waiting | NetEvent::NoInternet => {}
                other => panic!("host: unexpected {other:?}"),
            }
        }
        if guest_started.is_none()
            && let Some(e) = guest.recv_timeout(Duration::from_millis(50))
        {
            match e {
                NetEvent::Started { my_color, fen } => guest_started = Some((my_color, fen)),
                NetEvent::Searching => {}
                other => panic!("guest: unexpected {other:?}"),
            }
        }
    }
    assert_eq!(host_started.unwrap(), (joiner.flip(), START_FEN.to_string()));
    assert_eq!(guest_started.unwrap(), (joiner, START_FEN.to_string()));
    (host, guest)
}

/// Play `moves` (SAN), alternating from White, between the two players, each
/// keeping its own board and validating the other's moves.
fn play_game(white: &OnlineHandle, black: &OnlineHandle, moves: &[&str]) -> (Game, Game) {
    let (mut wg, mut bg) = (Game::new(), Game::new());
    for (ply, san) in moves.iter().enumerate() {
        let (mover, mover_game, other, other_game, other_color) = if ply % 2 == 0 {
            (white, &mut wg, black, &mut bg, Color::Black)
        } else {
            (black, &mut bg, white, &mut wg, Color::White)
        };
        let m = parse_move(mover_game.position(), san).unwrap();
        mover_game.play(m).unwrap();
        mover.send(NetCmd::Move { ply, uci: m.to_uci() });
        match next(other) {
            NetEvent::Move { ply: p, uci } => {
                let got = apply_remote_move(other_game, other_color, p, &uci).unwrap();
                assert_eq!(got, m);
            }
            e => panic!("expected a move, got {e:?}"),
        }
    }
    (wg, bg)
}

const SCHOLARS_MATE: [&str; 7] = ["e4", "e5", "Bc4", "Nc6", "Qh5", "Nf6", "Qxf7#"];

#[test]
fn local_game_to_checkmate() {
    // The joiner asked to be... whatever the host chose: here, Black.
    let (host, guest) = connect(Network::local(), Color::Black);
    let (wg, bg) = play_game(&host, &guest, &SCHOLARS_MATE);
    assert_eq!(wg.result(), Some(GameResult::Checkmate { winner: Color::White }));
    assert_eq!(bg.result(), wg.result());
    assert_eq!(wg.to_pgn("a", "b"), bg.to_pgn("a", "b"));
}

#[test]
fn local_host_can_play_black() {
    let (host, guest) = connect(Network::local(), Color::White);
    let (wg, _) = play_game(&guest, &host, &["d4", "d5", "c4"]);
    assert_eq!(wg.ply_count(), 3);
}

#[test]
fn resign_draw_and_leave_reach_the_opponent() {
    let (host, guest) = connect(Network::local(), Color::Black);
    guest.send(NetCmd::OfferDraw);
    assert_eq!(next(&host), NetEvent::DrawOffered);
    host.send(NetCmd::DeclineDraw);
    assert_eq!(next(&guest), NetEvent::DrawDeclined);
    host.send(NetCmd::OfferDraw);
    assert_eq!(next(&guest), NetEvent::DrawOffered);
    guest.send(NetCmd::AcceptDraw);
    assert_eq!(next(&host), NetEvent::DrawAccepted);
    host.send(NetCmd::Resign);
    assert_eq!(next(&guest), NetEvent::Resigned);
    let mut host = host;
    host.shutdown();
    assert_eq!(next(&guest), NetEvent::Left);
}

#[test]
fn dropping_the_app_counts_as_leaving() {
    let (host, guest) = connect(Network::local(), Color::Black);
    drop(guest);
    assert_eq!(next(&host), NetEvent::Left);
}

#[test]
fn a_third_player_is_turned_away() {
    let network = Network::local();
    let (host, _guest) = connect(network.clone(), Color::Black);
    let intruder = OnlineHandle::join(network, host.code().clone(), || {});
    assert_eq!(next(&intruder), NetEvent::Searching);
    match next(&intruder) {
        NetEvent::Failed(msg) => assert!(msg.contains("already started"), "{msg}"),
        e => panic!("expected failure, got {e:?}"),
    }
}

#[test]
fn wrong_code_fails_cleanly_and_can_be_cancelled() {
    let mut guest = OnlineHandle::join(Network::local(), GameCode::random(), || {});
    assert_eq!(next(&guest), NetEvent::Searching);
    // Nobody is hosting this code; leaving while searching must not hang.
    let t = std::time::Instant::now();
    guest.shutdown();
    assert!(t.elapsed() < Duration::from_secs(5));
}

/// Over the real internet (n0's relays and address lookup). Needs network
/// access, so it's not part of the default suite: `mise run test:online`.
#[test]
#[ignore]
fn public_internet_game() {
    let (host, guest) = connect(Network::Public, Color::Black);
    let (wg, _) = play_game(&host, &guest, &SCHOLARS_MATE);
    assert_eq!(wg.result(), Some(GameResult::Checkmate { winner: Color::White }));
}
