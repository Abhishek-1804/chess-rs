//! Static evaluation: incremental tapered PSQT plus a handful of cheap
//! bitboard terms (pawn structure, mobility, rook files, bishop pair, mop-up).

use crate::attacks::*;
use crate::position::Position;
use crate::psqt::MAX_PHASE;
use crate::types::*;

/// Material values used by SEE, MVV-LVA and delta pruning.
pub const SEE_VALUE: [i32; 6] = [100, 320, 330, 500, 900, 20_000];

const TEMPO: i32 = 12;
const BISHOP_PAIR: (i32, i32) = (30, 55);
const DOUBLED: (i32, i32) = (-8, -20);
const ISOLATED: (i32, i32) = (-12, -14);
const PASSED_MG: [i32; 8] = [0, 2, 6, 12, 24, 40, 65, 0];
const PASSED_EG: [i32; 8] = [0, 8, 14, 28, 50, 85, 130, 0];
const ROOK_OPEN: (i32, i32) = (28, 10);
const ROOK_SEMI: (i32, i32) = (12, 6);
// (centre count, mg weight, eg weight) per piece type for mobility
const MOBILITY: [(i32, i32, i32); 6] = [(0, 0, 0), (4, 4, 4), (6, 5, 5), (7, 2, 4), (13, 1, 2), (0, 0, 0)];

#[inline(always)]
fn file_mask(f: u8) -> Bitboard {
    FILE_A << f
}

#[inline(always)]
fn adjacent_files(f: u8) -> Bitboard {
    let m = file_mask(f);
    ((m << 1) & !FILE_A) | ((m >> 1) & !FILE_H)
}

#[inline(always)]
fn pawn_attack_span(c: Color, pawns: Bitboard) -> Bitboard {
    match c {
        Color::White => ((pawns & !FILE_A) << 7) | ((pawns & !FILE_H) << 9),
        Color::Black => ((pawns & !FILE_A) >> 9) | ((pawns & !FILE_H) >> 7),
    }
}

/// Distance of a square from the board centre (0..=6).
#[inline(always)]
fn center_distance(s: Square) -> i32 {
    let f = file_of(s) as i32;
    let r = rank_of(s) as i32;
    (3 - f).max(f - 4) + (3 - r).max(r - 4)
}

#[inline(always)]
fn king_distance(a: Square, b: Square) -> i32 {
    (file_of(a) as i32 - file_of(b) as i32).abs().max((rank_of(a) as i32 - rank_of(b) as i32).abs())
}

/// White-relative (mg, eg) terms for one side.
#[inline(always)]
fn side_terms(pos: &Position, c: Color) -> (i32, i32) {
    use PieceType::*;
    let them = c.flip();
    let occ = pos.occupied();
    let own = pos.color_bb(c);
    let our_pawns = pos.pieces(c, Pawn);
    let their_pawns = pos.pieces(them, Pawn);
    let (mut mg, mut eg) = (0, 0);

    if pos.pieces(c, Bishop).count_ones() >= 2 {
        mg += BISHOP_PAIR.0;
        eg += BISHOP_PAIR.1;
    }

    for sq in bits(our_pawns) {
        let f = file_of(sq);
        if our_pawns & adjacent_files(f) == 0 {
            mg += ISOLATED.0;
            eg += ISOLATED.1;
        }
        if passed_mask(c, sq) & their_pawns == 0 {
            let rel_rank = if c == Color::White { rank_of(sq) } else { 7 - rank_of(sq) } as usize;
            mg += PASSED_MG[rel_rank];
            eg += PASSED_EG[rel_rank];
        }
    }
    for f in 0..8 {
        let n = (our_pawns & file_mask(f)).count_ones() as i32;
        if n > 1 {
            mg += DOUBLED.0 * (n - 1);
            eg += DOUBLED.1 * (n - 1);
        }
    }

    for sq in bits(pos.pieces(c, Rook)) {
        let fm = file_mask(file_of(sq));
        if fm & our_pawns == 0 {
            if fm & their_pawns == 0 {
                mg += ROOK_OPEN.0;
                eg += ROOK_OPEN.1;
            } else {
                mg += ROOK_SEMI.0;
                eg += ROOK_SEMI.1;
            }
        }
    }

    // Mobility into squares not occupied by us and not attacked by enemy pawns.
    let safe = !own & !pawn_attack_span(them, their_pawns);
    for pt in [Knight, Bishop, Rook, Queen] {
        let (centre, wmg, weg) = MOBILITY[pt.idx()];
        for sq in bits(pos.pieces(c, pt)) {
            let att = match pt {
                Knight => knight_attacks(sq),
                Bishop => bishop_attacks(sq, occ),
                Rook => rook_attacks(sq, occ),
                _ => queen_attacks(sq, occ),
            };
            let n = (att & safe).count_ones() as i32 - centre;
            mg += n * wmg;
            eg += n * weg;
        }
    }
    (mg, eg)
}

