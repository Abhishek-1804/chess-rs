//! Endgame knowledge: an exact KPK bitbase (generated at compile time by
//! retrograde analysis in `build.rs`) and material-based draw scaling.

use crate::attacks::KPK_WIN;
use crate::position::Position;
use crate::types::*;

/// Index layout must match `kpk_index` in build.rs.
#[inline]
fn kpk_index(wk: Square, bk: Square, psq: Square, stm: usize) -> usize {
    let p = (rank_of(psq) as usize - 1) * 4 + file_of(psq) as usize;
    (((wk as usize * 64 + bk as usize) * 24 + p) * 2) + stm
}

/// Does the side with the pawn win K+P vs K with best play?
/// Squares are given from the strong side's point of view.
pub fn kpk_wins(strong: Color, strong_king: Square, weak_king: Square, pawn: Square, strong_to_move: bool) -> bool {
    // Normalise: strong side = white, pawn on files a-d.
    let flip_rank = |s: Square| if strong == Color::White { s } else { s ^ 56 };
    let (mut wk, mut bk, mut p) = (flip_rank(strong_king), flip_rank(weak_king), flip_rank(pawn));
    if file_of(p) >= 4 {
        wk ^= 7;
        bk ^= 7;
        p ^= 7;
    }
    let idx = kpk_index(wk, bk, p, usize::from(!strong_to_move));
    KPK_WIN[idx / 64] >> (idx % 64) & 1 != 0
}

/// Exact-knowledge evaluation for recognised endgames, from the side to
/// move's perspective. `None` if the material is not a recognised ending.
pub fn probe(pos: &Position) -> Option<i32> {
    if pos.occupied().count_ones() != 3 || pos.pieces_of_type(PieceType::Pawn).count_ones() != 1 {
        return None;
    }
    let psq = lsb(pos.pieces_of_type(PieceType::Pawn));
    let strong = pos.piece_on(psq)?.color();
    let stm = pos.side_to_move();
    let wins = kpk_wins(strong, pos.king_sq(strong), pos.king_sq(strong.flip()), psq, stm == strong);
    if !wins {
        return Some(0);
    }
    // Known win: big score, nudged by pawn progress so the search pushes it.
    let rel_rank = if strong == Color::White { rank_of(psq) } else { 7 - rank_of(psq) } as i32;
    let score = 900 + 30 * rel_rank;
    Some(if stm == strong { score } else { -score })
}

/// Scale factor (out of 64) for a white-relative score, for material
/// configurations where the side that is ahead can rarely or never win.
pub fn scale(pos: &Position, white_score: i32) -> i32 {
    use PieceType::*;
    let strong = if white_score >= 0 { Color::White } else { Color::Black };
    let weak = strong.flip();
    if rook_pawn_fortress(pos, strong) {
        return 0;
    }
    if pos.pieces(strong, Pawn) != 0 {
        return 64;
    }
    let value = |c: Color| {
        pos.pieces(c, Knight).count_ones() as i32 * 3
            + pos.pieces(c, Bishop).count_ones() as i32 * 3
            + pos.pieces(c, Rook).count_ones() as i32 * 5
            + pos.pieces(c, Queen).count_ones() as i32 * 9
    };
    let (s, w) = (value(strong), value(weak));
    // A lone minor (or nothing) cannot mate.
    if s <= 3 {
        return 0;
    }
    // Two knights cannot force mate against a bare king.
    if s == 6 && pos.pieces(strong, Knight).count_ones() == 2 && w == 0 && pos.pieces(weak, Pawn) == 0 {
        return 2;
    }
    // Pawnless with less than a rook's advantage: usually a draw (KR v KB, KR v KN, ...).
    if s - w < 4 {
        return 12;
    }
    64
}

