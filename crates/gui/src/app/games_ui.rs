//! Games tab: every saved game, newest first, with review and delete.

use chess_engine::pgn::parse_one;
use egui::{RichText, Ui};

use super::ChessApp;

impl ChessApp {
    /// Re-read the games folder.
    pub fn refresh_games(&mut self) {
        self.games = self.library.as_ref().map(|l| l.list()).unwrap_or_default();
        self.games_dirty = false;
    }

    /// Open a saved game in the Game Review tab (analysing it if needed).
    pub fn review_saved(&mut self, index: usize) {
        let Some(saved) = self.games.get(index).cloned() else { return };
        let Some(lib) = &self.library else { return };
        match lib.load(&saved.path) {
            Ok(pgn) => self.open_review(pgn, Some(saved.path), true),
            Err(e) => self.message = Some(format!("Could not open game: {e}")),
        }
    }

    pub fn delete_saved(&mut self, index: usize) {
        let Some(saved) = self.games.get(index).cloned() else { return };
        if let Some(lib) = &self.library
            && let Err(e) = lib.delete(&saved.path)
        {
            self.message = Some(format!("Could not delete: {e}"));
        }
        if self.record.as_ref() == Some(&saved.path) {
            self.record = None;
        }
        self.confirm_delete = None;
        self.refresh_games();
    }

    pub(super) fn games_tab(&mut self, ui: &mut Ui) {
        if self.games_dirty {
            self.refresh_games();
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Saved games ({})", self.games.len())).size(22.0).strong());
            if ui.button("Refresh").clicked() {
                self.refresh_games();
            }
        });
        match &self.library {
            Some(lib) => {
                ui.label(RichText::new(format!("Every game is saved automatically to {}", lib.dir().display())).weak())
            }
            None => ui.label(RichText::new("Game saving is off (--no-games).").weak()),
        };
        ui.add_space(6.0);

        let mut review = None;
        let mut delete = None;
        let mut confirm = None;
        let mut cancel = false;
        if self.games.is_empty() {
            ui.label("No games yet. Play one in the Play tab and it will appear here.");
        } else {
            egui::ScrollArea::vertical().id_salt("games_list").max_height(ui.available_height() - 170.0).show(
                ui,
                |ui| {
                    egui::Grid::new("games_grid").num_columns(8).striped(true).spacing([18.0, 6.0]).show(ui, |ui| {
                        for h in ["Date (UTC)", "White", "Black", "Result", "Moves", "Accuracy", "", ""] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (i, g) in self.games.iter().enumerate() {
                            ui.label(format!("{} {}", g.date, g.time));
                            ui.label(&g.white);
                            ui.label(&g.black);
                            ui.label(&g.result).on_hover_text(&g.termination);
                            ui.label(g.moves().to_string());
                            match &g.accuracy {
                                Some((w, b)) => ui.label(format!("{w} / {b}")),
                                None => ui.label(RichText::new("not reviewed").weak()),
                            };
                            if ui.button("Review").on_hover_text("Open in Game Review").clicked() {
                                review = Some(i);
                            }
                            if self.confirm_delete.as_ref() == Some(&g.path) {
                                ui.horizontal(|ui| {
                                    if ui.button("Confirm delete").clicked() {
                                        delete = Some(i);
                                    }
                                    if ui.button("Keep").clicked() {
                                        cancel = true;
                                    }
                                });
                            } else if ui.button("Delete").clicked() {
                                confirm = Some(g.path.clone());
                            }
                            ui.end_row();
                        }
                    });
                },
            );
        }
        if let Some(i) = review {
            self.review_saved(i);
        }
        if let Some(i) = delete {
            self.delete_saved(i);
        }
        if let Some(p) = confirm {
            self.confirm_delete = Some(p);
        }
        if cancel {
            self.confirm_delete = None;
        }

        ui.separator();
        ui.label(RichText::new("Review any game: paste PGN").strong());
        ui.add(
            egui::TextEdit::multiline(&mut self.pgn_input)
                .hint_text("[Event \"...\"]\n1. e4 e5 2. Nf3 ...")
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        if ui.button("Review pasted PGN").clicked() {
            match parse_one(&self.pgn_input) {
                Ok(pgn) if !pgn.moves.is_empty() => self.open_review(pgn, None, true),
                Ok(_) => self.message = Some("That PGN has no moves".into()),
                Err(e) => self.message = Some(format!("Invalid PGN: {e}")),
            }
        }
        if let Some(m) = &self.message {
            ui.label(RichText::new(m).weak());
        }
    }
}
