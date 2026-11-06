//! Board representation: bitboards + mailbox, copy-make move application with
//! incrementally maintained Zobrist hash and piece-square scores.

use std::fmt;

use crate::attacks::*;
use crate::psqt::{PSQ_EG, PSQ_MG, phase_inc};
use crate::types::*;
use crate::zobrist::ZOBRIST;

pub const WHITE_OO: u8 = 1;
pub const WHITE_OOO: u8 = 2;
pub const BLACK_OO: u8 = 4;
pub const BLACK_OOO: u8 = 8;

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Castling rights that survive a move touching each square.
const CASTLE_MASK: [u8; 64] = {
    let mut m = [15u8; 64];
    m[0] = 15 & !WHITE_OOO;
    m[4] = 15 & !(WHITE_OO | WHITE_OOO);
    m[7] = 15 & !WHITE_OO;
    m[56] = 15 & !BLACK_OOO;
    m[60] = 15 & !(BLACK_OO | BLACK_OOO);
    m[63] = 15 & !BLACK_OO;
    m
};

/// A complete chess position. `Copy` and ~160 bytes: search uses copy-make,
/// which avoids unmake bookkeeping entirely.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Position {
    by_type: [Bitboard; 6],
    by_color: [Bitboard; 2],
    mailbox: [Option<Piece>; 64],
    hash: u64,
    psq_mg: i32,
    psq_eg: i32,
    phase: i32,
    side: Color,
    castling: u8,
    ep: Square,
    halfmove: u16,
    fullmove: u16,
}

impl Default for Position {
    fn default() -> Self {
        Self::startpos()
    }
}

impl Position {
    fn empty() -> Self {
        Position {
            by_type: [0; 6],
            by_color: [0; 2],
            mailbox: [None; 64],
            hash: 0,
            psq_mg: 0,
            psq_eg: 0,
            phase: 0,
            side: Color::White,
            castling: 0,
            ep: NO_SQ,
            halfmove: 0,
            fullmove: 1,
        }
    }

    pub fn startpos() -> Self {
        Self::from_fen(START_FEN).expect("start FEN is valid")
    }

    // ------------------------------------------------------------------ accessors

    #[inline(always)]
    pub fn side_to_move(&self) -> Color {
        self.side
    }
    #[inline(always)]
    pub fn hash(&self) -> u64 {
        self.hash
    }
    #[inline(always)]
    pub fn halfmove_clock(&self) -> u16 {
        self.halfmove
    }
    #[inline(always)]
    pub fn fullmove_number(&self) -> u16 {
        self.fullmove
    }
    #[inline(always)]
    pub fn castling_rights(&self) -> u8 {
        self.castling
    }
    /// En-passant target square, only set when a capture there is pseudo-legal.
    #[inline(always)]
    pub fn ep_square(&self) -> Option<Square> {
        (self.ep != NO_SQ).then_some(self.ep)
    }
    #[inline(always)]
    pub fn piece_on(&self, sq: Square) -> Option<Piece> {
        self.mailbox[(sq & 63) as usize]
    }
    #[inline(always)]
    pub fn pieces(&self, c: Color, pt: PieceType) -> Bitboard {
        self.by_type[pt.idx()] & self.by_color[c.idx()]
    }
    #[inline(always)]
    pub fn pieces_of_type(&self, pt: PieceType) -> Bitboard {
        self.by_type[pt.idx()]
    }
    #[inline(always)]
    pub fn color_bb(&self, c: Color) -> Bitboard {
        self.by_color[c.idx()]
    }
    #[inline(always)]
    pub fn occupied(&self) -> Bitboard {
        self.by_color[0] | self.by_color[1]
    }
    #[inline(always)]
    pub fn king_sq(&self, c: Color) -> Square {
        lsb(self.pieces(c, PieceType::King))
    }
    #[inline(always)]
    pub fn psq(&self) -> (i32, i32) {
        (self.psq_mg, self.psq_eg)
    }
    #[inline(always)]
    pub fn phase(&self) -> i32 {
        self.phase
    }

    #[inline(always)]
    pub fn diagonal_sliders(&self, c: Color) -> Bitboard {
        (self.by_type[PieceType::Bishop.idx()] | self.by_type[PieceType::Queen.idx()]) & self.by_color[c.idx()]
    }
    #[inline(always)]
    pub fn orthogonal_sliders(&self, c: Color) -> Bitboard {
        (self.by_type[PieceType::Rook.idx()] | self.by_type[PieceType::Queen.idx()]) & self.by_color[c.idx()]
    }

