//! `chess_engine` — a fast, dependency-free chess engine.
//!
//! * Bitboards with compile-time generated magic tables (`build.rs`)
//! * Fully legal move generation (pin/check masks), copy-make positions
//! * Alpha-beta minimax (negamax/PVS) with a lock-free shared TT and Lazy SMP
//! * A persistent on-disk position cache shared across games

pub mod analysis;
pub mod attacks;
pub mod cache;
pub mod endgame;
pub mod eval;
pub mod game;
pub mod movegen;
pub mod notation;
pub mod pgn;
pub mod position;
pub mod psqt;
pub mod search;
pub mod see;
pub mod tt;
pub mod types;
pub mod zobrist;

pub use cache::{PositionCache, RootRecord};
pub use game::{Game, GameResult};
pub use movegen::{MoveList, legal_moves, perft};
pub use notation::{parse_move, to_san};
pub use position::{Position, START_FEN};
pub use search::{Engine, Limits, SearchInfo, SearchResult};
pub use types::{Color, Move, Piece, PieceType, Square};

/// Default number of search threads: every hardware thread.
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}
