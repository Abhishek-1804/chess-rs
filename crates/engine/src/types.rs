//! Core chess primitives: colors, pieces, squares, bitboards and moves.

use std::fmt;

/// A set of squares, one bit per square (bit 0 = a1, bit 63 = h8).
pub type Bitboard = u64;
/// Square index, 0 = a1 … 63 = h8.
pub type Square = u8;

pub const NO_SQ: Square = 64;

pub const FILE_A: Bitboard = 0x0101_0101_0101_0101;
pub const FILE_H: Bitboard = FILE_A << 7;
pub const RANK_1: Bitboard = 0xFF;
pub const RANK_2: Bitboard = RANK_1 << 8;
pub const RANK_3: Bitboard = RANK_1 << 16;
pub const RANK_6: Bitboard = RANK_1 << 40;
pub const RANK_7: Bitboard = RANK_1 << 48;
pub const RANK_8: Bitboard = RANK_1 << 56;
pub const DARK_SQUARES: Bitboard = 0xAA55_AA55_AA55_AA55;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    White = 0,
    Black = 1,
}

impl Color {
    pub const BOTH: [Color; 2] = [Color::White, Color::Black];

    #[inline(always)]
    pub const fn flip(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }

    #[inline(always)]
    pub const fn idx(self) -> usize {
        self as usize
    }

    pub const fn name(self) -> &'static str {
        match self {
            Color::White => "White",
            Color::Black => "Black",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PieceType {
    Pawn = 0,
    Knight = 1,
    Bishop = 2,
    Rook = 3,
    Queen = 4,
    King = 5,
}

impl PieceType {
    pub const ALL: [PieceType; 6] =
        [PieceType::Pawn, PieceType::Knight, PieceType::Bishop, PieceType::Rook, PieceType::Queen, PieceType::King];

    #[inline(always)]
    pub const fn idx(self) -> usize {
        self as usize
    }

    /// Lower-case letter (`p n b r q k`).
    pub const fn to_char(self) -> char {
        b"pnbrqk"[self as usize] as char
    }

    pub fn from_char(c: char) -> Option<PieceType> {
        Some(match c.to_ascii_lowercase() {
            'p' => PieceType::Pawn,
            'n' => PieceType::Knight,
            'b' => PieceType::Bishop,
            'r' => PieceType::Rook,
            'q' => PieceType::Queen,
            'k' => PieceType::King,
            _ => return None,
        })
    }
}

/// A colored piece. Twelve variants so `Option<Piece>` packs into one byte.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Piece {
    WP,
    WN,
    WB,
    WR,
    WQ,
    WK,
    BP,
    BN,
    BB,
    BR,
    BQ,
    BK,
}

impl Piece {
    pub const ALL: [Piece; 12] = [
        Piece::WP,
        Piece::WN,
        Piece::WB,
        Piece::WR,
        Piece::WQ,
        Piece::WK,
        Piece::BP,
        Piece::BN,
        Piece::BB,
        Piece::BR,
        Piece::BQ,
        Piece::BK,
    ];

    #[inline(always)]
    pub const fn new(c: Color, pt: PieceType) -> Piece {
        Self::ALL[c as usize * 6 + pt as usize]
    }

    #[inline(always)]
    pub const fn color(self) -> Color {
        if (self as u8) < 6 { Color::White } else { Color::Black }
    }

    #[inline(always)]
    pub const fn kind(self) -> PieceType {
        PieceType::ALL[self as usize % 6]
    }

    #[inline(always)]
    pub const fn idx(self) -> usize {
        self as usize
    }

    /// FEN letter: upper-case for white, lower-case for black.
    pub fn to_char(self) -> char {
        let c = self.kind().to_char();
        if self.color() == Color::White { c.to_ascii_uppercase() } else { c }
    }

    pub fn from_char(c: char) -> Option<Piece> {
        let pt = PieceType::from_char(c)?;
        let color = if c.is_ascii_uppercase() { Color::White } else { Color::Black };
        Some(Piece::new(color, pt))
    }
}

#[inline(always)]
pub const fn make_sq(file: u8, rank: u8) -> Square {
    rank * 8 + file
}

#[inline(always)]
pub const fn file_of(s: Square) -> u8 {
    s & 7
}

#[inline(always)]
pub const fn rank_of(s: Square) -> u8 {
    s >> 3
}

#[inline(always)]
pub const fn bb(s: Square) -> Bitboard {
    1u64 << s
}

#[inline(always)]
pub const fn lsb(b: Bitboard) -> Square {
    b.trailing_zeros() as Square
}

pub fn sq_name(s: Square) -> String {
    let mut out = String::with_capacity(2);
    out.push((b'a' + file_of(s)) as char);
    out.push((b'1' + rank_of(s)) as char);
    out
}

pub fn parse_sq(s: &str) -> Option<Square> {
    let b = s.as_bytes();
    if b.len() != 2 || !(b'a'..=b'h').contains(&b[0]) || !(b'1'..=b'8').contains(&b[1]) {
        return None;
    }
    Some(make_sq(b[0] - b'a', b[1] - b'1'))
}

/// Iterator over the squares of a bitboard, lowest first.
#[derive(Copy, Clone)]
pub struct BitIter(pub Bitboard);

impl Iterator for BitIter {
    type Item = Square;