    /// All pieces (of both colors) attacking `sq` given occupancy `occ`.
    #[inline(always)]
    pub fn attackers_to(&self, sq: Square, occ: Bitboard) -> Bitboard {
        use PieceType::*;
        let bq = self.by_type[Bishop.idx()] | self.by_type[Queen.idx()];
        let rq = self.by_type[Rook.idx()] | self.by_type[Queen.idx()];
        (pawn_attacks(Color::White, sq) & self.pieces(Color::Black, Pawn))
            | (pawn_attacks(Color::Black, sq) & self.pieces(Color::White, Pawn))
            | (knight_attacks(sq) & self.by_type[Knight.idx()])
            | (king_attacks(sq) & self.by_type[King.idx()])
            | (bishop_attacks(sq, occ) & bq)
            | (rook_attacks(sq, occ) & rq)
    }

    /// Is `sq` attacked by any piece of color `by`, given occupancy `occ`?
    #[inline(always)]
    pub fn is_attacked(&self, sq: Square, by: Color, occ: Bitboard) -> bool {
        use PieceType::*;
        (pawn_attacks(by.flip(), sq) & self.pieces(by, Pawn)) != 0
            || (knight_attacks(sq) & self.pieces(by, Knight)) != 0
            || (king_attacks(sq) & self.pieces(by, King)) != 0
            || (bishop_attacks(sq, occ) & self.diagonal_sliders(by)) != 0
            || (rook_attacks(sq, occ) & self.orthogonal_sliders(by)) != 0
    }

    #[inline(always)]
    pub fn in_check(&self) -> bool {
        self.is_attacked(self.king_sq(self.side), self.side.flip(), self.occupied())
    }

    #[inline(always)]
    pub fn checkers(&self) -> Bitboard {
        self.attackers_to(self.king_sq(self.side), self.occupied()) & self.by_color[self.side.flip().idx()]
    }

    /// Our pieces that are absolutely pinned to our king.
    #[inline(always)]
    pub fn pinned(&self, us: Color) -> Bitboard {
        let them = us.flip();
        let ksq = self.king_sq(us);
        let occ = self.occupied();
        let snipers = (rook_attacks(ksq, 0) & self.orthogonal_sliders(them))
            | (bishop_attacks(ksq, 0) & self.diagonal_sliders(them));
        let mut pinned = 0;
        for s in bits(snipers) {
            let b = between(ksq, s) & occ;
            if b != 0 && b & (b - 1) == 0 {
                pinned |= b & self.by_color[us.idx()];
            }
        }
        pinned
    }

    /// Does `c` have anything besides pawns and king? (Null-move safety.)
    #[inline(always)]
    pub fn has_non_pawn_material(&self, c: Color) -> bool {
        let pk = self.by_type[PieceType::Pawn.idx()] | self.by_type[PieceType::King.idx()];
        self.by_color[c.idx()] & !pk != 0
    }

    /// Dead positions under FIDE rules that we can detect cheaply:
    /// K v K, K+minor v K, and bishops-only all on one square color.
    pub fn insufficient_material(&self) -> bool {
        use PieceType::*;
        if self.by_type[Pawn.idx()] | self.by_type[Rook.idx()] | self.by_type[Queen.idx()] != 0 {
            return false;
        }
        let minors = self.by_type[Knight.idx()] | self.by_type[Bishop.idx()];
        if minors.count_ones() <= 1 {
            return true;
        }
        if self.by_type[Knight.idx()] == 0 {
            let b = self.by_type[Bishop.idx()];
            return b & DARK_SQUARES == 0 || b & !DARK_SQUARES == 0;
        }
        false
    }

    // ------------------------------------------------------------------ mutation

    #[inline(always)]
    fn put_piece(&mut self, sq: Square, p: Piece) {
        let b = bb(sq);
        self.by_type[p.kind().idx()] |= b;
        self.by_color[p.color().idx()] |= b;
        self.mailbox[sq as usize] = Some(p);
        self.hash ^= ZOBRIST.pieces[p.idx()][sq as usize];
        self.psq_mg += PSQ_MG[p.idx()][sq as usize];
        self.psq_eg += PSQ_EG[p.idx()][sq as usize];
        self.phase += phase_inc(p.kind());
    }

    #[inline(always)]
    fn remove_piece(&mut self, sq: Square) {
        let Some(p) = self.mailbox[sq as usize] else { unreachable!("remove_piece on empty square {}", sq_name(sq)) };
        let b = bb(sq);
        self.by_type[p.kind().idx()] ^= b;
        self.by_color[p.color().idx()] ^= b;
        self.mailbox[sq as usize] = None;
        self.hash ^= ZOBRIST.pieces[p.idx()][sq as usize];
        self.psq_mg -= PSQ_MG[p.idx()][sq as usize];
        self.psq_eg -= PSQ_EG[p.idx()][sq as usize];
        self.phase -= phase_inc(p.kind());
    }

