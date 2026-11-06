//! Game Review tab: chess.com-style analysis of a finished or saved game.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chess_engine::analysis::{self, GameReview, MoveClass, PositionEval};
use chess_engine::notation::line_to_san;
use chess_engine::pgn::PgnGame;
use chess_engine::{Color, Game};
use egui::{Align2, Color32, FontId, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::{ChessApp, Tab, format_score};
use crate::board::{self, BoardView};
use crate::review_worker::ReviewEvent;

/// Rating colours (each is always shown together with its symbol and label).
pub fn class_color(c: MoveClass) -> Color32 {
    match c {
        MoveClass::Brilliant => Color32::from_rgb(27, 172, 166),
        MoveClass::Great => Color32::from_rgb(92, 139, 176),
        MoveClass::Best => Color32::from_rgb(110, 165, 60),
        MoveClass::Excellent => Color32::from_rgb(140, 180, 80),
        MoveClass::Good => Color32::from_rgb(130, 150, 120),
        MoveClass::Inaccuracy => Color32::from_rgb(230, 180, 40),
        MoveClass::Mistake => Color32::from_rgb(225, 130, 40),
        MoveClass::Miss => Color32::from_rgb(230, 100, 85),
        MoveClass::Blunder => Color32::from_rgb(202, 52, 49),
        MoveClass::Forced => Color32::from_rgb(140, 150, 156),
    }
}

/// State of the Game Review tab.
pub struct ReviewState {
    pub pgn: PgnGame,
    pub game: Game,
    /// File the game came from (the review is saved back into it).
    pub path: Option<PathBuf>,
    pub review: Option<GameReview>,
    /// Evaluations streamed in while the analysis runs.
    pub partial: Vec<Option<PositionEval>>,
    pub progress: (usize, usize),
    /// Position being shown: 0 = start, i = after ply i.
    pub ply: usize,
    pub show_best: bool,
    pub job: Option<(u64, Arc<AtomicBool>)>,
}

impl ReviewState {
    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// White-relative score of position `i`, from the review or the partial results.
    pub fn score_at(&self, i: usize) -> Option<i32> {
        match &self.review {
            Some(r) => r.evals.get(i).map(|e| e.score),
            None => self.partial.get(i).and_then(|e| e.as_ref()).map(|e| e.score),
        }
    }

    pub fn plies(&self) -> usize {
        self.game.ply_count()
    }
}

impl ChessApp {
    /// Open the Game Review tab for a PGN game (from a file or the current game).
    pub fn open_review(&mut self, pgn: PgnGame, path: Option<PathBuf>, autostart: bool) {
        if let Some(old) = self.review.take()
            && let Some((_, stop)) = old.job
        {
            stop.store(true, Ordering::Relaxed);
        }
        let game = pgn.to_game();
        let review = analysis::review_from_pgn(&pgn);
        let n = game.ply_count();
        let needs_run = review.is_none();
        self.review = Some(ReviewState {
            pgn,
            game,
            path,
            review,
            partial: vec![None; n + 1],
            progress: (0, n + 1),
            ply: 0,
            show_best: true,
            job: None,
        });
        self.tab = Tab::Review;
        if autostart && needs_run {
            self.start_review();
        }
    }

    /// Review the game currently being played.
    pub fn review_current_game(&mut self) {
        self.autosave();
        let pgn = self.current_pgn();
        let path = self.record.clone();
        // Reuse a stored review if this exact game was already analysed.
        let stored = path.as_ref().and_then(|p| self.library.as_ref()?.load(p).ok());
        match stored {
            Some(saved) if saved.moves == pgn.moves && analysis::review_from_pgn(&saved).is_some() => {
                self.open_review(saved, path, false)
            }
            _ => self.open_review(pgn, path, true),
        }
    }

    pub fn start_review(&mut self) {
        let depth = self.review_depth;
        let threads = self.threads;
        let Some(st) = self.review.as_mut() else { return };
        if let Some((_, stop)) = st.job.take() {
            stop.store(true, Ordering::Relaxed);
        }
        let n = st.plies();
        st.review = None;
        st.partial = vec![None; n + 1];
        st.progress = (0, n + 1);
        st.job = Some(self.reviewer.start(st.game.clone(), depth, threads));
    }

    pub fn stop_review(&mut self) {
        if let Some(st) = self.review.as_mut()
            && let Some((_, stop)) = st.job.take()
        {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Drain review-thread events; store finished reviews in the PGN file.
    pub(super) fn poll_review(&mut self) {
        while let Some(ev) = self.reviewer.try_recv() {
            let Some(st) = self.review.as_mut() else { continue };
            let current = st.job.as_ref().map(|(id, _)| *id);
            match ev {
                ReviewEvent::Progress { id, done, total, index, eval } if Some(id) == current => {
                    st.progress = (done, total);
                    if let Some(slot) = st.partial.get_mut(index) {
                        *slot = Some(eval);
                    }
                }
                ReviewEvent::Done { id, review } if Some(id) == current => {
                    st.job = None;
                    if let Some(r) = review {
                        analysis::annotate(&mut st.pgn, &r);
                        st.review = Some(r);
                        if let (Some(lib), Some(path)) = (&self.library, &st.path) {
                            if let Err(e) = lib.save(path, &st.pgn) {
                                self.message = Some(format!("Could not save review: {e}"));
                            }
                            self.games_dirty = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn review_keys(&mut self, ui: &Ui) {
        let Some(st) = self.review.as_mut() else { return };
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let n = st.plies();
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                st.ply = st.ply.saturating_sub(1);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                st.ply = (st.ply + 1).min(n);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Home) {
                st.ply = 0;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::End) {
                st.ply = n;
            }
        });
    }

    pub(super) fn review_board_area(&mut self, ui: &mut Ui) {
        let Some(st) = self.review.as_mut() else {
            ui.centered_and_justified(|ui| ui.label("Pick a game in the Games tab, or review the current game."));
            return;
        };
        let avail = ui.available_size();
        let graph_h = 96.0;
        let nav_h = 34.0;
        let size = (avail.x - 52.0).min(avail.y - nav_h - graph_h - 24.0).max(160.0);
        let n = st.plies();
        st.ply = st.ply.min(n);
        let ply = st.ply;
        let pos = *st.game.position_at(ply);
        let last_move = ply.checked_sub(1).map(|i| st.game.moves()[i]);
        let reviewed = st.review.as_ref().and_then(|r| ply.checked_sub(1).and_then(|i| r.moves.get(i)));

        // Rating badge on the moved piece, and a green arrow for the better move.
        let symbol;
        let badge = match reviewed {
            Some(m) => {
                symbol = m.class.symbol();
                Some((m.mv.to(), class_color(m.class), symbol))
            }
            None => None,
        };
        let mut arrows = Vec::new();
        if let Some(m) = reviewed
            && m.class.is_error()
            && let Some(best) = m.best
        {
            arrows.push((best, Color32::from_rgba_unmultiplied(110, 175, 60, 190)));
        }
        if st.show_best
            && let Some(r) = &st.review
            && let Some(next) = r.evals.get(ply).and_then(|e| e.best)
        {
            arrows.push((next, Color32::from_rgba_unmultiplied(40, 120, 210, 170)));
        }

        let score = st.score_at(ply);
        let flipped = self.board.flipped;
        let board_rect = ui
            .vertical_centered(|ui| {
                let r = ui
                    .horizontal(|ui| {
                        ui.add_space(((avail.x - size - 44.0) / 2.0).max(0.0));
                        let (frac, label) = match score {
                            Some(s) => (analysis::win_percent(s) as f32 / 100.0, format_score(s)),
                            None => (0.5, String::new()),
                        };
                        board::eval_bar(ui, size, frac, &label, flipped);
                        ui.add_space(6.0);
                        let view = BoardView {
                            pos: &pos,
                            last_move,
                            legal: &[],
                            hint: None,
                            show_legal: false,
                            images: &self.images,
                            arrows: &arrows,
                            badge,
                        };
                        board::board_ui(ui, &mut self.board, &view, size);
                        self.board.rect
                    })
                    .inner;
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(((avail.x - 260.0) / 2.0).max(0.0));
                    if ui.button("«").on_hover_text("Start (Home)").clicked() {
                        st.ply = 0;
                    }
                    if ui.button("‹").on_hover_text("Previous (Left arrow)").clicked() {
                        st.ply = st.ply.saturating_sub(1);
                    }
                    ui.label(RichText::new(format!("ply {ply}/{n}")).monospace());
                    if ui.button("›").on_hover_text("Next (Right arrow)").clicked() {
                        st.ply = (st.ply + 1).min(n);
                    }
                    if ui.button("»").on_hover_text("End (End)").clicked() {
                        st.ply = n;
                    }
                });
                r
            })
            .inner;
        ui.add_space(8.0);
        // Evaluation graph spans the eval bar + board.
        let graph_rect = egui::Rect::from_min_size(
            pos2(board_rect.min.x - 28.0, ui.cursor().min.y),
            vec2(board_rect.width() + 28.0, graph_h),
        );
        if let Some(clicked) = eval_graph(ui, graph_rect, st) {
            st.ply = clicked;
        }
    }

    pub(super) fn review_side_panel(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Game Review").size(22.0).strong());
        let Some(st) = self.review.as_ref() else {
            ui.label(RichText::new("No game selected.").weak());
            if ui.add_enabled(self.game.ply_count() > 0, egui::Button::new("Review current game")).clicked() {
                self.review_current_game();
            }
            return;
        };
        let white = st.pgn.tag("White").unwrap_or("White").to_string();
        let black = st.pgn.tag("Black").unwrap_or("Black").to_string();
        let term = st.pgn.tag("Termination").unwrap_or("").to_string();
        ui.label(format!("{white} vs {black}"));
        ui.label(RichText::new(format!("{} {term}", st.pgn.result)).weak());
        ui.separator();

        // ---- run / progress ----
        let running = st.is_running();
        let (done, total) = st.progress;
        let has_review = st.review.is_some();
        if running {
            let frac = done as f32 / total.max(1) as f32;
            ui.add(egui::ProgressBar::new(frac).text(format!("Analyzing… {done}/{total} positions")));
            if ui.button("Stop analysis").clicked() {
                self.stop_review();
            }
        } else {
            ui.horizontal(|ui| {
                let label = if has_review { "Re-analyze" } else { "Analyze" };
                if ui.button(label).clicked() {
                    self.start_review();
                }
                ui.add(egui::Slider::new(&mut self.review_depth, 1..=super::MAX_DEPTH).text("depth"));
            });
        }
        let Some(st) = self.review.as_mut() else { return };
        ui.checkbox(&mut st.show_best, "Show engine's best move (blue arrow)");
        ui.separator();

        // ---- accuracy + classification counts ----
        if let Some(r) = &st.review {
            ui.horizontal(|ui| {
                for (c, name) in [(Color::White, &white), (Color::Black, &black)] {
                    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
                        ui.set_min_width(150.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("{:.1}", r.accuracy[c.idx()])).size(30.0).strong());
                            ui.label(RichText::new(format!("{} accuracy", c.name())).weak());
                            ui.label(RichText::new(name).small());
                        });
                    });
                }
            });
            egui::Grid::new("class_counts").num_columns(3).spacing([18.0, 3.0]).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new("White").weak());
                ui.label(RichText::new("Black").weak());
                ui.end_row();
                for class in MoveClass::ALL {
                    class_chip(ui, class, true);
                    ui.monospace(r.count(Color::White, class).to_string());
                    ui.monospace(r.count(Color::Black, class).to_string());
                    ui.end_row();
                }
            });
            ui.add(
                egui::Label::new(
                    RichText::new("Accuracy and ratings follow lichess's win-% model and chess.com's bands.")
                        .small()
                        .weak(),
                )
                .wrap(),
            );
            ui.separator();
        }

        // ---- current move explanation ----
        move_card(ui, st);
        ui.separator();

        // ---- move list ----
        ui.label(RichText::new("Moves").strong());
        review_move_list(ui, st);
        ui.separator();
        if ui.button("Copy annotated PGN").clicked() {
            ui.ctx().copy_text(st.pgn.to_pgn());
            self.message = Some("Annotated PGN copied".into());
        }
        if let Some(p) = &st.path {
            ui.label(RichText::new(p.display().to_string()).small().weak());
        }
    }
}

