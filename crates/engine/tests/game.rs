//! Game-level rules: results, repetition, fifty-move rule, undo, PGN.

use chess_engine::{Color, Game, GameResult, START_FEN, parse_move};

fn play(game: &mut Game, moves: &str) {
    for s in moves.split_whitespace() {
        let m = parse_move(game.position(), s).unwrap_or_else(|e| panic!("{s}: {e}"));
        game.play(m).unwrap();
    }
}

#[test]
fn fools_mate() {
    let mut g = Game::new();
    play(&mut g, "f3 e5 g4 Qh4#");
    assert_eq!(g.result(), Some(GameResult::Checkmate { winner: Color::Black }));
    assert_eq!(g.san_moves().last().unwrap(), "Qh4#");
    // No further moves allowed.
    let any = g.legal_moves();
    assert!(any.is_empty());
}

#[test]
fn stalemate() {
    let mut g = Game::from_fen("7k/5K2/8/8/8/8/8/6Q1 w - - 0 1").unwrap();
    play(&mut g, "Qg6");
    assert_eq!(g.result(), Some(GameResult::Stalemate));
}

#[test]
fn threefold_repetition() {
    let mut g = Game::new();
    play(&mut g, "Nf3 Nf6 Ng1 Ng8 Nf3 Nf6 Ng1");
    assert_eq!(g.result(), None);
    assert_eq!(g.repetition_count(), 2);
    play(&mut g, "Ng8");
    assert_eq!(g.repetition_count(), 3);
    assert_eq!(g.result(), Some(GameResult::ThreefoldRepetition));
}

#[test]
fn fifty_move_rule() {
    let mut g = Game::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 99 80").unwrap();
    assert_eq!(g.result(), None);
    play(&mut g, "Ra2");
    assert_eq!(g.result(), Some(GameResult::FiftyMoveRule));
}

#[test]
fn checkmate_beats_fifty_move_rule() {
    let mut g = Game::from_fen("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 99 80").unwrap();
    play(&mut g, "Rd8#");
    assert_eq!(g.result(), Some(GameResult::Checkmate { winner: Color::White }));
}

#[test]
fn insufficient_material_after_capture() {
    let mut g = Game::from_fen("4k3/8/8/8/8/8/3r4/3QK3 w - - 0 1").unwrap();
    play(&mut g, "Qxd2");
    assert_eq!(g.result(), None); // K+Q v K is a win
    let mut g = Game::from_fen("4k3/8/8/8/8/8/3r4/3NK3 w - - 0 1").unwrap();
    play(&mut g, "Kxd2");
    assert_eq!(g.result(), Some(GameResult::InsufficientMaterial));
}

#[test]
fn undo_restores_everything() {
    let mut g = Game::new();
    play(&mut g, "e4 e5 Nf3 Nc6 Bb5 a6 O-O");
    let n = g.ply_count();
    while g.undo().is_some() {}
    assert_eq!(g.position().to_fen(), START_FEN);
    assert_eq!(g.ply_count(), 0);
    assert!(n == 7);
    assert!(g.undo().is_none());
}

#[test]
fn resignation_and_undo() {
    let mut g = Game::new();
    play(&mut g, "e4");
    g.resign(Color::Black);
    assert_eq!(g.result(), Some(GameResult::Resignation { winner: Color::White }));
    assert!(g.play(parse_move(g.position(), "e5").unwrap()).is_err());
    g.undo();
    assert_eq!(g.result(), None);
}

#[test]
fn en_passant_and_castling_through_game() {
    let mut g = Game::new();
    play(&mut g, "e4 Nf6 e5 d5 exd6 exd6 Nf3 Be7 Bc4 O-O O-O");
    assert_eq!(g.position().to_fen(), "rnbq1rk1/ppp1bppp/3p1n2/8/2B5/5N2/PPPP1PPP/RNBQ1RK1 b - - 5 6");
}

#[test]
fn history_hashes_exclude_current() {
    let mut g = Game::new();
    play(&mut g, "e4 e5");
    let h = g.history_hashes();
    assert_eq!(h.len(), 2);
    assert_eq!(h[0], g.start_position().hash());
    assert!(!h.contains(&g.position().hash()));
}

#[test]
fn pgn_export() {
    let mut g = Game::new();
    play(&mut g, "f3 e5 g4 Qh4#");
    let pgn = g.to_pgn("Human", "chess-rs");
    assert!(pgn.contains("[Result \"0-1\"]"));
    assert!(pgn.contains("1. f3 e5 2. g4 Qh4# 0-1"), "{pgn}");

    let mut g = Game::from_fen("4k3/8/8/8/8/8/4P3/4K3 b - - 0 12").unwrap();
    play(&mut g, "Kd7 e4");
    let pgn = g.to_pgn("a", "b");
    assert!(pgn.contains("[FEN \"4k3/8/8/8/8/8/4P3/4K3 b - - 0 12\"]"));
    assert!(pgn.contains("12... Kd7 13. e4 *"), "{pgn}");
}
