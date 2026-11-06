//! PGN reading/writing against famous historical games, and game analysis.

use std::sync::atomic::AtomicBool;

use chess_engine::analysis::{MoveClass, analyze, eval_from_pgn, eval_to_pgn};
use chess_engine::pgn::{comment_command, parse_one, parse_pgn};
use chess_engine::{Color, Engine, GameResult};

fn famous() -> Vec<chess_engine::pgn::PgnGame> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/famous_games.pgn")).unwrap();
    parse_pgn(&text).unwrap()
}

#[test]
fn famous_games_parse_and_end_in_mate() {
    let games = famous();
    assert_eq!(games.len(), 2);
    for (g, plies, last) in [(&games[0], 45, "Be7#"), (&games[1], 33, "Rd8#")] {
        assert_eq!(g.moves.len(), plies, "{:?}", g.tag("Annotation"));
        assert_eq!(g.result, "1-0");
        let game = g.to_game();
        assert_eq!(game.result(), Some(GameResult::Checkmate { winner: Color::White }));
        assert_eq!(game.san_moves().last().unwrap(), last);
    }
    assert_eq!(games[1].tag("White"), Some("Paul Morphy"));
}

#[test]
fn famous_games_roundtrip_through_our_writer() {
    for g in famous() {
        let text = g.to_pgn();
        let back = parse_one(&text).unwrap();
        assert_eq!(back.moves, g.moves);
        assert_eq!(back.tags, g.tags);
        assert_eq!(back.result, g.result);
        // Writing again is byte-identical (stable output).
        assert_eq!(back.to_pgn(), text);
    }
}

#[test]
fn opera_game_review() {
    let g = famous().remove(1).to_game();
    // One thread: Lazy SMP is non-deterministic, tests must not be. Depth 10 is
    // the shallowest depth that sees Black's errors (measured: 95.9 vs 88.1).
    let review = analyze(&Engine::new(32, 1), &g, 10, &AtomicBool::new(false), |_, _, _| {}).unwrap();
    assert_eq!(review.moves.len(), 33);
    // Morphy's finish: Qb8+ forces mate (best), Rd8# delivers it.
    let qb8 = &review.moves[30];
    assert!(matches!(qb8.class, MoveClass::Best | MoveClass::Great | MoveClass::Brilliant), "{:?}", qb8.class);
    // Black's only legal reply to Qb8+ is Nxb8: forced.
    assert_eq!(review.moves[31].class, MoveClass::Forced);
    assert!(review.accuracy[0] > review.accuracy[1] + 5.0, "Morphy clearly outplayed the Duke: {:?}", review.accuracy);
    // Black's play contains a serious error, not just inaccuracies.
    assert!(
        review
            .moves
            .iter()
            .any(|m| m.mover == Color::Black && matches!(m.class, MoveClass::Mistake | MoveClass::Blunder)),
        "{:?}",
        review.moves.iter().map(|m| m.class).collect::<Vec<_>>()
    );
}

#[test]
fn analysis_survives_pgn_comments() {
    // The [%eval] encoding used to store reviews round-trips through PGN.
    let mut g = famous().remove(1);
    for (i, c) in g.comments.iter_mut().enumerate() {
        *c = format!("[%eval {}] [%class Best]", eval_to_pgn(i as i32 * 7 - 50));
    }
    let back = parse_one(&g.to_pgn()).unwrap();
    for (i, c) in back.comments.iter().enumerate() {
        assert_eq!(eval_from_pgn(comment_command(c, "eval").unwrap()), Some(i as i32 * 7 - 50));
        assert_eq!(comment_command(c, "class"), Some("Best"));
    }
}
