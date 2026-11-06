//! chess-rs GUI: an egui front-end for `chess_engine`.

pub mod app;
pub mod board;
pub mod library;
pub mod pieces;
pub mod review_worker;
pub mod selfplay;
pub mod worker;

pub use app::{AppConfig, ChessApp, Mode, SidePick, Tab};

/// Default on-disk cache location: `<project>/cache/positions.bin`.
pub fn default_cache_path() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    // crates/gui -> project root
    let root = manifest.parent().and_then(|p| p.parent()).unwrap_or(manifest);
    root.join("cache").join("positions.bin")
}

/// Default folder for saved games: `<project>/games`.
pub fn default_games_dir() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent().and_then(|p| p.parent()).unwrap_or(manifest);
    root.join("games")
}