    #[inline(always)]
    fn next(&mut self) -> Option<Square> {
        if self.0 == 0 {
            None
        } else {
            let s = lsb(self.0);
            self.0 &= self.0 - 1;
            Some(s)
        }
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.0.count_ones() as usize;
        (n, Some(n))
    }
}

impl ExactSizeIterator for BitIter {}

#[inline(always)]
pub const fn bits(b: Bitboard) -> BitIter {
    BitIter(b)
}

/// Move flags (4 bits), chessprogramming-wiki layout.
pub mod flag {
    pub const QUIET: u16 = 0;
    pub const DOUBLE_PUSH: u16 = 1;
    pub const KING_CASTLE: u16 = 2;
    pub const QUEEN_CASTLE: u16 = 3;
    pub const CAPTURE: u16 = 4;
    pub const EP_CAPTURE: u16 = 5;
    pub const PROMO: u16 = 8; // + 0..3 => N, B, R, Q
    pub const PROMO_CAPTURE: u16 = 12; // + 0..3 => N, B, R, Q
}

/// A move packed in 16 bits: from (6) | to (6) | flags (4).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct Move(pub u16);

impl Move {
    pub const NULL: Move = Move(0);

    #[inline(always)]
    pub const fn new(from: Square, to: Square, flags: u16) -> Move {
        Move(from as u16 | (to as u16) << 6 | flags << 12)
    }

    #[inline(always)]
    pub const fn from(self) -> Square {
        (self.0 & 63) as Square
    }

    #[inline(always)]
    pub const fn to(self) -> Square {
        ((self.0 >> 6) & 63) as Square
    }

    #[inline(always)]
    pub const fn flags(self) -> u16 {
        self.0 >> 12
    }

    #[inline(always)]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    #[inline(always)]
    pub const fn is_capture(self) -> bool {
        self.flags() & 4 != 0
    }

    #[inline(always)]
    pub const fn is_promotion(self) -> bool {
        self.flags() & 8 != 0
    }

    #[inline(always)]
    pub const fn is_ep(self) -> bool {
        self.flags() == flag::EP_CAPTURE
    }

    #[inline(always)]
    pub const fn is_castle(self) -> bool {
        matches!(self.flags(), flag::KING_CASTLE | flag::QUEEN_CASTLE)
    }

    /// Captures and promotions: the moves quiescence search looks at.
    #[inline(always)]
    pub const fn is_noisy(self) -> bool {
        self.flags() & 12 != 0
    }

    #[inline(always)]
    pub const fn promotion(self) -> Option<PieceType> {
        if self.is_promotion() { Some(PieceType::ALL[(self.flags() & 3) as usize + 1]) } else { None }
    }

    /// Long-algebraic / UCI form, e.g. `e2e4`, `e7e8q`.
    pub fn to_uci(self) -> String {
        let mut s = sq_name(self.from());
        s.push_str(&sq_name(self.to()));
        if let Some(p) = self.promotion() {
            s.push(p.to_char());
        }
        s
    }
}

impl fmt::Display for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() { f.write_str("0000") } else { f.write_str(&self.to_uci()) }
    }
}

impl fmt::Debug for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_packing_roundtrips() {
        for from in 0..64u8 {
            for to in 0..64u8 {
                for fl in [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 12, 13, 14, 15] {
                    let m = Move::new(from, to, fl);
                    assert_eq!((m.from(), m.to(), m.flags()), (from, to, fl));
                }
            }
        }
        assert_eq!(Move::new(12, 28, flag::DOUBLE_PUSH).to_uci(), "e2e4");
        assert_eq!(Move::new(52, 60, flag::PROMO + 3).to_uci(), "e7e8q");
        assert_eq!(Move::new(52, 61, flag::PROMO_CAPTURE).promotion(), Some(PieceType::Knight));
    }

    #[test]
    fn option_piece_is_one_byte() {
        assert_eq!(std::mem::size_of::<Option<Piece>>(), 1);
    }

    #[test]
    fn square_names() {
        assert_eq!(sq_name(0), "a1");
        assert_eq!(sq_name(63), "h8");
        for s in 0..64 {
            assert_eq!(parse_sq(&sq_name(s)), Some(s));
        }
        assert_eq!(parse_sq("i1"), None);
        assert_eq!(parse_sq("a9"), None);
    }
}
