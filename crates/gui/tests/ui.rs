//! End-to-end GUI tests: the real `ChessApp` driven headlessly through
//! egui_kittest with synthetic pointer events and AccessKit button lookups.
//! The bot runs on its real background thread.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chess_engine::types::parse_sq;
use chess_engine::{Color, GameResult, PieceType, Position};
use chess_rs::worker::Purpose;
use chess_rs::{AppConfig, ChessApp, Mode, SidePick};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

type H = Harness<'static, Option<ChessApp>>;

fn cfg() -> AppConfig {
    AppConfig {
        depth: 4,
        threads: 2,
        hash_mb: 16,
        cache_path: None,
        mode: Mode::HumanVsBot,
        side: SidePick::White,
        fen: None,
        bot_delay: Duration::ZERO,
    }
}

fn harness_with(cfg: AppConfig, render: bool) -> H {
    let builder = Harness::<Option<ChessApp>>::builder().with_size(egui::vec2(1280.0, 880.0));
    let builder = if render { builder.wgpu() } else { builder };
    let mut h = builder.build_ui_state(
        move |ui, app: &mut Option<ChessApp>| app.get_or_insert_with(|| ChessApp::new(ui.ctx(), cfg.clone())).draw(ui),
        None,
    );
    h.step();
    h.step();
    h
}

fn harness(cfg: AppConfig) -> H {
    harness_with(cfg, false)
}

fn app(h: &H) -> &ChessApp {
    h.state().as_ref().unwrap()
}

fn app_mut(h: &mut H) -> &mut ChessApp {
    h.state_mut().as_mut().unwrap()
}

