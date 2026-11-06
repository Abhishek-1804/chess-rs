//! Standard Algebraic Notation (SAN) and coordinate notation.

use crate::movegen::legal_moves;
use crate::position::Position;
use crate::types::*;

/// Format a legal move in SAN, including `+` / `#` suffixes.
pub fn to_san(pos: &Position, m: Move) -> String {
    let mut s = san_core(pos, m, Disambig::Minimal);
    let child = pos.make_move(m);
    if child.in_check() {
        s.push(if legal_moves(&child).is_empty() { '#' } else { '+' });
    }
    s
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Disambig {
    Minimal,
    File,
    Rank,
    Square,
    None,
}

fn san_core(pos: &Position, m: Move, how: Disambig) -> String {
    match m.flags() {
        flag::KING_CASTLE => return "O-O".into(),
        flag::QUEEN_CASTLE => return "O-O-O".into(),
        _ => {}
    }
    let from = m.from();
    let to = m.to();
    let pt = pos.piece_on(from).map_or(PieceType::Pawn, |p| p.kind());
    let mut s = String::with_capacity(8);
    if pt == PieceType::Pawn {
        if m.is_capture() {
            s.push((b'a' + file_of(from)) as char);
            s.push('x');
        }
    } else {
        s.push(pt.to_char().to_ascii_uppercase());
        let (need_file, need_rank) = match how {
            Disambig::Minimal => {
                let rivals: Vec<Square> = legal_moves(pos)
                    .iter()
                    .filter(|o| o.to() == to && o.from() != from)
                    .filter(|o| pos.piece_on(o.from()).map(|p| p.kind()) == Some(pt))
                    .map(|o| o.from())
                    .collect();
                if rivals.is_empty() {
                    (false, false)
                } else if rivals.iter().all(|&r| file_of(r) != file_of(from)) {
                    (true, false)
                } else if rivals.iter().all(|&r| rank_of(r) != rank_of(from)) {
                    (false, true)
                } else {
                    (true, true)
                }
            }
            Disambig::File => (true, false),
            Disambig::Rank => (false, true),
            Disambig::Square => (true, true),
            Disambig::None => (false, false),
        };
        if need_file {
            s.push((b'a' + file_of(from)) as char);
        }
        if need_rank {
            s.push((b'1' + rank_of(from)) as char);
        }
        if m.is_capture() {
            s.push('x');
        }
    }
    s.push_str(&sq_name(to));
    if let Some(p) = m.promotion() {
        s.push('=');
        s.push(p.to_char().to_ascii_uppercase());
    }
    s
}

/// Strip decorations so "Nxf3+", "N:f3" and "Nf3" compare equal.
fn normalize(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, 'x' | 'X' | ':' | '=' | '-' | '+' | '#' | '!' | '?')).collect()
}

/// Parse SAN (`Nf3`, `exd5`, `e8=Q`, `O-O`), long algebraic (`Ng1-f3`) or
/// coordinate notation (`g1f3`, `e7e8q`). Only legal moves are returned.
pub fn parse_move(pos: &Position, input: &str) -> Result<Move, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("empty move".into());
    }
    let moves = legal_moves(pos);
    let castle = raw.trim_end_matches(['+', '#', '!', '?']);
    let castle_flag = match castle {
        "O-O" | "0-0" | "o-o" => Some(flag::KING_CASTLE),
        "O-O-O" | "0-0-0" | "o-o-o" => Some(flag::QUEEN_CASTLE),
        _ => None,
    };
    if let Some(fl) = castle_flag {
        return moves.iter().copied().find(|m| m.flags() == fl).ok_or_else(|| format!("illegal move: {raw}"));
    }

    // Coordinate notation.
    let coord = normalize(raw).to_ascii_lowercase();
    let cb = coord.as_bytes();
    if (cb.len() == 4 || cb.len() == 5)
        && let (Some(from), Some(to)) = (parse_sq(&coord[0..2]), parse_sq(&coord[2..4]))
    {
        let promo = if cb.len() == 5 {
            match PieceType::from_char(cb[4] as char) {
                Some(p @ (PieceType::Knight | PieceType::Bishop | PieceType::Rook | PieceType::Queen)) => Some(p),
                _ => return Err(format!("bad promotion piece in {raw}")),
            }
        } else {
            None
        };
        let candidates: Vec<Move> = moves.iter().copied().filter(|m| m.from() == from && m.to() == to).collect();
        if candidates.is_empty() {
            return Err(format!("illegal move: {raw}"));
        }
        let want = promo.or(candidates[0].promotion().map(|_| PieceType::Queen));
        return candidates.into_iter().find(|m| m.promotion() == want).ok_or_else(|| format!("illegal move: {raw}"));
    }

    // SAN / long algebraic: compare against every rendering of each legal move.
    let mut target = normalize(raw);
    if let Some(first) = target.chars().next()
        && matches!(first, 'n' | 'r' | 'q' | 'k')
    {
        target.replace_range(0..1, &first.to_ascii_uppercase().to_string());
    }
    let promo_fix = {
        let mut t = target.clone();
        if let Some(last) = t.pop()
            && matches!(last, 'n' | 'b' | 'r' | 'q')
        {
            t.push(last.to_ascii_uppercase());
        }
        t
    };
    let mut exact = Vec::new();
    let mut loose = Vec::new();
    for &m in moves.iter() {
        for how in [Disambig::Minimal, Disambig::File, Disambig::Rank, Disambig::Square] {
            let form = normalize(&san_core(pos, m, how));
            if (form == target || form == promo_fix) && !exact.contains(&m) {
                exact.push(m);
            }
        }
        let form = normalize(&san_core(pos, m, Disambig::None));
        if (form == target || form == promo_fix) && !loose.contains(&m) {
            loose.push(m);
        }
        // Pawn promotion written without a piece: default to queen.
        if m.promotion() == Some(PieceType::Queen) {
            let form = normalize(&san_core(pos, m, Disambig::Minimal));
            if form[..form.len() - 1] == target && !exact.contains(&m) {
                exact.push(m);
            }
        }
    }
    match (exact.len(), loose.len()) {
        (1, _) => Ok(exact[0]),
        (0, 1) => Ok(loose[0]),
        (0, n) if n > 1 => Err(format!("ambiguous move: {raw}")),
        (n, _) if n > 1 => Err(format!("ambiguous move: {raw}")),
        _ => Err(format!("illegal or unrecognised move: {raw}")),
    }
}