/// Evaluate from the side to move's point of view, in centipawns.
#[inline]
pub fn evaluate(pos: &Position) -> i32 {
    if let Some(exact) = crate::endgame::probe(pos) {
        return exact;
    }
    let (mut mg, mut eg) = pos.psq();
    let (wmg, weg) = side_terms(pos, Color::White);
    let (bmg, beg) = side_terms(pos, Color::Black);
    mg += wmg - bmg;
    eg += weg - beg;

    // Mop-up: with a lone enemy king, drive it to the edge and approach it.
    for c in Color::BOTH {
        let them = c.flip();
        if pos.color_bb(them) == pos.pieces(them, PieceType::King) && pos.has_non_pawn_material(c) {
            let ek = pos.king_sq(them);
            let bonus = 10 * center_distance(ek) + 4 * (14 - 2 * king_distance(pos.king_sq(c), ek));
            eg += if c == Color::White { bonus } else { -bonus };
        }
    }

    let phase = pos.phase().min(MAX_PHASE);
    let score = (mg * phase + eg * (MAX_PHASE - phase)) / MAX_PHASE;
    let factor = crate::endgame::scale(pos, score);
    if factor == 0 {
        return 0;
    }
    let score = score * factor / 64;
    let score = if pos.side_to_move() == Color::White { score } else { -score };
    score + TEMPO * factor / 64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movegen::legal_moves;

    #[test]
    fn evaluation_is_color_symmetric() {
        let fens = [
            crate::position::START_FEN,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
            "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P3/2NP1N2/PPP1QPPP/R4RK1 w - - 0 10",
            "8/8/8/4k3/8/8/2Q5/4K3 b - - 0 1",
        ];
        for fen in fens {
            let p = Position::from_fen(fen).unwrap();
            assert_eq!(evaluate(&p), evaluate(&p.mirrored()), "{fen}");
        }
    }

    #[test]
    fn startpos_is_roughly_balanced() {
        let e = evaluate(&Position::startpos());
        assert!(e.abs() < 40, "startpos eval {e}");
    }

    #[test]
    fn material_advantage_is_seen() {
        let up_queen = Position::from_fen("rnb1kbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").unwrap();
        assert!(evaluate(&up_queen) > 700);
        assert!(evaluate(&up_queen.mirrored()) > 700); // black to move, black up a queen
    }

    /// Incremental hash and PSQ always equal a from-scratch recomputation.
    #[test]
    fn incremental_state_matches_recomputation() {
        let mut seed = 0xdead_beefu64;
        for _game in 0..200 {
            let mut pos = Position::startpos();
            for _ in 0..150 {
                let moves = legal_moves(&pos);
                if moves.is_empty() {
                    break;
                }
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                pos = pos.make_move(moves[(seed % moves.len() as u64) as usize]);
                assert_eq!(pos.hash(), pos.compute_hash(), "{}", pos.to_fen());
                let (mg, eg, ph) = pos.compute_psq();
                assert_eq!((pos.psq().0, pos.psq().1, pos.phase()), (mg, eg, ph));
                assert_eq!(Position::from_fen(&pos.to_fen()).unwrap(), pos);
            }
        }
    }
}