/// Step the UI until `cond` holds (the engine thread works in real time).
fn wait(h: &mut H, what: &str, cond: impl Fn(&ChessApp) -> bool) {
    let t = Instant::now();
    while !cond(app(h)) {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for: {what}");
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn center(h: &H, sq: &str) -> egui::Pos2 {
    app(h).board.square_rect(parse_sq(sq).unwrap()).center()
}

fn click_square(h: &mut H, sq: &str) {
    let p = center(h, sq);
    h.hover_at(p);
    h.step();
    h.drag_at(p);
    h.step();
    h.drop_at(p);
    h.step();
}

fn drag_piece(h: &mut H, from: &str, to: &str) {
    let (a, b) = (center(h, from), center(h, to));
    h.hover_at(a);
    h.step();
    h.drag_at(a);
    h.step();
    for i in 1..=6 {
        h.hover_at(a + (b - a) * (i as f32 / 6.0));
        h.step();
    }
    h.drop_at(b);
    h.step();
}

fn click_button(h: &mut H, label: &str) {
    h.get_by_label(label).click();
    h.step();
    h.step();
}

fn shots_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ui-screenshots");
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn click_to_move_and_bot_replies() {
    let mut h = harness(cfg());
    assert!(app(&h).status_text().starts_with("Your move"));
    click_square(&mut h, "e2");
    assert_eq!(app(&h).board.selected, parse_sq("e2"));
    click_square(&mut h, "e4");
    assert_eq!(app(&h).game.san_moves(), ["e4"]);
    wait(&mut h, "bot reply", |a| a.game.ply_count() == 2);
    assert_eq!(app(&h).game.side_to_move(), Color::White);
    assert!(app(&h).last_search.as_ref().is_some_and(|l| l.result.depth == 4));
}

#[test]
fn drag_and_drop_move() {
    let mut h = harness(cfg());
    drag_piece(&mut h, "g1", "f3");
    assert_eq!(app(&h).game.san_moves(), ["Nf3"]);
}

#[test]
fn illegal_moves_and_opponent_pieces_are_ignored() {
    let mut h = harness(cfg());
    click_square(&mut h, "e7"); // black pawn: not selectable for White
    assert_eq!(app(&h).board.selected, None);
    click_square(&mut h, "e2");
    click_square(&mut h, "e5"); // three squares: illegal
    assert_eq!(app(&h).game.ply_count(), 0);
    drag_piece(&mut h, "d1", "d5"); // queen is blocked
    assert_eq!(app(&h).game.ply_count(), 0);
    click_square(&mut h, "b1");
    assert_eq!(app(&h).board.selected, parse_sq("b1"));
    click_square(&mut h, "b1"); // clicking again deselects
    assert_eq!(app(&h).board.selected, None);
}

#[test]
fn promotion_dialog_underpromotes() {
    let mut h = harness(AppConfig { fen: Some("8/1P5k/8/8/8/8/8/K7 w - - 0 1".into()), ..cfg() });
    click_square(&mut h, "b7");
    click_square(&mut h, "b8");
    assert!(app(&h).promotion.is_some(), "promotion picker should open");
    assert_eq!(app(&h).game.ply_count(), 0);
    click_button(&mut h, "Knight");
    let pos = app(&h).game.position_at(1);
    assert_eq!(pos.piece_on(parse_sq("b8").unwrap()).map(|p| p.kind()), Some(PieceType::Knight));
    assert_eq!(app(&h).game.san_moves()[0], "b8=N");
}

#[test]
fn promotion_can_be_cancelled() {
    let mut h = harness(AppConfig { fen: Some("8/1P5k/8/8/8/8/8/K7 w - - 0 1".into()), ..cfg() });
    drag_piece(&mut h, "b7", "b8");
    assert!(app(&h).promotion.is_some());
    click_button(&mut h, "Cancel");
    assert!(app(&h).promotion.is_none());
    assert_eq!(app(&h).game.ply_count(), 0);
}

#[test]
fn human_delivers_checkmate() {
    let mut h = harness(AppConfig { fen: Some("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1".into()), ..cfg() });
    drag_piece(&mut h, "d1", "d8");
    h.step();
    assert_eq!(app(&h).game.result(), Some(GameResult::Checkmate { winner: Color::White }));
    assert_eq!(app(&h).status_text(), "Checkmate — White wins");
    // Board is now read-only.
    click_square(&mut h, "g1");
    assert_eq!(app(&h).board.selected, None);
}

#[test]
fn bot_finds_mate_against_human() {
    // Human plays Black; the bot (White) has mate in 1.
    let mut h = harness(AppConfig {
        side: SidePick::Black,
        fen: Some("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1".into()),
        ..cfg()
    });
    assert!(app(&h).board.flipped, "board flips when playing Black");
    wait(&mut h, "bot mates", |a| a.game.result().is_some());
    assert_eq!(app(&h).game.result(), Some(GameResult::Checkmate { winner: Color::White }));
}

#[test]
fn undo_takes_back_human_and_bot_moves() {
    let mut h = harness(cfg());
    click_square(&mut h, "d2");
    click_square(&mut h, "d4");
    wait(&mut h, "bot reply", |a| a.game.ply_count() == 2);
    click_button(&mut h, "Undo");
    assert_eq!(app(&h).game.ply_count(), 0);
    assert_eq!(app(&h).game.position(), &Position::startpos());
    assert!(app(&h).status_text().starts_with("Your move"));
}

#[test]
fn hint_shows_a_legal_move() {
    let mut h = harness(cfg());
    click_button(&mut h, "Hint");
    wait(&mut h, "hint", |a| a.hint.is_some());
    let hint = app(&h).hint.unwrap();
    assert!(app(&h).game.legal_moves().contains(&hint));
    assert_eq!(app(&h).last_search.as_ref().unwrap().purpose, Purpose::Hint);
    assert_eq!(app(&h).game.ply_count(), 0, "a hint must not play a move");
}

#[test]
fn keyboard_shortcuts() {
    let mut h = harness(cfg());
    h.key_press(egui::Key::F);
    h.step();
    assert!(app(&h).board.flipped);
    click_square(&mut h, "e2");
    click_square(&mut h, "e4");
    wait(&mut h, "bot reply", |a| a.game.ply_count() == 2);
    h.key_press(egui::Key::ArrowLeft);
    h.step();
    assert_eq!(app(&h).view_ply, Some(1));
    assert!(app(&h).is_reviewing());
    h.key_press(egui::Key::End);
    h.step();
    assert_eq!(app(&h).view_ply, None);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    h.step();
    assert_eq!(app(&h).game.ply_count(), 0);
}

#[test]
fn reviewing_history_blocks_moves() {
    let mut h = harness(cfg());
    click_square(&mut h, "e2");
    click_square(&mut h, "e4");
    wait(&mut h, "bot reply", |a| a.game.ply_count() == 2);
    app_mut(&mut h).view_ply = Some(0);
    h.step();
    click_square(&mut h, "d2");
    click_square(&mut h, "d4");
    assert_eq!(app(&h).game.ply_count(), 2, "no moves while reviewing");
}

#[test]
fn bot_vs_bot_plays_on_its_own_and_pauses() {
    let mut h = harness(AppConfig { mode: Mode::BotVsBot, depth: 2, ..cfg() });
    wait(&mut h, "several bot moves", |a| a.game.ply_count() >= 6);
    click_button(&mut h, "Pause");
    let n = app(&h).game.ply_count();
    // An in-flight search may still land once; afterwards play must stop.
    for _ in 0..40 {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    let after = app(&h).game.ply_count();
    assert!(after <= n + 1);
    for _ in 0..40 {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(app(&h).game.ply_count(), after);
}

#[test]
fn new_game_as_black_lets_bot_open() {
    let mut h = harness(cfg());
    app_mut(&mut h).set_side(SidePick::Black);
    click_button(&mut h, "New game");
    assert_eq!(app(&h).human, Color::Black);
    wait(&mut h, "bot opening move", |a| a.game.ply_count() == 1);
    assert!(app(&h).status_text().starts_with("Your move (Black)"));
}

#[test]
fn repeated_positions_are_answered_from_cache() {
    let mut h = harness(AppConfig { side: SidePick::Black, depth: 6, ..cfg() });
    wait(&mut h, "first bot move", |a| a.game.ply_count() == 1);
    let first = app(&h).game.moves()[0];
    assert!(!app(&h).last_search.as_ref().unwrap().from_cache);
    click_button(&mut h, "New game");
    wait(&mut h, "second bot move", |a| a.game.ply_count() == 1);
    let ls = app(&h).last_search.as_ref().unwrap();
    assert!(ls.from_cache, "the same position should be answered instantly from the cache");
    assert_eq!(app(&h).game.moves()[0], first);
    wait(&mut h, "live cache stats", |a| a.cache_stats.hits >= 1 && a.cache_stats.root_entries >= 1);
}

#[test]
fn cache_persists_across_app_instances() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cache-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("positions.bin");
    let config = AppConfig { side: SidePick::Black, depth: 6, cache_path: Some(path.clone()), ..cfg() };

    let mut h = harness(config.clone());
    wait(&mut h, "bot move", |a| a.game.ply_count() == 1);
    app_mut(&mut h).shutdown(); // saves the cache
    drop(h);
    assert!(path.exists(), "cache file written");

    let mut h = harness(config);
    wait(&mut h, "cache load", |a| a.cache_ready);
    assert!(app(&h).cache_stats.root_entries >= 1);
    wait(&mut h, "bot move", |a| a.game.ply_count() == 1);
    assert!(app(&h).last_search.as_ref().unwrap().from_cache);
    app_mut(&mut h).shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resign_ends_the_game() {
    let mut h = harness(cfg());
    click_button(&mut h, "Resign");
    assert_eq!(app(&h).game.result(), Some(GameResult::Resignation { winner: Color::Black }));
}

/// Renders screenshots of key states (inspect them under target/tmp/ui-screenshots).
#[test]
fn render_screenshots() {
    let mut h = harness_with(cfg(), true);
    for _ in 0..4 {
        h.step();
    }
    h.render().unwrap().save(shots_dir().join("01-start.png")).unwrap();
    click_square(&mut h, "e2");
    h.step();
    h.render().unwrap().save(shots_dir().join("02-selected.png")).unwrap();
    click_square(&mut h, "e4");
    wait(&mut h, "bot reply", |a| a.game.ply_count() == 2);
    for _ in 0..4 {
        h.step();
    }
    h.render().unwrap().save(shots_dir().join("03-after-reply.png")).unwrap();

    let mut h = harness_with(AppConfig { fen: Some("8/1P5k/8/8/8/8/8/K7 w - - 0 1".into()), ..cfg() }, true);
    click_square(&mut h, "b7");
    click_square(&mut h, "b8");
    h.step();
    h.render().unwrap().save(shots_dir().join("04-promotion.png")).unwrap();

    let mut h = harness_with(AppConfig { fen: Some("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1".into()), ..cfg() }, true);
    drag_piece(&mut h, "d1", "d8");
    for _ in 0..4 {
        h.step();
    }
    h.render().unwrap().save(shots_dir().join("05-checkmate.png")).unwrap();
}

#[test]
fn en_passant_by_clicking() {
    // After 1.e4 Nf6 2.e5 d5, White can take en passant on d6.
    let mut h = harness(AppConfig {
        fen: Some("rnbqkb1r/ppp1pppp/5n2/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3".into()),
        ..cfg()
    });
    click_square(&mut h, "e5");
    click_square(&mut h, "d6");
    let pos = app(&h).game.position_at(1);
    assert_eq!(app(&h).game.san_moves()[0], "exd6");
    assert!(pos.piece_on(parse_sq("d5").unwrap()).is_none(), "captured pawn removed from d5");
    assert_eq!(pos.piece_on(parse_sq("d6").unwrap()).map(|p| p.kind()), Some(PieceType::Pawn));
}

#[test]
fn castling_by_dragging_the_king() {
    let mut h = harness(AppConfig { fen: Some("r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w KQkq - 0 1".into()), ..cfg() });
    drag_piece(&mut h, "e1", "g1");
    assert_eq!(app(&h).game.san_moves()[0], "O-O");
    let pos = app(&h).game.position_at(1);
    assert_eq!(pos.piece_on(parse_sq("f1").unwrap()).map(|p| p.kind()), Some(PieceType::Rook));
}

#[test]
fn live_position_counter_while_thinking() {
    // Deep search so the bot is still thinking while we watch the counter.
    let mut h = harness_with(AppConfig { side: SidePick::Black, depth: 40, threads: 2, ..cfg() }, true);
    wait(&mut h, "bot starts thinking", |a| a.is_thinking() && a.live_positions().is_some_and(|n| n > 0));
    let first = app(&h).live_positions().unwrap();
    wait(&mut h, "counter keeps rising", |a| a.live_positions().is_some_and(|n| n > first * 2 + 100_000));
    // The count is shown prominently in the side panel.
    let label = format!("Positions searched: {}", chess_rs::app::with_commas(app(&h).live_positions().unwrap()));
    assert!(label.contains(','));
    h.step();
    assert!(h.query_by_label_contains("Positions searched:").is_some(), "counter label visible");
    h.render().unwrap().save(shots_dir().join("06-positions-counter.png")).unwrap();
    click_button(&mut h, "Move now");
    wait(&mut h, "bot plays after Move now", |a| a.game.ply_count() == 1);
    assert!(app(&h).live_positions().is_none(), "counter hidden when not searching");
    assert!(h.query_by_label_contains("Positions searched:").is_none());
}

#[test]
fn thousands_separators() {
    use chess_rs::app::with_commas;
    assert_eq!(with_commas(0), "0");
    assert_eq!(with_commas(999), "999");
    assert_eq!(with_commas(1_000), "1,000");
    assert_eq!(with_commas(12_345_678), "12,345,678");
    assert_eq!(with_commas(u64::MAX), "18,446,744,073,709,551,615");
}
