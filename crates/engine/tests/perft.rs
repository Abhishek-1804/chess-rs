//! Move-generator validation against published perft counts.
//!
//! The six chessprogramming.org reference positions plus Martin Sedlak's
//! edge-case suite (en passant discovered checks, castling through check,
//! promotions out of check, stalemate traps, ...).
//!
//! Fast depths run by default; `cargo test -- --ignored` runs the deep ones.

use chess_engine::Position;
use chess_engine::movegen::{perft, perft_parallel};

struct Case {
    name: &'static str,
    fen: &'static str,
    /// Expected node counts for depth 1, 2, 3, ...
    counts: &'static [u64],
}

const CASES: &[Case] = &[
    Case {
        name: "start position",
        fen: "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
        counts: &[20, 400, 8_902, 197_281, 4_865_609, 119_060_324, 3_195_901_860],
    },
    Case {
        name: "kiwipete",
        fen: "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        counts: &[48, 2_039, 97_862, 4_085_603, 193_690_690, 8_031_647_685],
    },
    Case {
        name: "position 3 (en passant / rook endgame)",
        fen: "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        counts: &[14, 191, 2_812, 43_238, 674_624, 11_030_083, 178_633_661, 3_009_794_393],
    },
    Case {
        name: "position 4 (promotions, castling rights)",
        fen: "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
        counts: &[6, 264, 9_467, 422_333, 15_833_292, 706_045_033],
    },
    Case {
        name: "position 4 mirrored",
        fen: "r2q1rk1/pP1p2pp/Q4n2/bbp1p3/Np6/1B3NBn/pPPP1PPP/R3K2R b KQ - 0 1",
        counts: &[6, 264, 9_467, 422_333, 15_833_292, 706_045_033],
    },
    Case {
        name: "position 5",
        fen: "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
        counts: &[44, 1_486, 62_379, 2_103_487, 89_941_194],
    },
    Case {
        name: "position 6",
        fen: "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
        counts: &[46, 2_079, 89_890, 3_894_594, 164_075_551, 6_923_051_137],
    },
];

/// (name, fen, depth, nodes) — Martin Sedlak's edge-case positions.
const EDGE_CASES: &[(&str, &str, u32, u64)] = &[
    ("illegal ep move #1", "3k4/3p4/8/K1P4r/8/8/8/8 b - - 0 1", 6, 1_134_888),
    ("illegal ep move #2", "8/8/4k3/8/2p5/8/B2P2K1/8 w - - 0 1", 6, 1_015_133),
    ("ep capture checks opponent", "8/8/1k6/2b5/2pP4/8/5K2/8 b - d3 0 1", 6, 1_440_467),
    ("short castling gives check", "5k2/8/8/8/8/8/8/4K2R w K - 0 1", 6, 661_072),
    ("long castling gives check", "3k4/8/8/8/8/8/8/R3K3 w Q - 0 1", 6, 803_711),
    ("castle rights", "r3k2r/1b4bq/8/8/8/8/7B/R3K2R w KQkq - 0 1", 4, 1_274_206),
    ("castling prevented", "r3k2r/8/3Q4/8/8/5q2/8/R3K2R b KQkq - 0 1", 4, 1_720_476),
    ("promote out of check", "2K2r2/4P3/8/8/8/8/8/3k4 w - - 0 1", 6, 3_821_001),
    ("discovered check", "8/8/1P2K3/8/2n5/1q6/8/5k2 b - - 0 1", 5, 1_004_658),
    ("promote to give check", "4k3/1P6/8/8/8/8/K7/8 w - - 0 1", 6, 217_342),
    ("under-promote to give check", "8/P1k5/K7/8/8/8/8/8 w - - 0 1", 6, 92_683),
    ("self stalemate", "K1k5/8/P7/8/8/8/8/8 w - - 0 1", 6, 2_217),
    ("stalemate & checkmate #1", "8/k1P5/8/1K6/8/8/8/8 w - - 0 1", 7, 567_584),
    ("stalemate & checkmate #2", "8/8/2k5/5q2/5n2/8/5K2/8 b - - 0 1", 4, 23_527),
];

/// Node budget for the default (non-ignored) run.
const FAST_LIMIT: u64 = 200_000_000;

fn run(case: &Case, deep: bool) {
    let pos = Position::from_fen(case.fen).unwrap();
    for (i, &expected) in case.counts.iter().enumerate() {
        let depth = i as u32 + 1;
        if (expected > FAST_LIMIT) != deep {
            continue;
        }
        let got = if depth <= 3 { perft(&pos, depth) } else { perft_parallel(&pos, depth) };
        assert_eq!(got, expected, "{} at depth {depth}", case.name);
    }
}

#[test]
fn reference_positions() {
    for case in CASES {
        run(case, false);
    }
}

#[test]
#[ignore = "deep perft: billions of nodes; run with --ignored"]
fn reference_positions_deep() {
    for case in CASES {
        run(case, true);
    }
}

#[test]
fn edge_cases() {
    for &(name, fen, depth, expected) in EDGE_CASES {
        let pos = Position::from_fen(fen).unwrap();
        assert_eq!(perft_parallel(&pos, depth), expected, "{name}");
        // Colour-mirrored positions must produce identical trees.
        assert_eq!(perft_parallel(&pos.mirrored(), depth), expected, "{name} (mirrored)");
    }
}

#[test]
fn serial_and_parallel_perft_agree() {
    let pos = Position::from_fen(CASES[1].fen).unwrap();
    assert_eq!(perft(&pos, 4), perft_parallel(&pos, 4));
}
