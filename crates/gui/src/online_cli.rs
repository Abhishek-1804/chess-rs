//! `chess-rs online`: play an online game from the terminal.
//!
//! Moves typed before it's your turn are queued (premoves), so a game can also
//! be scripted by piping moves into stdin. When stdin closes and no queued
//! moves remain, it's your turn and the game is still on, the player leaves.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

use chess_engine::types::make_sq;
use chess_engine::{Color, Game, START_FEN, parse_move};

use crate::SidePick;
use crate::online::{GameCode, NetCmd, NetEvent, Network, OnlineHandle, apply_remote_move};

pub const USAGE: &str = "\
chess-rs online — play someone over the internet, peer to peer

USAGE:
  chess-rs online host [--color white|black|random] [--fen <FEN>]
  chess-rs online join <CODE>

  host   Start a game and print a code (like K7QM-X2PA) to give your opponent.
         --color is your side [default: random]; --fen a custom start position.
  join   Join the game with the code your opponent gave you.

The opponent can use this command or the app (Mode: Online in the Play tab).
No server or account is needed. The apps connect directly, or through a free
public relay when a direct connection isn't possible.

During the game, type a move (e4, Nf3, O-O, e7e8q, g1f3) or a command:
  draw      offer a draw (or accept the one on offer)
  decline   decline a draw offer
  resign    resign the game
  board     show the board again
  quit      leave the game
";

enum Input {
    Line(String),
    Closed,
}

fn stdin_lines() -> mpsc::Receiver<Input> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) => {
                    if tx.send(Input::Line(l)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    eprintln!("error reading input: {e}");
                    break;
                }
            }
        }
        let _ = tx.send(Input::Closed);
    });
    rx
}

/// The board as text, from `side`'s point of view.
pub fn board_text(game: &Game, side: Color) -> String {
    let pos = game.position();
    let mut out = String::new();
    let ranks: Vec<u8> = if side == Color::White { (0..8).rev().collect() } else { (0..8).collect() };
    let files: Vec<u8> = if side == Color::White { (0..8).collect() } else { (0..8).rev().collect() };
    for &r in &ranks {
        out.push_str(&format!("  {} ", r + 1));
        for &f in &files {
            out.push(pos.piece_on(make_sq(f, r)).map_or('.', |p| p.to_char()));
            out.push(' ');
        }
        out.push('\n');
    }
    out.push_str("    ");
    for &f in &files {
        out.push((b'a' + f) as char);
        out.push(' ');
    }
    out.push('\n');
    out
}

struct Cli {
    net: OnlineHandle,
    game: Option<Game>,
    me: Color,
    queue: VecDeque<String>,
    input_closed: bool,
    they_offer_draw: bool,
    i_offer_draw: bool,
}

/// Why the session ended.
enum End {
    Finished,
    Quit,
}

fn say(msg: impl AsRef<str>) {
    println!("{}", msg.as_ref());
    let _ = std::io::stdout().flush();
}

pub fn run(args: &[String]) -> Result<(), String> {
    run_on(Network::Public, args)
}