    #[inline(always)]
    fn move_piece(&mut self, from: Square, to: Square, p: Piece) {
        let mask = bb(from) | bb(to);
        self.by_type[p.kind().idx()] ^= mask;
        self.by_color[p.color().idx()] ^= mask;
        self.mailbox[from as usize] = None;
        self.mailbox[to as usize] = Some(p);
        let (pi, f, t) = (p.idx(), from as usize, to as usize);
        self.hash ^= ZOBRIST.pieces[pi][f] ^ ZOBRIST.pieces[pi][t];
        self.psq_mg += PSQ_MG[pi][t] - PSQ_MG[pi][f];
        self.psq_eg += PSQ_EG[pi][t] - PSQ_EG[pi][f];
    }

    /// Apply a (legal) move and return the resulting position.
    #[inline]
    pub fn make_move(&self, m: Move) -> Position {
        let mut p = *self;
        let us = self.side;
        let (from, to, fl) = (m.from(), m.to(), m.flags());
        let Some(moving) = self.mailbox[from as usize] else { unreachable!("make_move {m}: no piece on from-square") };
        p.halfmove += 1;
        if p.ep != NO_SQ {
            p.hash ^= ZOBRIST.ep[file_of(p.ep) as usize];
            p.ep = NO_SQ;
        }
        if m.is_capture() {
            // For en passant the captured pawn sits one rank behind the target.
            p.remove_piece(if fl == flag::EP_CAPTURE { to ^ 8 } else { to });
            p.halfmove = 0;
        }
        p.move_piece(from, to, moving);
        match moving.kind() {
            PieceType::Pawn => {
                p.halfmove = 0;
                if fl == flag::DOUBLE_PUSH {
                    let ep = (from + to) / 2;
                    if pawn_attacks(us, ep) & p.pieces(us.flip(), PieceType::Pawn) != 0 {
                        p.ep = ep;
                        p.hash ^= ZOBRIST.ep[file_of(ep) as usize];
                    }
                } else if let Some(promo) = m.promotion() {
                    p.remove_piece(to);
                    p.put_piece(to, Piece::new(us, promo));
                }
            }
            PieceType::King if m.is_castle() => {
                let (rf, rt) = if fl == flag::KING_CASTLE { (to + 1, to - 1) } else { (to - 2, to + 1) };
                p.move_piece(rf, rt, Piece::new(us, PieceType::Rook));
            }
            _ => {}
        }
        let rights = p.castling & CASTLE_MASK[from as usize] & CASTLE_MASK[to as usize];
        if rights != p.castling {
            p.hash ^= ZOBRIST.castling[p.castling as usize] ^ ZOBRIST.castling[rights as usize];
            p.castling = rights;
        }
        if us == Color::Black {
            p.fullmove += 1;
        }
        p.side = us.flip();
        p.hash ^= ZOBRIST.side;
        p
    }

    /// Pass the move (null-move pruning). Resets the halfmove clock so
    /// repetition detection never looks across a null move.
    #[inline]
    pub fn make_null(&self) -> Position {
        let mut p = *self;
        if p.ep != NO_SQ {
            p.hash ^= ZOBRIST.ep[file_of(p.ep) as usize];
            p.ep = NO_SQ;
        }
        p.side = p.side.flip();
        p.hash ^= ZOBRIST.side;
        p.halfmove = 0;
        p
    }

    // ------------------------------------------------------------------ FEN