/// A coloured symbol + label for a rating (never colour alone).
fn class_chip(ui: &mut Ui, class: MoveClass, with_label: bool) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(26.0, 18.0), Sense::hover());
        ui.painter().rect_filled(rect, 9.0, class_color(class));
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            class.symbol(),
            FontId::proportional(12.0),
            Color32::WHITE,
        );
        if with_label {
            ui.label(class.label());
        }
    });
}

fn move_number_prefix(game: &Game, ply_index: usize) -> String {
    let pos = game.position_at(ply_index);
    let n = pos.fullmove_number();
    if pos.side_to_move() == Color::White { format!("{n}.") } else { format!("{n}...") }
}

fn move_card(ui: &mut Ui, st: &ReviewState) {
    let ply = st.ply;
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_min_width(ui.available_width() - 4.0);
        if ply == 0 {
            ui.label(RichText::new("Starting position").strong());
            if let Some(s) = st.score_at(0) {
                ui.label(format!("Evaluation: {}", format_score(s)));
            }
            return;
        }
        let i = ply - 1;
        let san = &st.game.san_moves()[i];
        let prefix = move_number_prefix(&st.game, i);
        match st.review.as_ref().and_then(|r| r.moves.get(i)) {
            Some(m) => {
                ui.horizontal(|ui| {
                    class_chip(ui, m.class, false);
                    let verdict = match m.class {
                        MoveClass::Brilliant => "is brilliant",
                        MoveClass::Great => "is a great move",
                        MoveClass::Best => "is the best move",
                        MoveClass::Excellent => "is excellent",
                        MoveClass::Good => "is good",
                        MoveClass::Inaccuracy => "is an inaccuracy",
                        MoveClass::Mistake => "is a mistake",
                        MoveClass::Miss => "is a miss",
                        MoveClass::Blunder => "is a blunder",
                        MoveClass::Forced => "was forced",
                    };
                    let text = format!("{prefix} {san} {verdict}");
                    ui.label(RichText::new(text).strong().color(ui.visuals().strong_text_color()));
                });
                let before = st.score_at(i).map(format_score).unwrap_or_default();
                let after = st.score_at(ply).map(format_score).unwrap_or_default();
                ui.label(format!("Evaluation {before} to {after}   (accuracy {:.0}%)", m.accuracy));
                if m.class.is_error()
                    && let Some(best) = m.best
                {
                    let before_pos = st.game.position_at(i);
                    let pv = st.review.as_ref().map(|r| r.evals[i].pv.clone()).unwrap_or_default();
                    let line = line_to_san(
                        before_pos,
                        if pv.first() == Some(&best) { &pv } else { std::slice::from_ref(&best) },
                    );
                    ui.label(
                        RichText::new(format!("Best was {} (green arrow)", line.first().cloned().unwrap_or_default()))
                            .strong(),
                    );
                    if line.len() > 1 {
                        ui.add(
                            egui::Label::new(RichText::new(format!("Line: {}", line.join(" "))).monospace().small())
                                .wrap(),
                        );
                    }
                }
            }
            None => {
                ui.label(RichText::new(format!("{prefix} {san}")).strong());
                ui.label(RichText::new("Not analysed yet").weak());
            }
        }
    });
}

