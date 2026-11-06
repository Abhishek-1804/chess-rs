//! Fully legal move generation using check and pin masks.
//!
//! No make/test/unmake: every generated move is legal, which makes both perft
//! and search cheaper. `NOISY` is a const generic, so the capture-only variant
//! used by quiescence search is a separately monomorphised function with the
//! quiet-move branches compiled out.

use std::ops::Deref;

use crate::attacks::*;
use crate::position::*;
use crate::types::*;

pub const MAX_MOVES: usize = 256;

/// Fixed-capacity, stack-allocated move list (no heap traffic in search).
#[derive(Clone)]
pub struct MoveList {
    moves: [Move; MAX_MOVES],
    len: usize,
}

impl MoveList {
    #[inline(always)]
    pub fn new() -> Self {
        MoveList { moves: [Move::NULL; MAX_MOVES], len: 0 }
    }

    #[inline(always)]
    pub fn push(&mut self, m: Move) {
        debug_assert!(self.len < MAX_MOVES);
        // SAFETY: no chess position has more than 218 legal moves.
        unsafe { *self.moves.get_unchecked_mut(self.len) = m };
        self.len += 1;
    }

    #[inline(always)]
    pub fn swap(&mut self, a: usize, b: usize) {
        self.moves[..self.len].swap(a, b);
    }
}