/// Like [`run`], on a chosen network.
pub fn run_on(network: Network, args: &[String]) -> Result<(), String> {
    let mut it = args.iter();
    let net = match it.next().map(String::as_str) {
        Some("host") => {
            let mut side = SidePick::Random;
            let mut fen = START_FEN.to_string();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "-c" | "--color" => side = SidePick::parse(it.next().ok_or("--color needs a value")?)?,
                    "--fen" => {
                        fen = it.next().ok_or("--fen needs a value")?.clone();
                        Game::from_fen(&fen).map_err(|e| format!("invalid --fen: {e}"))?;
                    }
                    "-h" | "--help" => {
                        print!("{USAGE}");
                        return Ok(());
                    }
                    other => return Err(format!("unknown argument '{other}'")),
                }
            }
            let me = crate::app::resolve_side(side);
            let net = OnlineHandle::host(network, me.flip(), fen, || {});
            say(format!("Hosting a game (you play {}). Your game code is:\n", me.name()));
            say(format!("    {}\n", net.code()));
            say("Give it to your opponent. They enter it in the chess-rs app (Mode: Online, Join),");
            say(format!("or run:  chess-rs online join {}", net.code()));
            net
        }
        Some("join") => {
            let code = it.next().ok_or("join needs the game code, e.g. chess-rs online join K7QM-X2PA")?;
            if code == "-h" || code == "--help" {
                print!("{USAGE}");
                return Ok(());
            }
            let code = GameCode::parse(code)?;
            if let Some(extra) = it.next() {
                return Err(format!("unknown argument '{extra}'"));
            }
            OnlineHandle::join(network, code, || {})
        }
        Some("-h" | "--help") | None => {
            print!("{USAGE}");
            return Ok(());
        }
        Some(other) => return Err(format!("unknown online command '{other}' (use host or join)")),
    };
    let mut cli = Cli {
        net,
        game: None,
        me: Color::White,
        queue: VecDeque::new(),
        input_closed: false,
        they_offer_draw: false,
        i_offer_draw: false,
    };
    let input = stdin_lines();
    let result = cli.main_loop(&input);
    cli.net.shutdown();
    result.map(|_| ())
}

