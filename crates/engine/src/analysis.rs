//! Game review: evaluate every position of a game, then rate each move and
//! each player the way chess.com's "Game Review" / lichess's analysis do.
//!
//! * Scores are turned into **win percentages** with lichess's published
//!   logistic model, so a 300cp slip in a dead-won position matters far less
//!   than in an equal one.
//! * **Move accuracy** uses lichess's formula on the win-percentage drop, and
//!   a player's **game accuracy** blends the arithmetic and harmonic means of
//!   their move accuracies (one blunder pulls it down hard, as it should).
//! * **Classification** follows chess.com's expected-points bands
//!   (Best / Excellent / Good / Inaccuracy / Mistake / Blunder), plus
//!   *Great* (the only good move), *Brilliant* (a sound piece sacrifice),
//!   *Miss* (failing to punish the opponent's error) and *Forced*.
//!
//! [`review`] is a pure function of the evaluations, so it is fully testable;
//! [`analyze`] runs the engine to produce those evaluations.

use std::sync::atomic::AtomicBool;

use crate::eval::SEE_VALUE;
use crate::game::Game;
use crate::movegen::{legal_moves, noisy_moves};
use crate::position::Position;
use crate::search::{Engine, Limits, MATE, mate_in};
use crate::see::see;
use crate::types::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MoveClass {
    Brilliant,
    Great,
    Best,
    Excellent,
    Good,
    Inaccuracy,
    Mistake,
    Miss,
    Blunder,
    Forced,
}

impl MoveClass {
    pub const ALL: [MoveClass; 10] = [
        MoveClass::Brilliant,
        MoveClass::Great,
        MoveClass::Best,
        MoveClass::Excellent,
        MoveClass::Good,
        MoveClass::Inaccuracy,
        MoveClass::Mistake,
        MoveClass::Miss,
        MoveClass::Blunder,
        MoveClass::Forced,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MoveClass::Brilliant => "Brilliant",
            MoveClass::Great => "Great",
            MoveClass::Best => "Best",
            MoveClass::Excellent => "Excellent",
            MoveClass::Good => "Good",
            MoveClass::Inaccuracy => "Inaccuracy",
            MoveClass::Mistake => "Mistake",
            MoveClass::Miss => "Miss",
            MoveClass::Blunder => "Blunder",
            MoveClass::Forced => "Forced",
        }
    }

    /// Short glyph shown next to the move (ASCII so every font has it).
    pub fn symbol(self) -> &'static str {
        match self {
            MoveClass::Brilliant => "!!",
            MoveClass::Great => "!",
            MoveClass::Best => "*",
            MoveClass::Excellent => "+",
            MoveClass::Good => "=",
            MoveClass::Inaccuracy => "?!",
            MoveClass::Mistake => "?",
            MoveClass::Miss => "x",
            MoveClass::Blunder => "??",
            MoveClass::Forced => "-",
        }
    }

    /// Standard PGN NAG for the class (0 = none).
    pub fn nag(self) -> u8 {
        match self {
            MoveClass::Brilliant => 3,
            MoveClass::Great => 1,
            MoveClass::Inaccuracy => 6,
            MoveClass::Mistake | MoveClass::Miss => 2,
            MoveClass::Blunder => 4,
            _ => 0,
        }
    }

    /// Bad moves (shown with an explanation of what was better).
    pub fn is_error(self) -> bool {
        matches!(self, MoveClass::Inaccuracy | MoveClass::Mistake | MoveClass::Miss | MoveClass::Blunder)
    }
}

