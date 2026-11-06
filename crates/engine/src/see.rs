//! Static exchange evaluation (swap algorithm with x-ray discovery).

use crate::attacks::{bishop_attacks, rook_attacks};
use crate::eval::SEE_VALUE;
use crate::position::Position;
use crate::types::*;

/// Material balance (for the mover) of the capture sequence started by `m`
/// on its destination square, assuming both sides recapture optimally.
pub fn see(pos: &Position, m: Move) -> i32 {
    let from = m.from();
    let to = m.to();
    let Some(mover) = pos.piece_on(from) else { return 0 };
    let mut attacker = mover.kind();
    let mut gain = [0i32; 34];
    let mut occ = pos.occupied();

    gain[0] = if m.is_ep() {
        occ ^= bb(to ^ 8);
        SEE_VALUE[PieceType::Pawn.idx()]
    } else {
        pos.piece_on(to).map_or(0, |p| SEE_VALUE[p.kind().idx()])
    };
    if let Some(promo) = m.promotion() {
        gain[0] += SEE_VALUE[promo.idx()] - SEE_VALUE[PieceType::Pawn.idx()];
        attacker = promo;
    }

    let bq = pos.pieces_of_type(PieceType::Bishop) | pos.pieces_of_type(PieceType::Queen);
    let rq = pos.pieces_of_type(PieceType::Rook) | pos.pieces_of_type(PieceType::Queen);
    let mut attackers = pos.attackers_to(to, occ);
    let mut from_bb = bb(from);
    let mut side = mover.color().flip();
    let mut d = 0;

    loop {
        d += 1;
        gain[d] = SEE_VALUE[attacker.idx()] - gain[d - 1];
        if (-gain[d - 1]).max(gain[d]) < 0 || d >= 32 {
            break;
        }
        occ ^= from_bb;
        attackers = (attackers | (bishop_attacks(to, occ) & bq) | (rook_attacks(to, occ) & rq)) & occ;
        let ours = attackers & pos.color_bb(side);
        if ours == 0 {
            break;
        }
        let Some(pt) = PieceType::ALL.into_iter().find(|&pt| ours & pos.pieces_of_type(pt) != 0) else { break };
        let set = ours & pos.pieces_of_type(pt);
        from_bb = set.isolate_lowest_one();
        attacker = pt;
        side = side.flip();
    }
    while d > 1 {
        d -= 1;
        gain[d - 1] = -(-gain[d - 1]).max(gain[d]);
    }
    gain[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::parse_move;

    fn see_of(fen: &str, mv: &str) -> i32 {
        let pos = Position::from_fen(fen).unwrap();
        let m = parse_move(&pos, mv).unwrap();
        see(&pos, m)
    }

    #[test]
    fn see_values() {
        // Free pawn.
        assert_eq!(see_of("1k1r4/1pp4p/p7/4p3/8/P5P1/1PP4P/2K1R3 w - - 0 1", "Rxe5"), 100);
        // Pawn defended by pawn: knight takes, pawn recaptures.
        assert_eq!(see_of("4k3/8/3p4/4p3/8/5N2/8/4K3 w - - 0 1", "Nxe5"), 100 - 320);
        // Doubled rooks vs. doubled queens: black declines to recapture (x-ray).
        assert_eq!(see_of("3qk3/8/8/3q4/8/8/3R4/3RK3 w - - 0 1", "Rxd5"), 900);
        // Rook takes a queen defended by a rook, second rook x-rays behind.
        assert_eq!(see_of("3rk3/8/8/3q4/8/8/3R4/3RK3 w - - 0 1", "Rxd5"), 900);
        // Rook takes a pawn defended by a pawn: loses the exchange.
        assert_eq!(see_of("4k3/8/2p5/3p4/8/8/8/3RK3 w - - 0 1", "Rxd5"), 100 - 500);
        // Quiet moves: safe square vs. square attacked by a pawn.
        assert_eq!(see_of("4k3/8/3p4/8/8/8/8/4K1N1 w - - 0 1", "Nf3"), 0);
        assert_eq!(see_of("4k3/8/8/8/3p4/8/8/4KN2 w - - 0 1", "Ne3"), -320);
    }
}