impl Cli {
    fn main_loop(&mut self, input: &mpsc::Receiver<Input>) -> Result<End, String> {
        loop {
            while let Some(ev) = self.net.try_recv() {
                if let Some(end) = self.on_event(ev)? {
                    return Ok(end);
                }
            }
            while let Ok(i) = input.try_recv() {
                match i {
                    Input::Line(l) => {
                        if let Some(end) = self.on_line(l.trim()) {
                            return Ok(end);
                        }
                    }
                    Input::Closed => self.input_closed = true,
                }
            }
            if let Some(end) = self.play_queued() {
                return Ok(end);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn my_turn(&self) -> bool {
        self.game.as_ref().is_some_and(|g| g.result().is_none() && g.side_to_move() == self.me)
    }

    fn prompt(&self) {
        if self.my_turn() && self.queue.is_empty() {
            print!("Your move ({}): ", self.me.name());
            let _ = std::io::stdout().flush();
        }
    }

    fn show(&self) {
        if let Some(g) = &self.game {
            say(format!("\n{}", board_text(g, self.me)));
        }
    }

    /// Print the result if the game just ended.
    fn check_over(&self) -> Option<End> {
        let g = self.game.as_ref()?;
        let r = g.result()?;
        let outcome = match r.winner() {
            Some(w) if w == self.me => "You win!",
            Some(_) => "You lose.",
            None => "It's a draw.",
        };
        say(format!("\n{} — {outcome}", r.describe()));
        let (w, b) = if self.me == Color::White { ("You", "Opponent") } else { ("Opponent", "You") };
        say(format!("\n{}", g.to_pgn(w, b)));
        Some(End::Finished)
    }

    fn on_event(&mut self, ev: NetEvent) -> Result<Option<End>, String> {
        match ev {
            NetEvent::Waiting => say("Waiting for your opponent to join… (Ctrl+C to cancel)"),
            NetEvent::NoInternet => say("Still no connection to the internet relays; check your network. Waiting…"),
            NetEvent::Searching => say("Looking for the game…"),
            NetEvent::Started { my_color, fen } => {
                self.me = my_color;
                self.game = Some(Game::from_fen(&fen)?);
                say(format!("\nConnected! You play {}.", my_color.name()));
                self.show();
                self.prompt();
            }
            NetEvent::Move { ply, uci } => {
                let game = self.game.as_mut().ok_or("move before the game started")?;
                let san = {
                    let before = *game.position();
                    let m = apply_remote_move(game, self.me, ply, &uci)?;
                    chess_engine::to_san(&before, m)
                };
                self.they_offer_draw = false;
                self.i_offer_draw = false;
                say(format!("\nOpponent played {san}"));
                self.show();
                if let Some(end) = self.check_over() {
                    return Ok(Some(end));
                }
                self.prompt();
            }
            NetEvent::Resigned => {
                if let Some(g) = &mut self.game {
                    g.resign(self.me.flip());
                }
                say("\nYour opponent resigned.");
                return Ok(self.check_over());
            }
            NetEvent::DrawOffered => {
                self.they_offer_draw = true;
                say("\nYour opponent offers a draw. Type 'draw' to accept or 'decline'.");
                self.prompt();
            }
            NetEvent::DrawAccepted if self.i_offer_draw => {
                if let Some(g) = &mut self.game {
                    g.agree_draw();
                }
                return Ok(self.check_over());
            }
            NetEvent::DrawAccepted => {}
            NetEvent::DrawDeclined => {
                self.i_offer_draw = false;
                say("\nYour opponent declined the draw.");
                self.prompt();
            }
            NetEvent::Left => {
                say("\nYour opponent left the game.");
                return Ok(Some(End::Quit));
            }
            NetEvent::Lost(e) => {
                say(format!("\nConnection lost: {e}"));
                return Ok(Some(End::Quit));
            }
            NetEvent::Failed(e) => return Err(e),
        }
        Ok(None)
    }

    fn on_line(&mut self, line: &str) -> Option<End> {
        match line.to_lowercase().as_str() {
            "" => {}
            "quit" | "exit" | "leave" => {
                say("Leaving the game.");
                return Some(End::Quit);
            }
            "help" | "?" => say(USAGE),
            "board" => self.show(),
            "resign" | "draw" | "decline" if self.game.as_ref().is_none_or(|g| g.result().is_some()) => {
                say("The game hasn't started yet.");
            }
            "resign" => {
                self.net.send(NetCmd::Resign);
                if let Some(g) = &mut self.game {
                    g.resign(self.me);
                }
                return self.check_over();
            }
            "draw" if self.they_offer_draw => {
                self.net.send(NetCmd::AcceptDraw);
                if let Some(g) = &mut self.game {
                    g.agree_draw();
                }
                return self.check_over();
            }
            "draw" => {
                if !self.i_offer_draw {
                    self.i_offer_draw = true;
                    self.net.send(NetCmd::OfferDraw);
                    say("Draw offered.");
                }
            }
            "decline" if self.they_offer_draw => {
                self.they_offer_draw = false;
                self.net.send(NetCmd::DeclineDraw);
                say("Draw declined.");
                self.prompt();
            }
            "decline" => say("There's no draw offer to decline."),
            _ => self.queue.push_back(line.to_string()),
        }
        None
    }

    /// Play the next queued move if it's our turn.
    fn play_queued(&mut self) -> Option<End> {
        if !self.my_turn() {
            return None;
        }
        let Some(text) = self.queue.pop_front() else {
            if self.input_closed {
                say("\nInput closed: leaving the game.");
                return Some(End::Quit);
            }
            return None;
        };
        let game = self.game.as_mut()?;
        match parse_move(game.position(), &text) {
            Ok(m) => {
                let ply = game.ply_count();
                let san = chess_engine::to_san(game.position(), m);
                game.play(m).ok()?;
                self.net.send(NetCmd::Move { ply, uci: m.to_uci() });
                // Moving declines an offer on the table.
                if self.they_offer_draw {
                    self.they_offer_draw = false;
                    self.net.send(NetCmd::DeclineDraw);
                }
                say(format!("You played {san}"));
                self.show();
                if let Some(end) = self.check_over() {
                    return Some(end);
                }
                say("Waiting for your opponent's move…");
            }
            Err(e) => {
                say(format!("'{text}': {e}"));
                if !self.queue.is_empty() {
                    say("(cleared your other queued moves)");
                    self.queue.clear();
                }
                self.prompt();
            }
        }
        None
    }
}
