//! A small opening book: main lines of common openings, about ten plies each.
//!
//! The engine is deterministic, so two bots searching to the same depth play
//! the same game every time. Following a randomly chosen book line for the
//! first moves gives every game a different opening.

use crate::game::Game;
use crate::notation::parse_move;
use crate::position::{Position, START_FEN};
use crate::types::Move;

/// Opening lines in SAN from the standard start position.
pub const LINES: &[&str] = &[
    // 1.e4 e5
    "e4 e5 Nf3 Nc6 Bb5 a6 Ba4 Nf6 O-O Be7",
    "e4 e5 Nf3 Nc6 Bb5 Nf6 O-O Nxe4 d4 Nd6",
    "e4 e5 Nf3 Nc6 Bb5 a6 Bxc6 dxc6 O-O f6",
    "e4 e5 Nf3 Nc6 Bc4 Bc5 c3 Nf6 d3 d6",
    "e4 e5 Nf3 Nc6 Bc4 Nf6 d3 Be7 O-O O-O",
    "e4 e5 Nf3 Nc6 Bc4 Bc5 b4 Bxb4 c3 Ba5",
    "e4 e5 Nf3 Nc6 d4 exd4 Nxd4 Nf6 Nxc6 bxc6",
    "e4 e5 Nf3 Nc6 Nc3 Nf6 d4 exd4 Nxd4 Bb4",
    "e4 e5 Nf3 Nf6 Nxe5 d6 Nf3 Nxe4 d4 d5",
    "e4 e5 Nf3 d6 d4 Nf6 Nc3 Nbd7 Bc4 Be7",
    "e4 e5 Nc3 Nf6 g3 d5 exd5 Nxd5 Bg2 Nxc3",
    "e4 e5 f4 exf4 Nf3 d6 d4 g5 h4 g4",
    // Sicilian
    "e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 a6",
    "e4 c5 Nf3 d6 d4 cxd4 Nxd4 Nf6 Nc3 g6",
    "e4 c5 Nf3 Nc6 d4 cxd4 Nxd4 Nf6 Nc3 e5",
    "e4 c5 Nf3 e6 d4 cxd4 Nxd4 Nc6 Nc3 Qc7",
    "e4 c5 Nf3 e6 d4 cxd4 Nxd4 a6 Bd3 Nf6",
    "e4 c5 Nf3 Nc6 Bb5 g6 O-O Bg7 Re1 e5",
    "e4 c5 Nf3 d6 Bb5+ Bd7 Bxd7+ Qxd7 O-O Nc6",
    "e4 c5 c3 Nf6 e5 Nd5 d4 cxd4 Nf3 Nc6",
    "e4 c5 Nc3 Nc6 g3 g6 Bg2 Bg7 d3 d6",
    // French
    "e4 e6 d4 d5 Nc3 Nf6 Bg5 Be7 e5 Nfd7",
    "e4 e6 d4 d5 Nc3 Bb4 e5 c5 a3 Bxc3+",
    "e4 e6 d4 d5 e5 c5 c3 Nc6 Nf3 Qb6",
    "e4 e6 d4 d5 Nd2 Nf6 e5 Nfd7 Bd3 c5",
    "e4 e6 d4 d5 exd5 exd5 Nf3 Nf6 Bd3 Bd6",
    // Caro-Kann
    "e4 c6 d4 d5 Nc3 dxe4 Nxe4 Bf5 Ng3 Bg6",
    "e4 c6 d4 d5 e5 Bf5 Nf3 e6 Be2 c5",
    "e4 c6 d4 d5 exd5 cxd5 Bd3 Nc6 c3 Nf6",
    // Other replies to 1.e4
    "e4 d6 d4 Nf6 Nc3 g6 f4 Bg7 Nf3 O-O",
    "e4 g6 d4 Bg7 Nc3 d6 Be3 a6 Qd2 Nd7",
    "e4 Nf6 e5 Nd5 d4 d6 Nf3 Bg4 Be2 e6",
    "e4 d5 exd5 Qxd5 Nc3 Qa5 d4 Nf6 Nf3 c6",
    // 1.d4 d5
    "d4 d5 c4 e6 Nc3 Nf6 Bg5 Be7 e3 O-O",
    "d4 Nf6 c4 e6 Nc3 d5 cxd5 exd5 Bg5 c6",
    "d4 d5 c4 c6 Nf3 Nf6 Nc3 dxc4 a4 Bf5",
    "d4 d5 c4 c6 Nf3 Nf6 Nc3 e6 e3 Nbd7",
    "d4 d5 c4 dxc4 Nf3 Nf6 e3 e6 Bxc4 c5",
    "d4 d5 Bf4 Nf6 e3 c5 c3 Nc6 Nd2 e6",
    "d4 d5 Nf3 Nf6 e3 e6 Bd3 c5 b3 Nc6",
    // 1.d4 Nf6
    "d4 Nf6 c4 g6 Nc3 Bg7 e4 d6 Nf3 O-O",
    "d4 Nf6 c4 g6 Nc3 d5 cxd5 Nxd5 e4 Nxc3",
    "d4 Nf6 c4 e6 Nc3 Bb4 e3 O-O Bd3 d5",
    "d4 Nf6 c4 e6 Nf3 b6 g3 Ba6 b3 Bb4+",
    "d4 Nf6 c4 e6 g3 d5 Bg2 Be7 Nf3 O-O",
    "d4 Nf6 c4 c5 d5 e6 Nc3 exd5 cxd5 d6",
    "d4 Nf6 c4 c5 d5 b5 cxb5 a6 bxa6 Bxa6",
    "d4 Nf6 Nf3 e6 Bg5 c5 e3 h6 Bh4 Be7",
    "d4 f5 g3 Nf6 Bg2 g6 Nf3 Bg7 O-O O-O",
    // Flank openings
    "c4 e5 Nc3 Nf6 g3 d5 cxd5 Nxd5 Bg2 Nb6",
    "c4 c5 Nf3 Nf6 Nc3 Nc6 g3 g6 Bg2 Bg7",
    "c4 Nf6 Nc3 e6 e4 d5 e5 d4 exf6 dxc3",
    "Nf3 d5 g3 Nf6 Bg2 e6 O-O Be7 d3 O-O",
    "Nf3 Nf6 c4 b6 g3 Bb7 Bg2 e6 O-O Be7",
    "g3 d5 Bg2 Nf6 Nf3 c5 O-O Nc6 d3 e5",
];