    pub fn from_fen(fen: &str) -> Result<Position, String> {
        let parts: Vec<&str> = fen.split_whitespace().collect();
        if parts.len() < 4 {
            return Err("FEN needs at least 4 fields".into());
        }
        let mut pos = Position::empty();

        let ranks: Vec<&str> = parts[0].split('/').collect();
        if ranks.len() != 8 {
            return Err("FEN board must have 8 ranks".into());
        }
        for (i, rank_str) in ranks.iter().enumerate() {
            let rank = 7 - i as u8;
            let mut file = 0u8;
            for c in rank_str.chars() {
                if let Some(d) = c.to_digit(10) {
                    if !(1..=8).contains(&d) {
                        return Err(format!("bad digit '{c}' in FEN"));
                    }
                    file += d as u8;
                } else {
                    let p = Piece::from_char(c).ok_or_else(|| format!("bad piece '{c}' in FEN"))?;
                    if file >= 8 {
                        return Err(format!("rank {} overflows", rank + 1));
                    }
                    pos.put_piece(make_sq(file, rank), p);
                    file += 1;
                }
                if file > 8 {
                    return Err(format!("rank {} overflows", rank + 1));
                }
            }
            if file != 8 {
                return Err(format!("rank {} has {} files", rank + 1, file));
            }
        }

        pos.side = match parts[1] {
            "w" => Color::White,
            "b" => Color::Black,
            s => return Err(format!("bad side to move '{s}'")),
        };

        if parts[2] != "-" {
            for c in parts[2].chars() {
                pos.castling |= match c {
                    'K' => WHITE_OO,
                    'Q' => WHITE_OOO,
                    'k' => BLACK_OO,
                    'q' => BLACK_OOO,
                    _ => return Err(format!("bad castling flag '{c}'")),
                };
            }
        }

        for c in Color::BOTH {
            if pos.pieces(c, PieceType::King).count_ones() != 1 {
                return Err(format!("{} must have exactly one king", c.name()));
            }
        }
        if pos.by_type[PieceType::Pawn.idx()] & (RANK_1 | RANK_8) != 0 {
            return Err("pawns cannot stand on the first or last rank".into());
        }

        // Drop castling rights that the piece placement contradicts.
        let has = |pos: &Position, sq: Square, p: Piece| pos.mailbox[sq as usize] == Some(p);
        if !has(&pos, 4, Piece::WK) {
            pos.castling &= !(WHITE_OO | WHITE_OOO);
        }
        if !has(&pos, 7, Piece::WR) {
            pos.castling &= !WHITE_OO;
        }
        if !has(&pos, 0, Piece::WR) {
            pos.castling &= !WHITE_OOO;
        }
        if !has(&pos, 60, Piece::BK) {
            pos.castling &= !(BLACK_OO | BLACK_OOO);
        }
        if !has(&pos, 63, Piece::BR) {
            pos.castling &= !BLACK_OO;
        }
        if !has(&pos, 56, Piece::BR) {
            pos.castling &= !BLACK_OOO;
        }

        if parts[3] != "-" {
            let ep = parse_sq(parts[3]).ok_or_else(|| format!("bad en-passant square '{}'", parts[3]))?;
            let us = pos.side;
            let expected_rank = if us == Color::White { 5 } else { 2 };
            if rank_of(ep) == expected_rank
                && pos.mailbox[(ep ^ 8) as usize] == Some(Piece::new(us.flip(), PieceType::Pawn))
                && pawn_attacks(us.flip(), ep) & pos.pieces(us, PieceType::Pawn) != 0
            {
                pos.ep = ep;
            }
        }

        pos.halfmove = parts.get(4).map_or(Ok(0), |s| s.parse()).map_err(|_| "bad halfmove clock")?;
        pos.fullmove = parts.get(5).map_or(Ok(1), |s| s.parse()).map_err(|_| "bad fullmove number")?;
        pos.fullmove = pos.fullmove.max(1);

        if pos.is_attacked(pos.king_sq(pos.side.flip()), pos.side, pos.occupied()) {
            return Err("side not to move is in check".into());
        }

        pos.hash = pos.compute_hash();
        Ok(pos)
    }