impl Default for MoveList {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for MoveList {
    type Target = [Move];
    #[inline(always)]
    fn deref(&self) -> &[Move] {
        &self.moves[..self.len]
    }
}

impl std::fmt::Debug for MoveList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// All legal moves.
#[inline]
pub fn legal_moves(pos: &Position) -> MoveList {
    let mut list = MoveList::new();
    generate::<false>(pos, &mut list);
    list
}

/// Legal captures and promotions.
#[inline]
pub fn noisy_moves(pos: &Position) -> MoveList {
    let mut list = MoveList::new();
    generate::<true>(pos, &mut list);
    list
}

#[inline(always)]
fn push_targets(list: &mut MoveList, from: Square, targets: Bitboard, theirs: Bitboard) {
    for to in bits(targets & theirs) {
        list.push(Move::new(from, to, flag::CAPTURE));
    }
    for to in bits(targets & !theirs) {
        list.push(Move::new(from, to, flag::QUIET));
    }
}

#[inline(always)]
fn push_promotions(list: &mut MoveList, from: Square, to: Square, base: u16) {
    // Queen first: better for move ordering when nothing else distinguishes them.
    for p in [3, 0, 2, 1] {
        list.push(Move::new(from, to, base + p));
    }
}

fn generate<const NOISY: bool>(pos: &Position, list: &mut MoveList) {
    use PieceType::*;
    let us = pos.side_to_move();
    let them = us.flip();
    let occ = pos.occupied();
    let ours = pos.color_bb(us);
    let theirs = pos.color_bb(them);
    let ksq = pos.king_sq(us);
    let checkers = pos.attackers_to(ksq, occ) & theirs;

    // King moves: test each destination with the king lifted off the board so
    // sliders "see through" its current square.
    let occ_no_king = occ ^ bb(ksq);
    let king_targets = king_attacks(ksq) & if NOISY { theirs } else { !ours };
    for to in bits(king_targets) {
        if !pos.is_attacked(to, them, occ_no_king) {
            let fl = if theirs & bb(to) != 0 { flag::CAPTURE } else { flag::QUIET };
            list.push(Move::new(ksq, to, fl));
        }
    }
    if checkers & checkers.wrapping_sub(1) != 0 {
        return; // double check: only the king may move
    }

    let check_mask = if checkers != 0 { between(ksq, lsb(checkers)) | checkers } else { !0 };
    let pinned = pos.pinned(us);
    let targets = (if NOISY { theirs } else { !ours }) & check_mask;

    for from in bits(pos.pieces(us, Knight) & !pinned) {
        push_targets(list, from, knight_attacks(from) & targets, theirs);
    }
    for from in bits(pos.diagonal_sliders(us)) {
        let mut t = bishop_attacks(from, occ) & targets;
        if pinned & bb(from) != 0 {
            t &= line(ksq, from);
        }
        push_targets(list, from, t, theirs);
    }
    for from in bits(pos.orthogonal_sliders(us)) {
        let mut t = rook_attacks(from, occ) & targets;
        if pinned & bb(from) != 0 {
            t &= line(ksq, from);
        }
        push_targets(list, from, t, theirs);
    }

    // ---- Pawns (set-wise shifts) ----
    let pawns = pos.pieces(us, Pawn);
    let white = us == Color::White;
    let up = |b: Bitboard| if white { b << 8 } else { b >> 8 };
    let (promo_rank, third_rank) = if white { (RANK_8, RANK_3) } else { (RANK_1, RANK_6) };
    let fwd: i8 = if white { 8 } else { -8 };
    let pin_ok = |from: Square, to: Square| pinned & bb(from) == 0 || line(ksq, from) & bb(to) != 0;
    let origin = |to: Square, delta: i8| (to as i8 - delta) as Square;

    let empty = !occ;
    let single = up(pawns) & empty;
    if !NOISY {
        let double = up(single & third_rank) & empty;
        for to in bits(single & check_mask & !promo_rank) {
            let from = origin(to, fwd);
            if pin_ok(from, to) {
                list.push(Move::new(from, to, flag::QUIET));
            }
        }
        for to in bits(double & check_mask) {
            let from = origin(to, 2 * fwd);
            if pin_ok(from, to) {
                list.push(Move::new(from, to, flag::DOUBLE_PUSH));
            }
        }
    }
    for to in bits(single & check_mask & promo_rank) {
        let from = origin(to, fwd);
        if pin_ok(from, to) {
            push_promotions(list, from, to, flag::PROMO);
        }
    }

    // Diagonal captures: (attack set, from->to delta)
    let (west, west_d, east, east_d) = if white {
        ((pawns & !FILE_A) << 7, 7i8, (pawns & !FILE_H) << 9, 9i8)
    } else {
        ((pawns & !FILE_A) >> 9, -9i8, (pawns & !FILE_H) >> 7, -7i8)
    };
    let cap_targets = theirs & check_mask;
    for (set, delta) in [(west, west_d), (east, east_d)] {
        for to in bits(set & cap_targets) {
            let from = origin(to, delta);
            if pin_ok(from, to) {
                if promo_rank & bb(to) != 0 {
                    push_promotions(list, from, to, flag::PROMO_CAPTURE);
                } else {
                    list.push(Move::new(from, to, flag::CAPTURE));
                }
            }
        }
    }

    // En passant: rare, so verify legality exactly by re-testing slider
    // attacks on the king with both pawns moved (catches the horizontal
    // two-pawn discovered check that pin masks cannot express).
    if let Some(ep) = pos.ep_square() {
        let cap_sq = ep ^ 8;
        if check_mask & (bb(ep) | bb(cap_sq)) != 0 {
            for from in bits(pawn_attacks(them, ep) & pawns) {
                let occ2 = (occ ^ bb(from) ^ bb(cap_sq)) | bb(ep);
                if rook_attacks(ksq, occ2) & pos.orthogonal_sliders(them) == 0
                    && bishop_attacks(ksq, occ2) & pos.diagonal_sliders(them) == 0
                {
                    list.push(Move::new(from, ep, flag::EP_CAPTURE));
                }
            }
        }
    }

    // ---- Castling ----
    if !NOISY && checkers == 0 {
        let rights = pos.castling_rights();
        let (oo, ooo, base) = if white { (WHITE_OO, WHITE_OOO, 0u8) } else { (BLACK_OO, BLACK_OOO, 56u8) };
        let safe = |sq: Square| !pos.is_attacked(sq, them, occ);
        if rights & oo != 0 && occ & (bb(base + 5) | bb(base + 6)) == 0 && safe(base + 5) && safe(base + 6) {
            list.push(Move::new(base + 4, base + 6, flag::KING_CASTLE));
        }
        if rights & ooo != 0
            && occ & (bb(base + 1) | bb(base + 2) | bb(base + 3)) == 0
            && safe(base + 3)
            && safe(base + 2)
        {
            list.push(Move::new(base + 4, base + 2, flag::QUEEN_CASTLE));
        }
    }
}

/// Count leaf nodes of the legal move tree (bulk-counted at the last ply).
pub fn perft(pos: &Position, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = legal_moves(pos);
    if depth == 1 {
        return moves.len() as u64;
    }
    moves.iter().map(|&m| perft(&pos.make_move(m), depth - 1)).sum()
}

/// Perft split by root move, parallelised across root moves with scoped threads.
pub fn perft_divide(pos: &Position, depth: u32) -> Vec<(Move, u64)> {
    let moves = legal_moves(pos);
    if depth <= 1 {
        return moves.iter().map(|&m| (m, 1)).collect();
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = moves
            .iter()
            .map(|&m| {
                let child = pos.make_move(m);
                s.spawn(move || (m, perft(&child, depth - 1)))
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("perft thread panicked")).collect()
    })
}

/// Parallel perft total.
pub fn perft_parallel(pos: &Position, depth: u32) -> u64 {
    if depth <= 2 {
        return perft(pos, depth);
    }
    perft_divide(pos, depth).iter().map(|&(_, n)| n).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startpos_has_twenty_moves() {
        assert_eq!(legal_moves(&Position::startpos()).len(), 20);
        assert_eq!(noisy_moves(&Position::startpos()).len(), 0);
    }

    /// Every noisy move is legal and every legal noisy move is generated.
    #[test]
    fn noisy_is_exact_subset_of_legal() {
        let fens = [
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
            "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
        ];
        for fen in fens {
            let mut pos = Position::from_fen(fen).unwrap();
            for ply in 0..40 {
                let all = legal_moves(&pos);
                if all.is_empty() {
                    break;
                }
                let mut want: Vec<_> = all.iter().copied().filter(|m| m.is_noisy()).collect();
                let mut got: Vec<_> = noisy_moves(&pos).to_vec();
                want.sort_by_key(|m| m.0);
                got.sort_by_key(|m| m.0);
                assert_eq!(want, got, "{}", pos.to_fen());
                pos = pos.make_move(all[(ply * 7 + 3) % all.len()]);
            }
        }
    }
}