/// SAN for a whole line of moves starting at `pos` (stops at the first illegal move).
pub fn line_to_san(pos: &Position, line: &[Move]) -> Vec<String> {
    let mut p = *pos;
    let mut out = Vec::with_capacity(line.len());
    for &m in line {
        if !legal_moves(&p).contains(&m) {
            break;
        }
        out.push(to_san(&p, m));
        p = p.make_move(m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(fen: &str) -> Position {
        Position::from_fen(fen).unwrap()
    }

    #[test]
    fn san_roundtrip_on_all_moves_of_tricky_positions() {
        for fen in [
            crate::position::START_FEN,
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
            "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
            "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
            "7k/8/8/8/1N3N2/8/1N3N2/K7 w - - 0 1", // four knights: full disambiguation
            "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3",
        ] {
            let p = pos(fen);
            for &m in legal_moves(&p).iter() {
                let san = to_san(&p, m);
                assert_eq!(parse_move(&p, &san), Ok(m), "{fen}: {san}");
                assert_eq!(parse_move(&p, &m.to_uci()), Ok(m), "{fen}: {}", m.to_uci());
            }
        }
    }

    #[test]
    fn san_formatting() {
        let p = Position::startpos();
        assert_eq!(to_san(&p, parse_move(&p, "g1f3").unwrap()), "Nf3");
        let p = pos("7k/8/8/8/1N3N2/8/1N3N2/K7 w - - 0 1");
        assert_eq!(to_san(&p, parse_move(&p, "b2d3").unwrap()), "Nb2d3");
        assert_eq!(to_san(&p, parse_move(&p, "b4a6").unwrap()), "Na6");
        let p = pos("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1");
        assert_eq!(to_san(&p, parse_move(&p, "d1d8").unwrap()), "Rd8#");
        let p = pos("r3k3/1P6/8/8/8/8/8/4K3 w q - 0 1");
        assert_eq!(to_san(&p, parse_move(&p, "b7a8n").unwrap()), "bxa8=N");
        assert_eq!(to_san(&p, parse_move(&p, "b7b8q").unwrap()), "b8=Q+");
        let p = pos("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
        assert_eq!(to_san(&p, parse_move(&p, "e1g1").unwrap()), "O-O");
        assert_eq!(to_san(&p, parse_move(&p, "e1c1").unwrap()), "O-O-O");
    }

    #[test]
    fn lenient_parsing() {
        let p = Position::startpos();
        for s in ["Nf3", "nf3", "Ng1f3", "Ng1-f3", "g1f3", "g1-f3", "Nf3!?"] {
            assert_eq!(parse_move(&p, s).unwrap().to_uci(), "g1f3", "{s}");
        }
        let p = pos("4k3/1P6/8/8/8/8/8/4K3 w - - 0 1");
        assert_eq!(parse_move(&p, "b8").unwrap().promotion(), Some(PieceType::Queen));
        assert_eq!(parse_move(&p, "b8n").unwrap().promotion(), Some(PieceType::Knight));
        assert_eq!(parse_move(&p, "b7b8").unwrap().promotion(), Some(PieceType::Queen));
        assert_eq!(parse_move(&p, "b7b8r").unwrap().promotion(), Some(PieceType::Rook));
    }

    #[test]
    fn rejects_illegal_and_ambiguous() {
        let p = Position::startpos();
        assert!(parse_move(&p, "e5").is_err());
        assert!(parse_move(&p, "e2e5").is_err());
        assert!(parse_move(&p, "O-O").is_err());
        assert!(parse_move(&p, "").is_err());
        assert!(parse_move(&p, "hello").is_err());
        let p = pos("7k/8/8/8/1N3N2/8/1N3N2/K7 w - - 0 1");
        assert!(parse_move(&p, "Nd3").unwrap_err().contains("ambiguous"));
        assert!(parse_move(&p, "Nbd3").unwrap_err().contains("ambiguous"));
    }
}