/// Book moves that continue `game`: the next move of every line the game has
/// followed so far. A move shared by several lines appears once per line, so
/// picking uniformly from the list favours the more common choices. Empty once
/// the game leaves the book, or if it didn't start from the standard position.
pub fn candidates(game: &Game) -> Vec<Move> {
    let start = Position::from_fen(START_FEN).expect("start position");
    if game.start_position().hash() != start.hash() {
        return Vec::new();
    }
    let played = game.moves();
    let mut out = Vec::new();
    'lines: for line in LINES {
        let mut pos = start;
        for (i, san) in line.split_whitespace().enumerate() {
            let Ok(m) = parse_move(&pos, san) else { continue 'lines };
            match played.get(i) {
                Some(&p) if p == m => pos = pos.make_move(m),
                Some(_) => continue 'lines,
                None => {
                    out.push(m);
                    continue 'lines;
                }
            }
        }
    }
    out
}

/// A book move for `game` chosen by `random` (any number, e.g. from an RNG),
/// or `None` once out of book.
pub fn pick(game: &Game, random: u64) -> Option<Move> {
    let moves = candidates(game);
    (!moves.is_empty()).then(|| moves[(random % moves.len() as u64) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_line_is_legal_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for line in LINES {
            let mut g = Game::new();
            for san in line.split_whitespace() {
                let m = parse_move(g.position(), san).unwrap_or_else(|e| panic!("{line}: {san}: {e}"));
                g.play(m).unwrap();
            }
            assert!(g.ply_count() >= 10, "{line}");
            assert!(seen.insert(g.position().hash()), "duplicate line {line}");
        }
    }

    #[test]
    fn candidates_follow_the_game() {
        let mut g = Game::new();
        let first: std::collections::HashSet<String> = candidates(&g).iter().map(|m| m.to_uci()).collect();
        for m in ["e2e4", "d2d4", "c2c4", "g1f3", "g2g3"] {
            assert!(first.contains(m), "{m}");
        }
        g.play(parse_move(g.position(), "e4").unwrap()).unwrap();
        let replies: std::collections::HashSet<String> = candidates(&g).iter().map(|m| m.to_uci()).collect();
        assert!(replies.contains("e7e5") && replies.contains("c7c5") && !replies.contains("d2d4"));
        // Off-book move: no candidates.
        g.play(parse_move(g.position(), "a6").unwrap()).unwrap();
        assert!(candidates(&g).is_empty());
        // Custom start positions have no book.
        let g = Game::from_fen("8/8/8/4k3/8/8/4P3/4K3 w - - 0 1").unwrap();
        assert!(candidates(&g).is_empty());
    }

    #[test]
    fn book_ends_after_the_line() {
        let mut g = Game::new();
        for san in LINES[0].split_whitespace() {
            g.play(parse_move(g.position(), san).unwrap()).unwrap();
        }
        assert!(candidates(&g).is_empty());
    }
}
