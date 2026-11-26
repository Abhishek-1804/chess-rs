//! chess-rs GUI: an egui front-end for `chess_engine`.

pub mod app;
pub mod board;
pub mod library;
pub mod online;
pub mod online_cli;
pub mod pieces;
pub mod review_worker;
pub mod selfplay;
pub mod worker;

pub use app::{AppConfig, ChessApp, Mode, SidePick, Tab};

/// The project folder (two levels above this crate).
fn project_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().and_then(|p| p.parent()).unwrap_or(manifest).to_path_buf()
}

/// Default on-disk cache location: `<project>/cache/positions.bin`.
pub fn default_cache_path() -> std::path::PathBuf {
    project_root().join("cache").join("positions.bin")
}

/// Default folder for saved games: `<project>/games`.
pub fn default_games_dir() -> std::path::PathBuf {
    project_root().join("games")
}

/// A small fast random number generator (splitmix64), for picking openings.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    /// Seeded from the clock.
    pub fn from_time() -> Self {
        Rng(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}
