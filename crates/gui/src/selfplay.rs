//! Headless bot-vs-bot games, saved to the games library (`chess-rs selfplay`).
//!
//! A deterministic engine would play the same game every time, so each game
//! starts with a few random (legal) opening moves before the engines take over.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chess_engine::pgn::{PgnGame, utc_now};
use chess_engine::{Engine, Game, Limits};

use crate::library::GameLibrary;

/// Safety cap; the fifty-move rule ends real games long before this.
const MAX_PLIES: usize = 600;

pub struct SelfPlay {
    pub games: usize,
    pub depth: u32,
    pub threads: usize,
    /// Random moves played at the start of each game for variety.
    pub random_plies: usize,
    pub seed: u64,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Play one game; returns it with PGN tags filled in.
pub fn play_one(engine: &Engine, depth: u32, random_plies: usize, rng_seed: u64) -> PgnGame {
    let mut rng = Rng(rng_seed);
    let mut game = Game::new();
    let stop = AtomicBool::new(false);
    engine.clear();
    while game.result().is_none() && game.ply_count() < MAX_PLIES {
        let legal = game.legal_moves();
        let m = if game.ply_count() < random_plies {
            legal[(rng.next() % legal.len() as u64) as usize]
        } else {
            let pos = *game.position();
            let r = engine.search(&pos, &game.history_hashes(), &Limits::depth(depth), &stop, |_| {});
            r.best_move.unwrap_or(legal[0])
        };
        game.play(m).expect("engine and random moves are legal");
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
        ("RandomOpeningPlies", random_plies.to_string()),
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
    let mut paths = Vec::with_capacity(cfg.games);
    for i in 0..cfg.games {
        let pgn = play_one(&engine, cfg.depth, cfg.random_plies, cfg.seed.wrapping_add(i as u64 * 7919));
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
        let cfg = SelfPlay { games: 3, depth: 2, threads: 1, random_plies: 4, seed: 42 };
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
        assert_eq!(move_lists.len(), 3, "random openings make the games differ");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
