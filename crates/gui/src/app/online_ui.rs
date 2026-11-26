//! Online mode: host a game and share its code, or join with one.

use chess_engine::{Color, Game, START_FEN};
use egui::{Color32, Key, RichText, Ui};

use super::{ChessApp, SidePick, resolve_side};
use crate::online::{GameCode, NetCmd, NetEvent, OnlineHandle, apply_remote_move};

/// Where the online session stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnlineState {
    /// No session: host or join.
    Idle,
    /// Waiting for the opponent to enter our code.
    Hosting,
    /// Looking for the host of the code we entered.
    Joining,
    /// Connected; the game is on (or just finished).
    Playing,
    /// The opponent left or the connection dropped (the message says which).
    Ended(String),
}

const WARN: Color32 = Color32::from_rgb(230, 170, 60);
const ERROR: Color32 = Color32::from_rgb(230, 90, 80);

impl ChessApp {
    /// The code to give the opponent, while hosting or playing a hosted game.
    pub fn online_code(&self) -> Option<String> {
        self.online.as_ref().map(|n| n.code().to_string())
    }

    pub fn online_active(&self) -> bool {
        self.online.is_some()
    }

    fn wake(&self) -> impl Fn() + Send + 'static {
        let ctx = self.ctx.clone();
        move || ctx.request_repaint()
    }

    /// Start hosting: a fresh code to share; the game starts when someone joins.
    pub fn host_online(&mut self) {
        self.leave_online();
        let me = resolve_side(self.side_pick);
        let fen = self.start_fen.clone().unwrap_or_else(|| START_FEN.to_string());
        self.online = Some(OnlineHandle::host(self.network.clone(), me.flip(), fen, self.wake()));
        self.online_state = OnlineState::Hosting;
    }

    /// Join the game whose code is in the code box.
    pub fn join_online(&mut self) {
        match GameCode::parse(&self.online_code_input) {
            Ok(code) => {
                self.leave_online();
                self.online_code_input = code.to_string();
                self.online = Some(OnlineHandle::join(self.network.clone(), code, self.wake()));
                self.online_state = OnlineState::Joining;
            }
            Err(e) => self.online_error = Some(e),
        }
    }

    /// Leave the session (the opponent is told). The board stays as it is.
    pub fn leave_online(&mut self) {
        if let Some(net) = self.online.take() {
            net.send(NetCmd::Leave); // dropping it finishes the goodbye in the background
        }
        self.online_state = OnlineState::Idle;
        self.online_error = None;
        self.draw_offered_to_me = false;
        self.i_offered_draw = false;
    }

    fn online_playing(&self) -> bool {
        self.online_state == OnlineState::Playing && self.game.result().is_none()
    }

    pub fn offer_draw(&mut self) {
        if self.online_playing()
            && !self.i_offered_draw
            && let Some(net) = &self.online
        {
            net.send(NetCmd::OfferDraw);
            self.i_offered_draw = true;
            self.message = Some("Draw offered".into());
        }
    }

    pub fn accept_draw(&mut self) {
        if self.online_playing()
            && self.draw_offered_to_me
            && let Some(net) = &self.online
        {
            net.send(NetCmd::AcceptDraw);
            self.draw_offered_to_me = false;
            self.game.agree_draw();
            self.autosave();
        }
    }

    pub fn decline_draw(&mut self) {
        if self.draw_offered_to_me
            && let Some(net) = &self.online
        {
            net.send(NetCmd::DeclineDraw);
            self.draw_offered_to_me = false;
        }
    }

    /// Our move was just played on the board: send it to the opponent.
    pub(super) fn send_online_move(&mut self, ply: usize, uci: String) {
        if let Some(net) = &self.online {
            net.send(NetCmd::Move { ply, uci });
            if self.draw_offered_to_me {
                net.send(NetCmd::DeclineDraw); // moving declines the offer
                self.draw_offered_to_me = false;
            }
        }
    }

    pub(super) fn poll_online(&mut self) {
        let mut events = Vec::new();
        if let Some(net) = &self.online {
            while let Some(e) = net.try_recv() {
                events.push(e);
            }
        }
        for e in events {
            self.on_net_event(e);
        }
    }

    fn on_net_event(&mut self, e: NetEvent) {
        match e {
            NetEvent::Waiting | NetEvent::Searching => {}
            NetEvent::NoInternet => {
                self.online_error = Some("Can't reach the internet yet. Check your connection; still waiting.".into())
            }
            NetEvent::Started { my_color, fen } => {
                let game = Game::from_fen(&fen).unwrap_or_default();
                self.online_state = OnlineState::Playing;
                self.online_error = None;
                self.begin_game(game, my_color);
                self.message = Some(format!("Opponent connected. You play {}.", my_color.name()));
            }
            NetEvent::Move { ply, uci } => match apply_remote_move(&mut self.game, self.human, ply, &uci) {
                Ok(_) => {
                    self.i_offered_draw = false;
                    self.after_move();
                }
                Err(err) => {
                    self.leave_online();
                    self.online_state = OnlineState::Ended(format!("Left the game: {err}"));
                }
            },
            NetEvent::Resigned => {
                self.game.resign(self.human.flip());
                self.autosave();
            }
            NetEvent::DrawOffered => {
                if self.online_playing() {
                    self.draw_offered_to_me = true;
                }
            }
            NetEvent::DrawAccepted => {
                if self.i_offered_draw && self.online_playing() {
                    self.game.agree_draw();
                    self.autosave();
                }
                self.i_offered_draw = false;
            }
            NetEvent::DrawDeclined => {
                if self.i_offered_draw {
                    self.message = Some("Your opponent declined the draw".into());
                }
                self.i_offered_draw = false;
            }
            NetEvent::Left => self.end_online("Your opponent left the game."),
            NetEvent::Lost(err) => self.end_online(&format!("Connection lost: {err}")),
            NetEvent::Failed(err) => {
                self.online = None;
                self.online_state = OnlineState::Idle;
                self.online_error = Some(err);
            }
        }
    }

    fn end_online(&mut self, msg: &str) {
        self.online = None;
        self.draw_offered_to_me = false;
        self.i_offered_draw = false;
        self.online_state = OnlineState::Ended(msg.to_string());
    }

    pub(super) fn online_status(&self) -> String {
        match &self.online_state {
            OnlineState::Idle => "Play online: host a game, or join one with a code".into(),
            OnlineState::Hosting => "Waiting for your opponent to join…".into(),
            OnlineState::Joining => "Looking for the game…".into(),
            OnlineState::Ended(msg) => msg.clone(),
            OnlineState::Playing => {
                let side = self.game.side_to_move();
                let check = if self.game.position().in_check() { " — check!" } else { "" };
                if side == self.human {
                    format!("Your move ({}){check}", side.name())
                } else {
                    format!("Opponent's move ({}){check}", side.name())
                }
            }
        }
    }

    /// The Online section of the side panel.
    pub(super) fn online_panel(&mut self, ui: &mut Ui) {
        match self.online_state.clone() {
            OnlineState::Idle | OnlineState::Ended(_) => {
                ui.label(RichText::new("Host a game").strong());
                ui.horizontal(|ui| {
                    ui.label("Play as");
                    ui.radio_value(&mut self.side_pick, SidePick::White, "White");
                    ui.radio_value(&mut self.side_pick, SidePick::Black, "Black");
                    ui.radio_value(&mut self.side_pick, SidePick::Random, "Random");
                });
                if ui.button("Host game").on_hover_text("Get a code to give your opponent").clicked() {
                    self.host_online();
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Join a game").strong());
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.online_code_input)
                            .hint_text("Code, e.g. K7QM-X2PA")
                            .desired_width(180.0),
                    );
                    let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if ui.button("Join").clicked() || enter {
                        self.join_online();
                    }
                });
                if self.game.ply_count() > 0
                    && ui.button("Game review").on_hover_text("Analyse the last game").clicked()
                {
                    self.review_current_game();
                }
            }
            OnlineState::Hosting => {
                let code = self.online_code().unwrap_or_default();
                ui.label("Give this code to your opponent:");
                ui.label(RichText::new(&code).monospace().size(32.0).strong());
                ui.horizontal(|ui| {
                    if ui.button("Copy code").clicked() {
                        ui.ctx().copy_text(code.clone());
                        self.message = Some("Code copied to clipboard".into());
                    }
                    if ui.button("Cancel").clicked() {
                        self.leave_online();
                    }
                });
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Waiting for them to join…").weak());
                });
                ui.label(
                    RichText::new(
                        "They choose Mode: Online and enter the code under Join a game \
                         (or run: chess-rs online join <code>).",
                    )
                    .small()
                    .weak(),
                );
            }
            OnlineState::Joining => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Looking for game {}…", self.online_code_input));
                });
                if ui.button("Cancel").clicked() {
                    self.leave_online();
                }
            }
            OnlineState::Playing => {
                let code = self.online_code().unwrap_or_default();
                ui.label(format!("Online game {code} · you play {}", self.human.name()));
                let over = self.game.result().is_some();
                if self.draw_offered_to_me && !over {
                    ui.label(RichText::new("Your opponent offers a draw").color(WARN).strong());
                    ui.horizontal(|ui| {
                        if ui.button("Accept draw").clicked() {
                            self.accept_draw();
                        }
                        if ui.button("Decline").clicked() {
                            self.decline_draw();
                        }
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    let label = if self.i_offered_draw { "Draw offered" } else { "Offer draw" };
                    if ui.add_enabled(!over && !self.i_offered_draw, egui::Button::new(label)).clicked() {
                        self.offer_draw();
                    }
                    if ui.add_enabled(!over, egui::Button::new("Resign")).clicked() {
                        self.resign();
                    }
                    if ui.button("Flip").on_hover_text("F").clicked() {
                        self.board.flipped = !self.board.flipped;
                    }
                    if ui.button("Leave").on_hover_text("Leave the game and disconnect").clicked() {
                        self.leave_online();
                    }
                    if over && ui.button("Game review").clicked() {
                        self.review_current_game();
                    }
                });
            }
        }
        if let Some(e) = &self.online_error {
            ui.label(RichText::new(e).color(ERROR));
        }
    }
}

/// Which colour a finished online game had us playing, for PGN names.
pub(super) fn online_names(human: Color) -> (String, String) {
    if human == Color::White { ("You".into(), "Opponent".into()) } else { ("Opponent".into(), "You".into()) }
}