/// Engine verdict on one position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionEval {
    /// White-relative score (centipawns; mate scores as in the search).
    pub score: i32,
    /// Best move and line from this position (empty if the game is over).
    pub best: Option<Move>,
    pub pv: Vec<Move>,
    /// White-relative score of the best move *other than* `best`, when known.
    /// Used to detect "only move" (Great) situations.
    pub alt_score: Option<i32>,
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MoveReview {
    pub ply: usize,
    pub mv: Move,
    pub mover: Color,
    pub class: MoveClass,
    /// Win% for the mover before and after the move (0..100).
    pub win_before: f64,
    pub win_after: f64,
    pub accuracy: f64,
    /// Engine's preferred move in the position before this one.
    pub best: Option<Move>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameReview {
    /// One entry per position: index 0 = start, index i = after ply i.
    pub evals: Vec<PositionEval>,
    pub moves: Vec<MoveReview>,
    /// Game accuracy per colour (indexed by `Color::idx`), 0..100.
    pub accuracy: [f64; 2],
    pub depth: u32,
}

impl GameReview {
    pub fn count(&self, color: Color, class: MoveClass) -> usize {
        self.moves.iter().filter(|m| m.mover == color && m.class == class).count()
    }

    /// White's win% at each position, for the evaluation graph.
    pub fn white_win_curve(&self) -> Vec<f64> {
        self.evals.iter().map(|e| win_percent(e.score)).collect()
    }
}

/// Score -> centipawns for the win model (mates become +-10000).
fn model_cp(score: i32) -> f64 {
    match mate_in(score) {
        Some(n) if n > 0 => 10_000.0,
        Some(_) => -10_000.0,
        None => score as f64,
    }
}

/// Winning chances (0..100) for the side whose score this is (lichess model).
pub fn win_percent(score: i32) -> f64 {
    let cp = model_cp(score).clamp(-1000.0, 1000.0);
    50.0 + 50.0 * (2.0 / (1.0 + (-0.003_682_08 * cp).exp()) - 1.0)
}

/// Accuracy (0..100) of a move that changed the mover's win% from `before` to `after`.
pub fn move_accuracy(before: f64, after: f64) -> f64 {
    let drop = (before - after).max(0.0);
    (103.166_810_071_164_9 * (-0.043_544_153_867_539_51 * drop).exp() - 3.166_924_740_191_411 + 1.0).clamp(0.0, 100.0)
}

/// Game accuracy from move accuracies: mean of the arithmetic and harmonic
/// means (the harmonic mean is dominated by the worst moves).
pub fn game_accuracy(moves: &[f64]) -> f64 {
    if moves.is_empty() {
        return 100.0;
    }
    let n = moves.len() as f64;
    let mean = moves.iter().sum::<f64>() / n;
    let harmonic = n / moves.iter().map(|a| 1.0 / a.max(1.0)).sum::<f64>();
    ((mean + harmonic) / 2.0).clamp(0.0, 100.0)
}

/// Did `m` leave a non-pawn piece en prise or give material away by SEE?
pub fn is_sacrifice(pos: &Position, m: Move) -> bool {
    let Some(piece) = pos.piece_on(m.from()) else { return false };
    if piece.kind() == PieceType::King {
        return false;
    }
    // Capturing into a losing exchange with a real piece.
    if piece.kind() != PieceType::Pawn && see(pos, m) < 0 {
        return true;
    }
    // Leaving a piece (worth at least a minor) capturable at a profit.
    let child = pos.make_move(m);
    noisy_moves(&child).iter().any(|&reply| {
        reply.is_capture()
            && !reply.is_ep()
            && child.piece_on(reply.to()).is_some_and(|p| {
                p.color() == piece.color() && SEE_VALUE[p.kind().idx()] >= SEE_VALUE[PieceType::Knight.idx()]
            })
            && see(&child, reply) > 0
    })
}

/// Rate every move from the per-position evaluations (`evals.len() == moves.len() + 1`).
pub fn review(start: &Position, moves: &[Move], evals: &[PositionEval], depth: u32) -> GameReview {
    assert_eq!(evals.len(), moves.len() + 1, "one evaluation per position");
    let mut pos = *start;
    let mut out: Vec<MoveReview> = Vec::with_capacity(moves.len());
    let mut per_color: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    for (i, &mv) in moves.iter().enumerate() {
        let mover = pos.side_to_move();
        let sign = if mover == Color::White { 1 } else { -1 };
        let before = &evals[i];
        let after = &evals[i + 1];
        let win_before = win_percent(sign * before.score);
        let played_best = before.best == Some(mv);
        // A move that matches the engine's choice cannot have lost anything;
        // this removes noise between depth d (before) and d-1 (after) searches.
        let win_after =
            if played_best { win_before.max(win_percent(sign * after.score)) } else { win_percent(sign * after.score) };
        let loss = (win_before - win_after).max(0.0);
        let accuracy = if played_best { 100.0 } else { move_accuracy(win_before, win_after) };

        let only_move = legal_moves(&pos).len() == 1;
        let prev_was_error = out.last().is_some_and(|p| matches!(p.class, MoveClass::Mistake | MoveClass::Blunder));
        let class = if only_move {
            MoveClass::Forced
        } else if played_best || loss < 2.0 {
            let alt_gap = before.alt_score.map(|alt| win_before - win_percent(sign * alt));
            if is_sacrifice(&pos, mv) && win_after >= 50.0 && win_before < 95.0 {
                MoveClass::Brilliant
            } else if played_best && alt_gap.is_some_and(|g| g >= 15.0) && win_after >= 40.0 {
                MoveClass::Great
            } else if played_best {
                MoveClass::Best
            } else {
                MoveClass::Excellent
            }
        } else if loss < 5.0 {
            MoveClass::Good
        } else if prev_was_error && win_before >= 60.0 && loss >= 10.0 {
            MoveClass::Miss
        } else if loss < 10.0 {
            MoveClass::Inaccuracy
        } else if loss < 20.0 {
            MoveClass::Mistake
        } else {
            MoveClass::Blunder
        };
        if class != MoveClass::Forced {
            per_color[mover.idx()].push(accuracy);
        }
        out.push(MoveReview { ply: i + 1, mv, mover, class, win_before, win_after, accuracy, best: before.best });
        pos = pos.make_move(mv);
    }
    GameReview {
        evals: evals.to_vec(),
        moves: out,
        accuracy: [game_accuracy(&per_color[0]), game_accuracy(&per_color[1])],
        depth,
    }
}

/// Exact verdict for a finished position, if the game is over there.
fn terminal_eval(game: &Game) -> Option<PositionEval> {
    let pos = game.position();
    let over = legal_moves(pos).is_empty()
        || pos.insufficient_material()
        || pos.halfmove_clock() >= 100
        || game.repetition_count() >= 3;
    if !over {
        return None;
    }
    let score = if legal_moves(pos).is_empty() && pos.in_check() {
        if pos.side_to_move() == Color::White { -MATE } else { MATE }
    } else {
        0
    };
    Some(PositionEval { score, best: None, pv: Vec::new(), alt_score: None, depth: 0 })
}

/// Progress of [`analyze`]: (positions done, total positions).
pub type Progress = (usize, usize);

/// Evaluate every position of `game` at `depth`, then find the best
/// alternative where the best move was played (for "Great" moves).
/// Returns `None` if `stop` was raised.
pub fn analyze(
    engine: &Engine,
    game: &Game,
    depth: u32,
    stop: &AtomicBool,
    mut on_progress: impl FnMut(Progress, usize, &PositionEval),
) -> Option<GameReview> {
    let moves = game.moves();
    let total = moves.len() + 1;
    let mut replay = Game::from_position(*game.start_position());
    let mut evals = Vec::with_capacity(total);
    for i in 0..total {
        let eval = match terminal_eval(&replay) {
            Some(e) => e,
            None => {
                let pos = *replay.position();
                let r = engine.search(&pos, &replay.history_hashes(), &Limits::depth(depth), stop, |_| {});
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    return None;
                }
                let white = if pos.side_to_move() == Color::White { r.score } else { -r.score };
                PositionEval { score: white, best: r.best_move, pv: r.pv, alt_score: None, depth: r.depth }
            }
        };
        on_progress((i + 1, total), i, &eval);
        evals.push(eval);
        if i < moves.len() {
            replay.play(moves[i]).ok()?;
        }
    }

    // Second pass: how good was the best alternative, where the best move was played?
    let mut replay = Game::from_position(*game.start_position());
    for (i, &mv) in moves.iter().enumerate() {
        let pos = *replay.position();
        if evals[i].best == Some(mv) && legal_moves(&pos).len() > 1 {
            let limits = Limits { excluded: vec![mv], ..Limits::depth(depth.saturating_sub(2).max(1)) };
            let r = engine.search(&pos, &replay.history_hashes(), &limits, stop, |_| {});
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            evals[i].alt_score = Some(if pos.side_to_move() == Color::White { r.score } else { -r.score });
        }
        replay.play(mv).ok()?;
    }
    Some(review(game.start_position(), moves, &evals, depth))
}