/// Rook pawn(s) with no help, or only a bishop that cannot control the
/// queening square, against a king that already holds the corner: a
/// textbook dead draw, whatever else the defender has.
pub fn rook_pawn_fortress(pos: &Position, strong: Color) -> bool {
    use PieceType::*;
    let pawns = pos.pieces(strong, Pawn);
    if pawns == 0 {
        return false;
    }
    let file = if pawns & !FILE_A == 0 {
        0
    } else if pawns & !FILE_H == 0 {
        7
    } else {
        return false;
    };
    let others = pos.color_bb(strong) & !pawns & !pos.pieces(strong, King);
    if others & !pos.pieces(strong, Bishop) != 0 {
        return false; // knights, rooks or queens can break the fortress
    }
    let promo = if strong == Color::White { make_sq(file, 7) } else { make_sq(file, 0) };
    let promo_dark = bb(promo) & DARK_SQUARES != 0;
    let bishops = pos.pieces(strong, Bishop);
    let controls_corner = if promo_dark { bishops & DARK_SQUARES != 0 } else { bishops & !DARK_SQUARES != 0 };
    if controls_corner {
        return false;
    }
    let wk = pos.king_sq(strong.flip());
    let dist =
        (file_of(wk) as i32 - file_of(promo) as i32).abs().max((rank_of(wk) as i32 - rank_of(promo) as i32).abs());
    dist <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_fen(fen: &str) -> Option<i32> {
        probe(&Position::from_fen(fen).unwrap())
    }

    #[test]
    fn kpk_known_results() {
        // Opposition: Ke5+Pe4 v Ke7 is a draw with White to move...
        assert_eq!(probe_fen("8/4k3/8/4K3/4P3/8/8/8 w - - 0 1"), Some(0));
        // ...and a win with Black to move (White takes the opposition).
        assert!(probe_fen("8/4k3/8/4K3/4P3/8/8/8 b - - 0 1").unwrap() < -800);
        // King on the 6th in front of its pawn wins whoever moves.
        assert!(probe_fen("4k3/8/4K3/4P3/8/8/8/8 w - - 0 1").unwrap() > 800);
        assert!(probe_fen("4k3/8/4K3/4P3/8/8/8/8 b - - 0 1").unwrap() < -800);
        // Stalemate trick: Kf6+Pg6 v Kh8 is a draw.
        assert_eq!(probe_fen("7k/8/5KP1/8/8/8/8/8 w - - 0 1"), Some(0));
        // Rook pawn with the defender in the corner: draw.
        assert_eq!(probe_fen("7k/8/8/6KP/8/8/8/8 w - - 0 1"), Some(0));
        // Pawn far ahead of the defending king: rule of the square.
        assert!(probe_fen("8/8/8/1P6/8/8/8/K6k w - - 0 1").unwrap() > 800);
        // Mirror symmetry: black pawn versions agree.
        for fen in
            ["8/4k3/8/4K3/4P3/8/8/8 w - - 0 1", "8/4k3/8/4K3/4P3/8/8/8 b - - 0 1", "8/8/8/1P6/8/8/8/K6k w - - 0 1"]
        {
            let p = Position::from_fen(fen).unwrap();
            assert_eq!(probe(&p), probe(&p.mirrored()), "{fen}");
        }
    }

    /// Cross-check the bitbase against a brute-force search on simple cases:
    /// the classic "rule of the square" for an unsupported passed pawn.
    #[test]
    fn kpk_agrees_with_rule_of_the_square() {
        for (fen, win) in [
            ("8/8/8/8/P7/8/8/K5k1 w - - 0 1", true),  // king far outside the square
            ("8/8/8/5k2/P7/8/8/K7 w - - 0 1", true),  // one tempo short of the square
            ("8/8/8/5k2/P7/8/8/K7 b - - 0 1", false), // Black to move steps into it
            ("8/8/8/4k3/P7/8/8/K7 w - - 0 1", false), // ...Kd6-c7-b7 catches the pawn
        ] {
            assert_eq!(probe_fen(fen).unwrap() != 0, win, "{fen}");
        }
    }

    #[test]
    fn rook_pawn_fortresses() {
        let p = |f: &str| Position::from_fen(f).unwrap();
        // Wrong bishop (light) for h8 (dark), defender in the corner: draw.
        assert!(rook_pawn_fortress(&p("6k1/8/6BP/6K1/8/8/8/8 w - - 0 1"), Color::White));
        // Right bishop (dark): not a fortress.
        assert!(!rook_pawn_fortress(&p("6k1/8/5B1P/6K1/8/8/8/8 w - - 0 1"), Color::White));
        // Defender too far from the corner.
        assert!(!rook_pawn_fortress(&p("8/8/6BP/6K1/8/8/8/2k5 w - - 0 1"), Color::White));
        // Doubled h-pawns, king in the corner.
        assert!(rook_pawn_fortress(&p("6k1/8/6KP/7P/8/8/8/8 w - - 0 1"), Color::White));
        // Black version: a-pawn promoting on a1 (dark), light bishop.
        assert!(rook_pawn_fortress(&p("8/8/8/p7/k1b5/8/8/1K6 b - - 0 1"), Color::Black));
        // A knight can help: not a fortress.
        assert!(!rook_pawn_fortress(&p("6k1/8/6NP/6K1/8/8/8/8 w - - 0 1"), Color::White));
    }

    #[test]
    fn scaling() {
        let p = |f: &str| Position::from_fen(f).unwrap();
        assert_eq!(scale(&p("4k3/8/8/8/8/8/8/3BK3 w - - 0 1"), 300), 0);
        assert_eq!(scale(&p("4k3/8/8/8/8/8/8/2NNK3 w - - 0 1"), 300), 2);
        assert_eq!(scale(&p("4kb2/8/8/8/8/8/8/3RK3 w - - 0 1"), 200), 12);
        assert_eq!(scale(&p("4k3/8/8/8/8/8/8/3RK3 w - - 0 1"), 500), 64);
        assert_eq!(scale(&p("4k3/8/8/8/8/8/4P3/3RK3 w - - 0 1"), 500), 64);
    }
}
