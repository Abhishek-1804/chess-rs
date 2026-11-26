//! Headless bot-vs-bot games, saved to the games library (`chess-rs selfplay`).
//!
//! A deterministic engine would play the same game every time, so each game
//! follows a randomly chosen opening-book line before the engines take over.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chess_engine::pgn::{PgnGame, utc_now};
use chess_engine::{Engine, Game, Limits, book};

use crate::Rng;
use crate::library::GameLibrary;

/// Safety cap; the fifty-move rule ends real games long before this.
const MAX_PLIES: usize = 600;

pub struct SelfPlay {
    pub games: usize,
    pub depth: u32,
    pub threads: usize,
    /// Seed for choosing the openings.
    pub seed: u64,
}

/// Play one game; returns it with PGN tags filled in.
pub fn play_one(engine: &Engine, depth: u32, rng: &mut Rng) -> PgnGame {
    let mut game = Game::new();
    let stop = AtomicBool::new(false);
    engine.clear();
    while game.result().is_none() && game.ply_count() < MAX_PLIES {
        let m = match book::pick(&game, rng.next_u64()) {
            Some(m) => m,
            None => {
                let r = engine.search(game.position(), &game.history_hashes(), &Limits::depth(depth), &stop, |_| {});
                r.best_move.expect("the game is not over, so there is a legal move")
            }
        };
        game.play(m).expect("book and engine moves are legal");
    }
    let mut pgn = PgnGame::from_game(&game);
    let (date, time) = utc_now();
    let bot = format!("chess-rs (depth {depth})");
    for (k, v) in [
        ("Event", "chess-rs self-play".to_string()),
        ("Site", "chess-rs".into()),
        ("Date", date),
        ("Round", "-".into()),
        ("White", bot.clone()),
        ("Black", bot),
        ("Time", time),
        ("Mode", "Bot vs Bot".into()),
        ("BotDepth", depth.to_string()),
        ("PlyCount", game.ply_count().to_string()),
        ("Termination", game.result().map_or("move limit reached".into(), |r| r.describe())),
    ] {
        pgn.set_tag(k, v);
    }
    pgn
}

/// Play and save `cfg.games` games; `on_game(index, path, summary)` reports each.
pub fn generate(
    lib: &GameLibrary,
    cfg: &SelfPlay,
    mut on_game: impl FnMut(usize, &PathBuf, &str),
) -> io::Result<Vec<PathBuf>> {
    let engine = Engine::new(64, cfg.threads);
    let mut rng = Rng::new(cfg.seed);
    let mut paths = Vec::with_capacity(cfg.games);
    for i in 0..cfg.games {
        let pgn = play_one(&engine, cfg.depth, &mut rng);
        let path = lib.new_path(&pgn);
        lib.save(&path, &pgn)?;
        let summary = format!(
            "{} in {} moves ({})",
            pgn.result,
            pgn.moves.len().div_ceil(2),
            pgn.tag("Termination").unwrap_or("")
        );
        on_game(i, &path, &summary);
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_finished_distinct_games() {
        let dir = std::env::temp_dir().join(format!("chess-rs-selfplay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let lib = GameLibrary::new(&dir);
        let cfg = SelfPlay { games: 3, depth: 2, threads: 1, seed: 42 };
        let mut reported = 0;
        let paths = generate(&lib, &cfg, |_, _, _| reported += 1).unwrap();
        assert_eq!((paths.len(), reported), (3, 3));
        let games = lib.list();
        assert_eq!(games.len(), 3, "unique file per game even within the same second");
        let mut move_lists = Vec::new();
        for g in &games {
            let pgn = lib.load(&g.path).unwrap();
            let replay = pgn.to_game();
            assert_eq!(replay.ply_count(), pgn.moves.len(), "every move legal");
            assert!(replay.result().is_some() || pgn.moves.len() >= MAX_PLIES, "games are played to the end");
            assert_eq!(pgn.tag("Mode"), Some("Bot vs Bot"));
            move_lists.push(pgn.moves);
        }
        move_lists.dedup();
        assert_eq!(move_lists.len(), 3, "book openings make the games differ");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