    pub fn to_fen(&self) -> String {
        let mut s = String::with_capacity(90);
        for rank in (0..8).rev() {
            let mut empty = 0;
            for file in 0..8 {
                match self.mailbox[make_sq(file, rank) as usize] {
                    Some(p) => {
                        if empty > 0 {
                            s.push((b'0' + empty) as char);
                            empty = 0;
                        }
                        s.push(p.to_char());
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                s.push((b'0' + empty) as char);
            }
            if rank > 0 {
                s.push('/');
            }
        }
        s.push(' ');
        s.push(if self.side == Color::White { 'w' } else { 'b' });
        s.push(' ');
        if self.castling == 0 {
            s.push('-');
        } else {
            for (bit, c) in [(WHITE_OO, 'K'), (WHITE_OOO, 'Q'), (BLACK_OO, 'k'), (BLACK_OOO, 'q')] {
                if self.castling & bit != 0 {
                    s.push(c);
                }
            }
        }
        s.push(' ');
        s.push_str(&self.ep_square().map_or("-".to_string(), sq_name));
        s.push_str(&format!(" {} {}", self.halfmove, self.fullmove));
        s
    }

    /// Hash recomputed from scratch (the incremental one must always agree).
    pub fn compute_hash(&self) -> u64 {
        let mut h = 0;
        for sq in 0..64u8 {
            if let Some(p) = self.mailbox[sq as usize] {
                h ^= ZOBRIST.pieces[p.idx()][sq as usize];
            }
        }
        h ^= ZOBRIST.castling[self.castling as usize];
        if self.ep != NO_SQ {
            h ^= ZOBRIST.ep[file_of(self.ep) as usize];
        }
        if self.side == Color::Black {
            h ^= ZOBRIST.side;
        }
        h
    }

    /// Piece-square totals recomputed from scratch.
    pub fn compute_psq(&self) -> (i32, i32, i32) {
        let (mut mg, mut eg, mut ph) = (0, 0, 0);
        for sq in 0..64usize {
            if let Some(p) = self.mailbox[sq] {
                mg += PSQ_MG[p.idx()][sq];
                eg += PSQ_EG[p.idx()][sq];
                ph += phase_inc(p.kind());
            }
        }
        (mg, eg, ph)
    }

    /// Colour-flipped mirror image (for evaluation symmetry tests).
    pub fn mirrored(&self) -> Position {
        let mut p = Position::empty();
        for sq in 0..64u8 {
            if let Some(pc) = self.mailbox[sq as usize] {
                p.put_piece(sq ^ 56, Piece::new(pc.color().flip(), pc.kind()));
            }
        }
        p.side = self.side.flip();
        p.castling = ((self.castling & 3) << 2) | (self.castling >> 2);
        p.ep = if self.ep == NO_SQ { NO_SQ } else { self.ep ^ 56 };
        p.halfmove = self.halfmove;
        p.fullmove = self.fullmove;
        p.hash = p.compute_hash();
        p
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for rank in (0..8).rev() {
            write!(f, "{} ", rank + 1)?;
            for file in 0..8 {
                let c = self.mailbox[make_sq(file, rank) as usize].map_or('.', Piece::to_char);
                write!(f, "{c} ")?;
            }
            writeln!(f)?;
        }
        writeln!(f, "  a b c d e f g h")?;
        write!(f, "{}", self.to_fen())
    }
}

impl fmt::Debug for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Position({})", self.to_fen())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fen_roundtrip() {
        for fen in [
            START_FEN,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
            "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3",
            "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P3/2NP1N2/PPP1QPPP/R4RK1 w - - 12 10",
        ] {
            let p = Position::from_fen(fen).unwrap();
            assert_eq!(p.to_fen(), fen);
            assert_eq!(p.hash(), p.compute_hash());
        }
    }

    #[test]
    fn uncapturable_ep_square_is_dropped() {
        let p = Position::from_fen("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq e3 0 1").unwrap();
        assert_eq!(p.ep_square(), None);
        assert_eq!(p, Position::from_fen("rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1").unwrap());
    }

    #[test]
    fn rejects_bad_fens() {
        for fen in [
            "",
            "8/8/8/8/8/8/8/8 w - - 0 1",                                // no kings
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBN w KQkq - 0 1",  // short rank
            "rnbqkbnr/pppppppp/9/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1", // bad digit
            "4k3/8/8/8/8/8/8/4K2R x - - 0 1",                           // bad side
            "4k2R/8/8/8/8/8/8/4K3 w - - 0 1",                           // opponent in check
            "P3k3/8/8/8/8/8/8/4K3 w - - 0 1",                           // pawn on 8th
        ] {
            assert!(Position::from_fen(fen).is_err(), "accepted {fen:?}");
        }
    }

    #[test]
    fn inconsistent_castling_rights_are_dropped() {
        let p = Position::from_fen("4k3/8/8/8/8/8/8/4K3 w KQkq - 0 1").unwrap();
        assert_eq!(p.castling_rights(), 0);
    }

    #[test]
    fn insufficient_material() {
        let cases = [
            ("4k3/8/8/8/8/8/8/4K3 w - - 0 1", true),
            ("4k3/8/8/8/8/8/8/3NK3 w - - 0 1", true),
            ("4k3/8/8/8/8/8/8/3BK3 w - - 0 1", true),
            ("4kb2/8/8/8/8/8/8/2B1K3 w - - 0 1", true), // both bishops on dark squares
            ("4k1b1/8/8/8/8/8/8/2B1K3 w - - 0 1", false), // opposite colors
            ("4k3/8/8/8/8/8/8/2NNK3 w - - 0 1", false),
            ("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1", false),
        ];
        for (fen, expect) in cases {
            assert_eq!(Position::from_fen(fen).unwrap().insufficient_material(), expect, "{fen}");
        }
    }

    #[test]
    fn mirror_is_involution() {
        let p = Position::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K1R1 w Qkq - 0 1").unwrap();
        assert_eq!(p.mirrored().mirrored(), p);
    }
}