fn review_move_list(ui: &mut Ui, st: &mut ReviewState) {
    let sans = st.game.san_moves().to_vec();
    let classes: Vec<Option<MoveClass>> =
        (0..sans.len()).map(|i| st.review.as_ref().and_then(|r| r.moves.get(i)).map(|m| m.class)).collect();
    let start = *st.game.start_position();
    egui::ScrollArea::vertical().id_salt("review_moves").max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("review_moves_grid").num_columns(3).striped(true).min_col_width(70.0).show(ui, |ui| {
            let mut number = start.fullmove_number();
            let mut i = 0;
            let cell = |ui: &mut Ui, idx: usize, st: &mut ReviewState| {
                let label = match classes[idx] {
                    Some(c) => {
                        RichText::new(format!("{}{}", sans[idx], c.symbol().trim_start_matches(['*', '+', '=', '-'])))
                            .color(if c.is_error() || matches!(c, MoveClass::Brilliant | MoveClass::Great) {
                                class_color(c)
                            } else {
                                ui.visuals().text_color()
                            })
                    }
                    None => RichText::new(&sans[idx]),
                };
                if ui.selectable_label(st.ply == idx + 1, label).clicked() {
                    st.ply = idx + 1;
                }
            };
            if start.side_to_move() == Color::Black && !sans.is_empty() {
                ui.monospace(format!("{number}."));
                ui.monospace("…");
                cell(ui, 0, st);
                ui.end_row();
                number += 1;
                i = 1;
            }
            while i < sans.len() {
                ui.monospace(format!("{number}."));
                cell(ui, i, st);
                if i + 1 < sans.len() {
                    cell(ui, i + 1, st);
                }
                ui.end_row();
                number += 1;
                i += 2;
            }
        });
    });
}