/// Format a white-relative score the way PGN `[%eval]` does: `0.34`, `#3`, `#-2`.
pub fn eval_to_pgn(score: i32) -> String {
    match mate_in(score) {
        Some(n) => format!("#{n}"),
        None => format!("{:.2}", score as f64 / 100.0),
    }
}

/// Parse a PGN `[%eval]` value back into a white-relative score.
pub fn eval_from_pgn(s: &str) -> Option<i32> {
    if let Some(m) = s.strip_prefix('#') {
        let n: i32 = m.parse().ok()?;
        return Some(if n > 0 { MATE - (2 * n - 1) } else { -(MATE - 2 * -n) });
    }
    let pawns: f64 = s.parse().ok()?;
    Some((pawns * 100.0).round() as i32)
}

/// Store a review inside a PGN: per-move `[%eval] [%best] [%alt]` comments,
/// class NAGs, and accuracy/depth tags. Readable by lichess and other tools.
pub fn annotate(pgn: &mut crate::pgn::PgnGame, review: &GameReview) {
    let fmt_pos = |e: &PositionEval| {
        let mut c = format!("[%eval {}]", eval_to_pgn(e.score));
        if let Some(b) = e.best {
            c.push_str(&format!(" [%best {}]", b.to_uci()));
        }
        if let Some(a) = e.alt_score {
            c.push_str(&format!(" [%alt {}]", eval_to_pgn(a)));
        }
        c
    };
    pgn.comments = review.evals[1..].iter().map(fmt_pos).collect();
    pgn.nags = review.moves.iter().map(|m| m.class.nag()).collect();
    pgn.pre_comment = fmt_pos(&review.evals[0]);
    pgn.set_tag("ReviewDepth", review.depth.to_string());
    pgn.set_tag("WhiteAccuracy", format!("{:.1}", review.accuracy[0]));
    pgn.set_tag("BlackAccuracy", format!("{:.1}", review.accuracy[1]));
    pgn.set_tag("Annotator", format!("chess-rs (depth {})", review.depth));
}

