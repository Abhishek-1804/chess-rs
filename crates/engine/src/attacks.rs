//! Attack lookups backed by tables generated at compile time (see `build.rs`).

use crate::types::{Bitboard, Color, Square};

#[derive(Copy, Clone)]
pub(crate) struct Magic {
    pub mask: u64,
    pub magic: u64,
    pub offset: u32,
    pub shift: u8,
}

include!(concat!(env!("OUT_DIR"), "/tables.rs"));

#[inline(always)]
fn magic_index(m: &Magic, occ: Bitboard) -> usize {
    m.offset as usize + ((occ & m.mask).wrapping_mul(m.magic) >> m.shift) as usize
}

#[inline(always)]
pub fn rook_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    let idx = magic_index(&ROOK_MAGICS[(sq & 63) as usize], occ);
    debug_assert!(idx < SLIDER_ATTACKS.len());
    // SAFETY: build.rs sizes each square's slice to 2^(64-shift) entries starting
    // at `offset`, and the hash is shifted down to exactly that many bits.
    unsafe { *SLIDER_ATTACKS.get_unchecked(idx) }
}

#[inline(always)]
pub fn bishop_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    let idx = magic_index(&BISHOP_MAGICS[(sq & 63) as usize], occ);
    debug_assert!(idx < SLIDER_ATTACKS.len());
    // SAFETY: see `rook_attacks`.
    unsafe { *SLIDER_ATTACKS.get_unchecked(idx) }
}

#[inline(always)]
pub fn queen_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    rook_attacks(sq, occ) | bishop_attacks(sq, occ)
}

#[inline(always)]
pub fn knight_attacks(sq: Square) -> Bitboard {
    KNIGHT_ATTACKS[(sq & 63) as usize]
}

#[inline(always)]
pub fn king_attacks(sq: Square) -> Bitboard {
    KING_ATTACKS[(sq & 63) as usize]
}

/// Squares attacked by a pawn of color `c` standing on `sq`.
#[inline(always)]
pub fn pawn_attacks(c: Color, sq: Square) -> Bitboard {
    PAWN_ATTACKS[c.idx()][(sq & 63) as usize]
}

/// Squares strictly between `a` and `b` if they share a line, else empty.
#[inline(always)]
pub fn between(a: Square, b: Square) -> Bitboard {
    BETWEEN[(a & 63) as usize][(b & 63) as usize]
}

/// The full line through `a` and `b` (edge to edge) if aligned, else empty.
#[inline(always)]
pub fn line(a: Square, b: Square) -> Bitboard {
    LINE[(a & 63) as usize][(b & 63) as usize]
}

/// Squares in front of a pawn on the same and adjacent files.
#[inline(always)]
pub fn passed_mask(c: Color, sq: Square) -> Bitboard {
    PASSED_MASK[c.idx()][(sq & 63) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow_slide(sq: Square, occ: u64, dirs: &[(i32, i32)]) -> u64 {
        let (r0, f0) = ((sq / 8) as i32, (sq % 8) as i32);
        let mut att = 0;
        for &(dr, df) in dirs {
            let (mut r, mut f) = (r0 + dr, f0 + df);
            while (0..8).contains(&r) && (0..8).contains(&f) {
                let b = 1u64 << (r * 8 + f);
                att |= b;
                if occ & b != 0 {
                    break;
                }
                r += dr;
                f += df;
            }
        }
        att
    }

    #[test]
    fn magic_lookups_match_reference_on_random_occupancies() {
        let mut x = 0x1234_5678_9abc_def1u64;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let occ = x & x.rotate_left(17); // ~25% density
            for sq in 0..64 {
                assert_eq!(rook_attacks(sq, occ), slow_slide(sq, occ, &[(1, 0), (-1, 0), (0, 1), (0, -1)]));
                assert_eq!(bishop_attacks(sq, occ), slow_slide(sq, occ, &[(1, 1), (1, -1), (-1, 1), (-1, -1)]));
            }
        }
    }

    #[test]
    fn leaper_tables() {
        assert_eq!(knight_attacks(0).count_ones(), 2);
        assert_eq!(knight_attacks(27).count_ones(), 8);
        assert_eq!(king_attacks(0).count_ones(), 3);
        assert_eq!(pawn_attacks(Color::White, 12), (1 << 19) | (1 << 21)); // e2 -> d3,f3
        assert_eq!(pawn_attacks(Color::Black, 52), (1 << 43) | (1 << 45)); // e7 -> d6,f6
        assert_eq!(pawn_attacks(Color::White, 8), 1 << 17); // a2 -> b3
    }

    #[test]
    fn between_and_line() {
        assert_eq!(between(0, 7), 0x7E); // a1..h1
        assert_eq!(between(0, 63).count_ones(), 6);
        assert_eq!(between(0, 10), 0); // a1, c2 not aligned
        assert_eq!(line(0, 9).count_ones(), 8);
        assert_eq!(line(0, 10), 0);
    }
}