/// White's winning chances over the game; click or drag to jump to a move.
/// Returns the ply to show if the user clicked.
fn eval_graph(ui: &mut Ui, rect: egui::Rect, st: &ReviewState) -> Option<usize> {
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let p = ui.painter_at(rect);
    let n = st.plies();
    let dark = Color32::from_gray(48);
    let light = Color32::from_gray(225);
    p.rect_filled(rect, 4.0, dark);
    let x_of = |i: usize| rect.left() + rect.width() * (i as f32 / n.max(1) as f32);
    let y_of = |win: f64| rect.bottom() - rect.height() * (win as f32 / 100.0);

    // White area under the curve, drawn as convex quads between known points.
    let points: Vec<(usize, f64)> =
        (0..=n).filter_map(|i| st.score_at(i).map(|s| (i, analysis::win_percent(s)))).collect();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let quad = vec![
            pos2(x_of(a.0), rect.bottom()),
            pos2(x_of(a.0), y_of(a.1)),
            pos2(x_of(b.0), y_of(b.1)),
            pos2(x_of(b.0), rect.bottom()),
        ];
        p.add(egui::Shape::convex_polygon(quad, light, Stroke::NONE));
    }
    // Recessive midline (equal position).
    p.line_segment(
        [pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)],
        Stroke::new(1.0, Color32::from_gray(120)),
    );
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        p.line_segment(
            [pos2(x_of(a.0), y_of(a.1)), pos2(x_of(b.0), y_of(b.1))],
            Stroke::new(2.0, Color32::from_gray(150)),
        );
    }
    // Markers for notable moves (colour + the move list/labels carry meaning too).
    if let Some(r) = &st.review {
        for m in &r.moves {
            if m.class.is_error() || matches!(m.class, MoveClass::Brilliant | MoveClass::Great) {
                let win_white = if m.mover == Color::White { m.win_after } else { 100.0 - m.win_after };
                let c = pos2(x_of(m.ply), y_of(win_white));
                p.circle_filled(c, 4.5, class_color(m.class));
                p.circle_stroke(c, 4.5, Stroke::new(1.5, dark));
            }
        }
    }
    // Current position cursor.
    let cx = x_of(st.ply);
    p.line_segment([pos2(cx, rect.top()), pos2(cx, rect.bottom())], Stroke::new(2.0, Color32::from_rgb(40, 120, 210)));

    // Hover crosshair + tooltip.
    let to_ply = |x: f32| (((x - rect.left()) / rect.width()) * n as f32).round().clamp(0.0, n as f32) as usize;
    if let Some(hp) = resp.hover_pos() {
        let i = to_ply(hp.x);
        p.line_segment(
            [pos2(x_of(i), rect.top()), pos2(x_of(i), rect.bottom())],
            Stroke::new(1.0, Color32::from_gray(170)),
        );
        let text = if i == 0 {
            "Start".to_string()
        } else {
            let mut t = format!("{} {}", move_number_prefix(&st.game, i - 1), st.game.san_moves()[i - 1]);
            if let Some(m) = st.review.as_ref().and_then(|r| r.moves.get(i - 1)) {
                t.push_str(&format!(" · {}", m.class.label()));
            }
            t
        };
        let eval = st.score_at(i).map(format_score).unwrap_or_else(|| "…".into());
        resp.clone().on_hover_text_at_pointer(format!("{text}  {eval}"));
    }
    if (resp.clicked() || resp.dragged())
        && let Some(pp) = resp.interact_pointer_pos()
    {
        return Some(to_ply(pp.x));
    }
    None
}