/// Rebuild a review stored by [`annotate`]; `None` if the PGN has none.
pub fn review_from_pgn(pgn: &crate::pgn::PgnGame) -> Option<GameReview> {
    use crate::pgn::comment_command;
    let depth: u32 = pgn.tag("ReviewDepth")?.parse().ok()?;
    let mut pos = pgn.start;
    let mut evals = Vec::with_capacity(pgn.moves.len() + 1);
    let comments = std::iter::once(&pgn.pre_comment).chain(pgn.comments.iter());
    for (i, c) in comments.enumerate() {
        let score = eval_from_pgn(comment_command(c, "eval")?)?;
        let best = match comment_command(c, "best") {
            Some(uci) => Some(crate::notation::parse_move(&pos, uci).ok()?),
            None => None,
        };
        let alt_score = comment_command(c, "alt").and_then(eval_from_pgn);
        evals.push(PositionEval { score, best, pv: best.into_iter().collect(), alt_score, depth });
        if let Some(&m) = pgn.moves.get(i) {
            pos = pos.make_move(m);
        }
    }
    (evals.len() == pgn.moves.len() + 1).then(|| review(&pgn.start, &pgn.moves, &evals, depth))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::parse_move;

    #[test]
    fn win_model_matches_lichess() {
        assert!((win_percent(0) - 50.0).abs() < 1e-9);
        // lichess: +100cp ~ 59.1%, +300cp ~ 75.2%.
        assert!((win_percent(100) - 59.1).abs() < 0.1, "{}", win_percent(100));
        assert!((win_percent(300) - 75.2).abs() < 0.1, "{}", win_percent(300));
        assert!((win_percent(-300) - 24.8).abs() < 0.1);
        assert!(win_percent(MATE - 3) > 97.0 && win_percent(-MATE + 2) < 3.0);
        for cp in [-800, -100, 0, 50, 400] {
            assert!(win_percent(cp) < win_percent(cp + 10), "monotonic");
        }
    }

    #[test]
    fn accuracy_formulas() {
        assert_eq!(move_accuracy(60.0, 60.0), 100.0);
        assert_eq!(move_accuracy(60.0, 70.0), 100.0); // gaining is not penalised
        assert!(move_accuracy(60.0, 50.0) < 70.0);
        assert!(move_accuracy(90.0, 10.0) < 5.0);
        assert_eq!(game_accuracy(&[]), 100.0);
        assert!((game_accuracy(&[100.0, 100.0]) - 100.0).abs() < 1e-9);
        // One bad move drags accuracy well below the arithmetic mean.
        let a = game_accuracy(&[100.0, 100.0, 100.0, 10.0]);
        assert!(a < 77.5 && a > 40.0, "{a}");
    }

    #[test]
    fn eval_pgn_roundtrip() {
        for s in [0, 34, -125, 900, MATE - 1, MATE - 5, -(MATE - 2), -(MATE - 6)] {
            assert_eq!(eval_from_pgn(&eval_to_pgn(s)), Some(s), "{s} -> {}", eval_to_pgn(s));
        }
        assert_eq!(eval_to_pgn(34), "0.34");
        assert_eq!(eval_to_pgn(MATE - 1), "#1");
        assert_eq!(eval_to_pgn(-(MATE - 4)), "#-2");
        assert_eq!(eval_from_pgn("junk"), None);
    }

    fn ev(score: i32, best: Option<Move>) -> PositionEval {
        PositionEval { score, best, pv: best.into_iter().collect(), alt_score: None, depth: 10 }
    }

    #[test]
    fn classification_bands() {
        let start = Position::startpos();
        let e4 = parse_move(&start, "e4").unwrap();
        let a3 = parse_move(&start, "a3").unwrap();
        let cases = [
            (e4, ev(30, Some(e4)), ev(30, None), MoveClass::Best),
            (a3, ev(30, Some(e4)), ev(25, None), MoveClass::Excellent),
            (a3, ev(30, Some(e4)), ev(-10, None), MoveClass::Good),
            (a3, ev(30, Some(e4)), ev(-60, None), MoveClass::Inaccuracy),
            (a3, ev(30, Some(e4)), ev(-150, None), MoveClass::Mistake),
            (a3, ev(30, Some(e4)), ev(-600, None), MoveClass::Blunder),
        ];
        for (mv, before, after, want) in cases {
            let r = review(&start, &[mv], &[before, after], 10);
            assert_eq!(r.moves[0].class, want, "after score {}", r.evals[1].score);
        }
    }

    #[test]
    fn black_moves_are_scored_from_blacks_side() {
        let start = Position::startpos();
        let e4 = parse_move(&start, "e4").unwrap();
        let after_e4 = start.make_move(e4);
        let e5 = parse_move(&after_e4, "e5").unwrap();
        let f6 = parse_move(&after_e4, "f6").unwrap();
        // White-relative: black's f6 lets the score go from +30 to +250.
        let r = review(&start, &[e4, f6], &[ev(30, Some(e4)), ev(30, Some(e5)), ev(250, None)], 10);
        assert_eq!(r.moves[1].mover, Color::Black);
        assert_eq!(r.moves[1].class, MoveClass::Mistake);
        assert!(r.accuracy[Color::Black.idx()] < r.accuracy[Color::White.idx()]);
    }

    #[test]
    fn forced_great_brilliant_and_miss() {
        // Forced: only one legal move (king in the corner, single escape).
        let pos = Position::from_fen("7k/8/8/8/8/8/1q6/K7 w - - 0 1").unwrap();
        assert_eq!(legal_moves(&pos).len(), 1);
        let only = legal_moves(&pos)[0];
        let r = review(&pos, &[only], &[ev(-900, Some(only)), ev(-900, None)], 10);
        assert_eq!(r.moves[0].class, MoveClass::Forced);

        // Great: the best move, and every alternative is much worse.
        let start = Position::startpos();
        let e4 = parse_move(&start, "e4").unwrap();
        let before = PositionEval { alt_score: Some(-400), ..ev(30, Some(e4)) };
        let r = review(&start, &[e4], &[before, ev(30, None)], 10);
        assert_eq!(r.moves[0].class, MoveClass::Great);

        // Brilliant: a sound piece sacrifice (Bxh7+ leaving the bishop to be taken).
        let pos = Position::from_fen("r1bq1rk1/pppn1ppp/4p3/3pP3/1b1P4/2NB1N2/PPP2PPP/R1BQK2R w KQ - 0 8").unwrap();
        let bxh7 = parse_move(&pos, "Bxh7+").unwrap();
        assert!(is_sacrifice(&pos, bxh7));
        let r = review(&pos, &[bxh7], &[ev(150, Some(bxh7)), ev(150, None)], 10);
        assert_eq!(r.moves[0].class, MoveClass::Brilliant);
        // Quiet developing moves are not sacrifices.
        assert!(!is_sacrifice(&start, e4));

        // Miss: opponent blundered, and we failed to punish it.
        let start = Position::startpos();
        let e4 = parse_move(&start, "e4").unwrap();
        let p1 = start.make_move(e4);
        let f6 = parse_move(&p1, "f6").unwrap();
        let p2 = p1.make_move(f6);
        let qh5 = parse_move(&p2, "Qh5+").unwrap();
        let a3 = parse_move(&p2, "a3").unwrap();
        let evals = [ev(30, Some(e4)), ev(30, None), ev(600, Some(qh5)), ev(30, None)];
        let r = review(&start, &[e4, f6, a3], &evals, 10);
        assert_eq!(r.moves[1].class, MoveClass::Blunder);
        assert_eq!(r.moves[2].class, MoveClass::Miss);
    }

    #[test]
    fn analyze_finds_the_blunder_in_a_real_game() {
        // 1.e4 e5 2.Qh5 Nc6 3.Bc4 Nf6?? 4.Qxf7#
        let mut g = Game::new();
        for s in ["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7#"] {
            g.play(parse_move(g.position(), s).unwrap()).unwrap();
        }
        let engine = Engine::new(16, 1);
        let mut seen = Vec::new();
        let r = analyze(&engine, &g, 6, &AtomicBool::new(false), |p, _, _| seen.push(p)).unwrap();
        assert_eq!(seen.last(), Some(&(8, 8)));
        assert_eq!(r.evals.len(), 8);
        assert_eq!(r.moves[5].class, MoveClass::Blunder, "Nf6?? allows mate");
        let nf6 = r.moves[5].mv;
        assert!(r.moves[5].best.is_some_and(|b| b != nf6), "engine prefers a defence of f7");
        assert!(matches!(r.moves[6].class, MoveClass::Best | MoveClass::Great), "{:?}", r.moves[6].class);
        // Final position: Black is checkmated.
        assert_eq!(r.evals[7].score, MATE);
        assert_eq!(r.evals[7].best, None);
        assert!(r.accuracy[Color::White.idx()] > r.accuracy[Color::Black.idx()]);
    }

    #[test]
    fn review_survives_a_pgn_roundtrip() {
        let mut g = Game::new();
        for s in ["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7#"] {
            g.play(parse_move(g.position(), s).unwrap()).unwrap();
        }
        let review = analyze(&Engine::new(16, 1), &g, 5, &AtomicBool::new(false), |_, _, _| {}).unwrap();
        let mut pgn = crate::pgn::PgnGame::from_game(&g);
        assert_eq!(review_from_pgn(&pgn), None, "unannotated game has no review");
        annotate(&mut pgn, &review);
        let text = pgn.to_pgn();
        assert!(text.contains("[%eval"), "{text}");
        assert!(text.contains("Nf6 $4"), "blunder NAG written: {text}");
        let back = review_from_pgn(&crate::pgn::parse_one(&text).unwrap()).unwrap();
        let classes = |r: &GameReview| r.moves.iter().map(|m| m.class).collect::<Vec<_>>();
        assert_eq!(classes(&back), classes(&review));
        assert_eq!(back.accuracy, review.accuracy);
        assert_eq!(back.white_win_curve(), review.white_win_curve());
    }

    #[test]
    fn analyze_can_be_stopped() {
        let g = Game::new();
        let stop = AtomicBool::new(true);
        assert!(analyze(&Engine::new(1, 1), &g, 5, &stop, |_, _, _| {}).is_none());
    }
}
