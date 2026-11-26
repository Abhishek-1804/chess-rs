//! Application state and UI.

mod games_ui;
mod online_ui;
mod review_ui;

pub use online_ui::OnlineState;
pub use review_ui::{ReviewState, class_color};

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chess_engine::cache::CacheStats;
use chess_engine::notation::line_to_san;
use chess_engine::pgn::{PgnGame, utc_now};
use chess_engine::search::mate_in;
use chess_engine::{Color, Game, Move, Piece, PieceType, SearchInfo, SearchResult, Square, default_threads};
use egui::{Color32, Key, RichText, Ui, vec2};

use crate::board::{self, BoardState, BoardView, PROMOTION_CHOICES};
use crate::library::{GameLibrary, SavedGame};
use crate::online::{Network, OnlineHandle};
use crate::pieces::PieceImages;
use crate::review_worker::ReviewHandle;
use crate::worker::{EngineConfig, EngineHandle, Purpose, Request, Response, SearchJob};

pub const MAX_DEPTH: u32 = 40;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    HumanVsBot,
    BotVsBot,
    HumanVsHuman,
    /// Against another chess-rs player over the internet.
    Online,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::HumanVsBot, Mode::BotVsBot, Mode::HumanVsHuman, Mode::Online];

    pub fn label(self) -> &'static str {
        match self {
            Mode::HumanVsBot => "Human vs Bot",
            Mode::BotVsBot => "Bot vs Bot",
            Mode::HumanVsHuman => "Human vs Human",
            Mode::Online => "Online",
        }
    }

    /// Parse a `--mode` value.
    pub fn parse(s: &str) -> Result<Mode, String> {
        match s.to_lowercase().as_str() {
            "human-vs-bot" | "hvb" => Ok(Mode::HumanVsBot),
            "bot-vs-bot" | "bvb" | "selfplay" => Ok(Mode::BotVsBot),
            "human-vs-human" | "hvh" => Ok(Mode::HumanVsHuman),
            "online" => Ok(Mode::Online),
            m => Err(format!("unknown mode '{m}'")),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Mode::HumanVsBot => "Play against the engine",
            Mode::BotVsBot => "Watch the engine play itself, from a random opening",
            Mode::HumanVsHuman => "Two players on this computer",
            Mode::Online => "Play a friend on another computer: host a game and share its code, or join with one",
        }
    }
}

impl SidePick {
    /// Parse a `--color` value.
    pub fn parse(s: &str) -> Result<SidePick, String> {
        match s.to_lowercase().as_str() {
            "w" | "white" => Ok(SidePick::White),
            "b" | "black" => Ok(SidePick::Black),
            "r" | "random" => Ok(SidePick::Random),
            c => Err(format!("unknown color '{c}'")),
        }
    }
}

/// The concrete colour for a side choice (random picks either).
pub fn resolve_side(pick: SidePick) -> Color {
    match pick {
        SidePick::White => Color::White,
        SidePick::Black => Color::Black,
        SidePick::Random if crate::Rng::from_time().next_u64() & 1 == 0 => Color::White,
        SidePick::Random => Color::Black,
    }
}

/// Top-level screens.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Tab {
    Play,
    Games,
    Review,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SidePick {
    White,
    Black,
    Random,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub depth: u32,
    pub threads: usize,
    pub hash_mb: usize,
    pub cache_path: Option<PathBuf>,
    pub mode: Mode,
    pub side: SidePick,
    pub fen: Option<String>,
    /// Minimum time before a bot move is shown (so instant replies don't feel abrupt).
    pub bot_delay: Duration,
    /// Folder where every game is saved as PGN (`None` = don't save).
    pub games_dir: Option<PathBuf>,
    /// Search depth for Game Review.
    pub review_depth: u32,
    /// Network for online games (tests use a local one).
    pub network: Network,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            depth: 12,
            threads: default_threads(),
            hash_mb: 1024,
            cache_path: None,
            mode: Mode::HumanVsBot,
            side: SidePick::White,
            fen: None,
            bot_delay: Duration::from_millis(350),
            games_dir: None,
            review_depth: 14,
            network: Network::Public,
        }
    }
}

struct ActiveJob {
    id: u64,
    purpose: Purpose,
    stop: Arc<AtomicBool>,
    started: Instant,
    ply: usize,
}

/// Summary of the most recent finished search, for the info panel.
#[derive(Clone, Debug)]
pub struct LastSearch {
    pub result: SearchResult,
    pub from_cache: bool,
    pub ply: usize,
    pub purpose: Purpose,
}

pub struct ChessApp {
    pub game: Game,
    pub depth: u32,
    pub threads: usize,
    pub mode: Mode,
    pub human: Color,
    side_pick: SidePick,
    pub show_legal: bool,
    pub paused: bool,
    bot_delay: Duration,
    start_fen: Option<String>,

    engine: EngineHandle,
    job: Option<ActiveJob>,
    pub thinking: Option<SearchInfo>,
    pub last_search: Option<LastSearch>,
    pending_bot_move: Option<(Move, Instant)>,
    pub hint: Option<Move>,
    /// White-relative evaluation of the latest search, if any.
    pub eval_white: Option<i32>,

    pub board: BoardState,
    pub promotion: Option<(Square, Square)>,
    pub view_ply: Option<usize>,
    images: PieceImages,

    fen_input: String,
    pub message: Option<String>,
    pub cache_stats: CacheStats,
    pub cache_ready: bool,
    cache_path: Option<PathBuf>,
    saved_this_game: bool,
    bot_moves: u64,

    pub tab: Tab,
    pub review: Option<ReviewState>,
    pub review_depth: u32,
    reviewer: ReviewHandle,
    library: Option<GameLibrary>,
    /// File the current game is being saved to (created on its first move).
    pub record: Option<PathBuf>,
    record_tags: Vec<(String, String)>,
    pub games: Vec<SavedGame>,
    games_dirty: bool,
    confirm_delete: Option<PathBuf>,
    pgn_input: String,

    ctx: egui::Context,
    network: Network,
    online: Option<OnlineHandle>,
    pub online_state: OnlineState,
    /// The code typed into the Join box.
    pub online_code_input: String,
    pub online_error: Option<String>,
    pub draw_offered_to_me: bool,
    pub i_offered_draw: bool,
    rng: crate::Rng,
    /// The bot's last move came from the opening book (no search).
    pub last_from_book: bool,
}

fn white_relative(score: i32, side: Color) -> i32 {
    if side == Color::White { score } else { -score }
}

pub fn format_score(white_score: i32) -> String {
    match mate_in(white_score) {
        Some(0) => "mate".into(), // the game ended in checkmate
        Some(n) if n > 0 => format!("#{n}"),
        Some(n) => format!("#-{}", -n),
        None => format!("{:+.2}", white_score as f64 / 100.0),
    }
}

/// "≈ 2410 Elo" for a depth, marked when extrapolated rather than measured.
pub fn strength_label(depth: u32) -> String {
    let s = chess_engine::strength::estimated_elo(depth);
    if s.measured { format!("≈ {} Elo", s.elo) } else { format!("≈ {} Elo (estimated)", s.elo) }
}

/// 12345678 -> "12,345,678"
pub fn with_commas(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn format_count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.2}M", n as f64 / 1e6),
        _ => format!("{:.2}G", n as f64 / 1e9),
    }
}

impl ChessApp {
    pub fn new(ctx: &egui::Context, cfg: AppConfig) -> Self {
        egui_extras::install_image_loaders(ctx);
        ctx.all_styles_mut(|style| {
            use egui::{FontFamily, FontId, TextStyle};
            style.text_styles = [
                (TextStyle::Heading, FontId::new(26.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(16.0, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(16.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(15.0, FontFamily::Monospace)),
                (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
            ]
            .into();
            style.spacing.item_spacing = vec2(8.0, 7.0);
            style.spacing.button_padding = vec2(10.0, 5.0);
        });
        let wake_ctx = ctx.clone();
        let engine = EngineHandle::spawn(
            EngineConfig { hash_mb: cfg.hash_mb, threads: cfg.threads, cache_path: cfg.cache_path.clone() },
            move || wake_ctx.request_repaint(),
        );
        let wake_ctx = ctx.clone();
        let reviewer = ReviewHandle::spawn(256.min(cfg.hash_mb.max(16)), move || wake_ctx.request_repaint());
        let mut app = ChessApp {
            game: Game::new(),
            depth: cfg.depth.clamp(1, MAX_DEPTH),
            threads: cfg.threads.max(1),
            mode: cfg.mode,
            human: Color::White,
            side_pick: cfg.side,
            show_legal: true,
            paused: false,
            bot_delay: cfg.bot_delay,
            start_fen: cfg.fen.clone(),
            engine,
            job: None,
            thinking: None,
            last_search: None,
            pending_bot_move: None,
            hint: None,
            eval_white: None,
            board: BoardState::default(),
            promotion: None,
            view_ply: None,
            images: PieceImages::new(),
            fen_input: String::new(),
            message: None,
            cache_stats: CacheStats::default(),
            cache_ready: false,
            cache_path: cfg.cache_path,
            saved_this_game: false,
            bot_moves: 0,
            tab: Tab::Play,
            review: None,
            review_depth: cfg.review_depth.clamp(1, MAX_DEPTH),
            reviewer,
            library: cfg.games_dir.map(GameLibrary::new),
            record: None,
            record_tags: Vec::new(),
            games: Vec::new(),
            games_dirty: true,
            confirm_delete: None,
            pgn_input: String::new(),
            ctx: ctx.clone(),
            network: cfg.network,
            online: None,
            online_state: OnlineState::Idle,
            online_code_input: String::new(),
            online_error: None,
            draw_offered_to_me: false,
            i_offered_draw: false,
            rng: crate::Rng::from_time(),
            last_from_book: false,
        };
        app.new_game();
        if let Some(fen) = cfg.fen
            && let Err(e) = app.load_fen(&fen)
        {
            app.message = Some(format!("Invalid --fen: {e}"));
        }
        app
    }

    // ------------------------------------------------------------ queries

    pub fn is_bot(&self, c: Color) -> bool {
        match self.mode {
            Mode::HumanVsBot => c != self.human,
            Mode::BotVsBot => true,
            Mode::HumanVsHuman | Mode::Online => false,
        }
    }

    /// Live count of positions searched by the running search, if any.
    pub fn live_positions(&self) -> Option<u64> {
        self.job.as_ref().map(|_| self.engine.nodes_searched())
    }

    pub fn is_thinking(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.purpose == Purpose::BotMove)
    }

    pub fn is_reviewing(&self) -> bool {
        self.view_ply.is_some_and(|p| p < self.game.ply_count())
    }

    fn human_to_move(&self) -> bool {
        if self.mode == Mode::Online {
            return self.online_state == OnlineState::Playing
                && self.game.side_to_move() == self.human
                && self.game.result().is_none();
        }
        !self.is_bot(self.game.side_to_move()) && self.game.result().is_none()
    }

    pub fn status_text(&self) -> String {
        if let Some(r) = self.game.result() {
            return r.describe();
        }
        if self.mode == Mode::Online {
            return self.online_status();
        }
        let side = self.game.side_to_move();
        if self.is_thinking() {
            let d = self.thinking.as_ref().map_or(0, |i| i.depth);
            return format!("{} (bot) is thinking… depth {d}/{}", side.name(), self.depth);
        }
        if self.pending_bot_move.is_some() {
            return format!("{} (bot) is moving…", side.name());
        }
        if self.mode == Mode::BotVsBot && self.paused {
            return "Paused".into();
        }
        let check = if self.game.position().in_check() { " — check!" } else { "" };
        match self.mode {
            Mode::HumanVsBot if !self.is_bot(side) => format!("Your move ({}){check}", side.name()),
            _ => format!("{} to move{check}", side.name()),
        }
    }

    // ------------------------------------------------------------ engine plumbing

    fn cancel_job(&mut self) {
        if let Some(j) = self.job.take() {
            j.stop.store(true, Ordering::Relaxed);
        }
        self.pending_bot_move = None;
        self.thinking = None;
    }

    fn start_search(&mut self, purpose: Purpose) {
        self.cancel_job();
        let id = self.engine.next_id();
        let stop = Arc::new(AtomicBool::new(false));
        self.engine.send(Request::Search(Box::new(SearchJob {
            id,
            purpose,
            pos: *self.game.position(),
            history: self.game.history_hashes(),
            depth: self.depth,
            threads: self.threads,
            stop: stop.clone(),
        })));
        self.job = Some(ActiveJob { id, purpose, stop, started: Instant::now(), ply: self.game.ply_count() });
    }

    /// Stop the current bot search and play the best move found so far.
    pub fn move_now(&mut self) {
        if let Some(j) = &self.job {
            j.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn request_hint(&mut self) {
        if self.mode != Mode::Online && self.human_to_move() && !self.is_reviewing() {
            self.hint = None;
            self.start_search(Purpose::Hint);
        }
    }

    pub fn save_cache(&mut self) {
        self.engine.send(Request::SaveCache);
    }

    pub fn clear_cache(&mut self) {
        self.engine.send(Request::ClearCache);
    }

    /// Drain engine responses. Called every frame.
    pub fn poll_engine(&mut self) {
        while let Some(resp) = self.engine.try_recv() {
            match resp {
                Response::Info { id, info } => {
                    if let Some(j) = &self.job
                        && j.id == id
                    {
                        let side = self.game.position().side_to_move();
                        self.eval_white = Some(white_relative(info.score, side));
                        self.thinking = Some(info);
                    }
                }
                Response::Done { id, purpose, result, from_cache } => {
                    let Some(j) = self.job.take_if(|j| j.id == id) else { continue };
                    if j.ply != self.game.ply_count() {
                        continue; // position changed under us
                    }
                    let side = self.game.side_to_move();
                    self.thinking = None;
                    self.last_from_book = false;
                    self.eval_white = Some(white_relative(result.score, side));
                    self.last_search = Some(LastSearch { result: result.clone(), from_cache, ply: j.ply, purpose });
                    match (purpose, result.best_move) {
                        (Purpose::BotMove, Some(m)) => {
                            let ready = j.started + self.bot_delay;
                            self.pending_bot_move = Some((m, ready));
                        }
                        (Purpose::Hint, Some(m)) => self.hint = Some(m),
                        _ => {}
                    }
                }
                Response::CacheReady { stats, warm_ms } => {
                    self.cache_stats = stats;
                    self.cache_ready = true;
                    if stats.root_entries + stats.tt_entries > 0 {
                        self.message = Some(format!(
                            "Loaded cache: {} positions, {} deep entries ({warm_ms} ms)",
                            stats.root_entries, stats.tt_entries
                        ));
                    }
                }
                Response::CacheStats(stats) => self.cache_stats = stats,
                Response::CacheSaved { stats, error } => {
                    self.cache_stats = stats;
                    if let Some(e) = error {
                        self.message = Some(format!("Cache save failed: {e}"));
                    }
                }
            }
        }
    }

    /// Advance the game state machine: start bot searches, play bot moves.
    pub fn tick(&mut self) {
        self.poll_engine();
        self.poll_review();
        self.poll_online();
        if let Some((m, ready)) = self.pending_bot_move
            && Instant::now() >= ready
        {
            self.pending_bot_move = None;
            self.play_move(m);
            self.bot_moves += 1;
        }
        let side = self.game.side_to_move();
        let wants_bot = self.is_bot(side)
            && self.game.result().is_none()
            && self.job.is_none()
            && self.pending_bot_move.is_none()
            && self.promotion.is_none()
            && !(self.mode == Mode::BotVsBot && self.paused);
        if wants_bot {
            // Bots open from a random book line, so their games differ.
            match self.book_move() {
                Some(m) => {
                    self.pending_bot_move = Some((m, Instant::now() + self.bot_delay));
                    self.last_from_book = true;
                }
                None => self.start_search(Purpose::BotMove),
            }
        }
        if self.game.result().is_some() && !self.saved_this_game {
            self.saved_this_game = true;
            self.save_cache();
        }
    }

    /// A random opening-book move for the bot, in Bot vs Bot games.
    fn book_move(&mut self) -> Option<Move> {
        if self.mode != Mode::BotVsBot {
            return None;
        }
        chess_engine::book::pick(&self.game, self.rng.next_u64())
    }

    // ------------------------------------------------------------ game actions

    pub fn play_move(&mut self, m: Move) {
        let ply = self.game.ply_count();
        let mover = self.game.side_to_move();
        if self.game.play(m).is_ok() {
            if self.mode == Mode::Online && mover == self.human {
                self.send_online_move(ply, m.to_uci());
            }
            self.after_move();
        }
    }

    /// Bookkeeping after any move lands on the board.
    fn after_move(&mut self) {
        self.hint = None;
        self.board.selected = None;
        self.view_ply = None;
        self.message = None;
        self.autosave();
    }

    /// The user dropped/clicked a piece from `from` to `to`.
    pub fn user_move(&mut self, from: Square, to: Square) {
        if !self.human_to_move() {
            return;
        }
        let legal = self.game.legal_moves();
        if board::is_promotion(&legal, from, to) {
            self.promotion = Some((from, to));
        } else if let Some(m) = board::find_move(&legal, from, to, None) {
            self.cancel_job(); // a pending hint is now stale
            self.play_move(m);
        }
    }

    pub fn choose_promotion(&mut self, pt: PieceType) {
        if let Some((from, to)) = self.promotion.take()
            && let Some(m) = board::find_move(&self.game.legal_moves(), from, to, Some(pt))
        {
            self.cancel_job();
            self.play_move(m);
        }
    }

    pub fn undo(&mut self) {
        if self.mode == Mode::Online {
            return; // no take-backs against a real opponent
        }
        self.cancel_job();
        self.promotion = None;
        self.hint = None;
        self.view_ply = None;
        match self.mode {
            Mode::HumanVsBot => {
                // Take back until it's the human's turn again (at least one ply).
                if self.game.undo().is_some() {
                    while self.game.side_to_move() != self.human && self.game.ply_count() > 0 {
                        self.game.undo();
                    }
                }
            }
            Mode::BotVsBot => {
                self.paused = true;
                self.game.undo();
            }
            Mode::HumanVsHuman | Mode::Online => {
                self.game.undo();
            }
        }
        self.saved_this_game = false;
        self.autosave();
    }

    pub fn resign(&mut self) {
        if self.game.result().is_none() {
            if self.mode == Mode::Online {
                if self.online_state != OnlineState::Playing {
                    return;
                }
                if let Some(net) = &self.online {
                    net.send(crate::online::NetCmd::Resign);
                }
            }
            self.cancel_job();
            let side = if matches!(self.mode, Mode::HumanVsBot | Mode::Online) {
                self.human
            } else {
                self.game.side_to_move()
            };
            self.game.resign(side);
            self.autosave();
        }
    }

    pub fn new_game(&mut self) {
        self.leave_online();
        let game = match &self.start_fen {
            Some(f) => Game::from_fen(f).unwrap_or_default(),
            None => Game::new(),
        };
        self.begin_game(game, resolve_side(self.side_pick));
    }

    /// Start `game` with the human playing `human`.
    fn begin_game(&mut self, game: Game, human: Color) {
        self.cancel_job();
        if self.game.ply_count() > 0 && !self.saved_this_game {
            self.save_cache();
        }
        self.game = game;
        self.human = human;
        self.last_from_book = false;
        self.board.flipped = matches!(self.mode, Mode::HumanVsBot | Mode::Online) && self.human == Color::Black;
        self.board.selected = None;
        self.promotion = None;
        self.view_ply = None;
        self.hint = None;
        self.eval_white = None;
        self.last_search = None;
        self.paused = false;
        self.saved_this_game = false;
        self.start_record();
    }

    // ------------------------------------------------------------ saved games

    /// Player names for the PGN tags of the current mode.
    fn player_names(&self) -> (String, String) {
        let bot = "chess-rs".to_string();
        let human = "Human".to_string();
        match self.mode {
            Mode::HumanVsBot if self.human == Color::White => (human, bot),
            Mode::HumanVsBot => (bot, human),
            Mode::BotVsBot => (bot.clone(), bot),
            Mode::HumanVsHuman => (human.clone(), human),
            Mode::Online => online_ui::online_names(self.human),
        }
    }

    /// Begin a new game record: timestamp and players are fixed at the start.
    fn start_record(&mut self) {
        self.record = None;
        let (date, time) = utc_now();
        let (white, black) = self.player_names();
        self.record_tags = vec![
            ("Event".into(), if self.mode == Mode::Online { "chess-rs online game" } else { "chess-rs game" }.into()),
            ("Site".into(), "chess-rs".into()),
            ("Date".into(), date),
            ("Round".into(), "-".into()),
            ("White".into(), white),
            ("Black".into(), black),
            ("Time".into(), time),
            ("Mode".into(), self.mode.label().into()),
        ];
    }

    /// The current game as PGN, with tags.
    pub fn current_pgn(&self) -> PgnGame {
        let mut pgn = PgnGame::from_game(&self.game);
        pgn.tags = self.record_tags.clone();
        if matches!(self.mode, Mode::HumanVsBot | Mode::BotVsBot) {
            pgn.set_tag("BotDepth", self.depth.to_string());
        }
        pgn.set_tag("PlyCount", self.game.ply_count().to_string());
        match self.game.result() {
            Some(r) => pgn.set_tag("Termination", r.describe()),
            None => pgn.set_tag("Termination", "unfinished"),
        }
        pgn
    }

    /// Save the current game after every change (no file for empty games).
    pub fn autosave(&mut self) {
        let Some(lib) = &self.library else { return };
        if self.game.ply_count() == 0 {
            if let Some(path) = self.record.take() {
                let _ = lib.delete(&path);
                self.games_dirty = true;
            }
            return;
        }
        let pgn = self.current_pgn();
        let path = self.record.get_or_insert_with(|| lib.new_path(&pgn)).clone();
        if let Err(e) = lib.save(&path, &pgn) {
            self.message = Some(format!("Could not save game: {e}"));
        }
        self.games_dirty = true;
    }

    pub fn load_fen(&mut self, fen: &str) -> Result<(), String> {
        let g = Game::from_fen(fen.trim())?;
        self.start_fen = Some(fen.trim().to_string());
        self.new_game();
        self.game = g;
        Ok(())
    }

    pub fn set_side(&mut self, s: SidePick) {
        self.side_pick = s;
    }

    pub fn set_mode(&mut self, m: Mode) {
        if self.mode == Mode::Online && m != Mode::Online {
            self.leave_online();
        }
        if m == Mode::Online {
            self.cancel_job();
        }
        self.mode = m;
    }

    // ------------------------------------------------------------ UI

    fn handle_keys(&mut self, ui: &Ui) {
        if self.tab == Tab::Review {
            self.review_keys(ui);
            return;
        }
        if self.tab != Tab::Play || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (undo, flip, hint, new, left, right, home, end, esc, now) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, Key::Z),
                i.consume_key(egui::Modifiers::NONE, Key::F),
                i.consume_key(egui::Modifiers::NONE, Key::H),
                i.consume_key(egui::Modifiers::COMMAND, Key::N),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowRight),
                i.consume_key(egui::Modifiers::NONE, Key::Home),
                i.consume_key(egui::Modifiers::NONE, Key::End),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
                i.consume_key(egui::Modifiers::NONE, Key::Space),
            )
        });
        if undo {
            self.undo();
        }
        if flip {
            self.board.flipped = !self.board.flipped;
        }
        if hint {
            self.request_hint();
        }
        if new && !self.online_active() {
            self.new_game();
        }
        let n = self.game.ply_count();
        let cur = self.view_ply.unwrap_or(n);
        if left {
            self.view_ply = Some(cur.saturating_sub(1));
        }
        if right {
            self.view_ply = if cur + 1 >= n { None } else { Some(cur + 1) };
        }
        if home {
            self.view_ply = Some(0);
        }
        if end {
            self.view_ply = None;
        }
        if esc {
            self.board.selected = None;
            self.promotion = None;
        }
        if now && self.is_thinking() {
            self.move_now();
        }
    }

    /// Entire UI. Used by the eframe app and by the headless test harness.
    pub fn draw(&mut self, ui: &mut Ui) {
        self.tick();
        self.handle_keys(ui);

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("chess-rs").size(20.0).strong());
                ui.add_space(16.0);
                let before = self.tab;
                ui.selectable_value(&mut self.tab, Tab::Play, RichText::new("Play").size(17.0));
                ui.selectable_value(&mut self.tab, Tab::Games, RichText::new("Games").size(17.0));
                ui.selectable_value(&mut self.tab, Tab::Review, RichText::new("Game Review").size(17.0));
                if self.tab != before && self.tab == Tab::Games {
                    self.refresh_games();
                }
            });
        });
        match self.tab {
            Tab::Play => {
                egui::Panel::right("side_panel").resizable(false).exact_size(390.0).show(ui, |ui| {
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.side_panel(ui));
                });
                egui::CentralPanel::default().show(ui, |ui| self.board_area(ui));
            }
            Tab::Games => {
                egui::CentralPanel::default().show(ui, |ui| self.games_tab(ui));
            }
            Tab::Review => {
                egui::Panel::right("review_panel").resizable(false).exact_size(390.0).show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("review_scroll")
                        .auto_shrink(false)
                        .show(ui, |ui| self.review_side_panel(ui));
                });
                egui::CentralPanel::default().show(ui, |ui| self.review_board_area(ui));
            }
        }
        if self.review.as_ref().is_some_and(|r| r.is_running()) {
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        }

        // Live "positions searched" counter while the engine works.
        if self.job.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some((_, ready)) = self.pending_bot_move {
            ui.ctx().request_repaint_after(ready.saturating_duration_since(Instant::now()));
        }
    }

    fn board_area(&mut self, ui: &mut Ui) {
        let avail = ui.available_size();
        let nav_h = 34.0;
        let size = (avail.x - 52.0).min(avail.y - nav_h - 8.0).max(160.0);

        let n = self.game.ply_count();
        let shown_ply = self.view_ply.unwrap_or(n).min(n);
        let pos = *self.game.position_at(shown_ply);
        let last_move = shown_ply.checked_sub(1).map(|i| self.game.moves()[i]);
        let interactive = shown_ply == n && self.human_to_move() && self.promotion.is_none();
        let legal = if interactive { self.game.legal_moves().to_vec() } else { Vec::new() };
        ui.vertical_centered(|ui| {
            ui.horizontal(|ui| {
                ui.add_space(((avail.x - size - 44.0) / 2.0).max(0.0));
                let (frac, label) = match self.eval_white {
                    Some(s) => (
                        match mate_in(s) {
                            Some(m) => {
                                if m > 0 {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            None => 1.0 / (1.0 + (-(s as f32) / 400.0).exp()),
                        },
                        format_score(s),
                    ),
                    None => (0.5, String::new()),
                };
                board::eval_bar(ui, size, frac, &label, self.board.flipped);
                ui.add_space(6.0);
                let view = BoardView {
                    pos: &pos,
                    last_move,
                    legal: &legal,
                    hint: if shown_ply == n { self.hint } else { None },
                    show_legal: self.show_legal,
                    images: &self.images,
                    arrows: &[],
                    badge: None,
                };
                if let Some((from, to)) = board::board_ui(ui, &mut self.board, &view, size) {
                    self.user_move(from, to);
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(((avail.x - 260.0) / 2.0).max(0.0));
                if ui.button("«").on_hover_text("First position (Home)").clicked() {
                    self.view_ply = Some(0);
                }
                if ui.button("‹").on_hover_text("Previous move (Left arrow)").clicked() {
                    self.view_ply = Some(shown_ply.saturating_sub(1));
                }
                let label = if shown_ply == n { format!("ply {n}") } else { format!("ply {shown_ply}/{n}") };
                ui.label(RichText::new(label).monospace());
                if ui.button("›").on_hover_text("Next move (Right arrow)").clicked() {
                    self.view_ply = if shown_ply + 1 >= n { None } else { Some(shown_ply + 1) };
                }
                if ui.button("»").on_hover_text("Current position (End)").clicked() {
                    self.view_ply = None;
                }
            });
        });

        self.overlays(ui);
    }

    fn overlays(&mut self, ui: &mut Ui) {
        let rect = self.board.rect;
        if let Some((_, to)) = self.promotion {
            let color = self.game.side_to_move();
            let sq = self.board.square_rect(to);
            let size = self.board.square_size();
            egui::Area::new(egui::Id::unique("chess-rs-promotion"))
                .order(egui::Order::Foreground)
                .fixed_pos(egui::pos2(
                    sq.min.x.clamp(rect.min.x, rect.max.x - size * 1.1),
                    rect.center().y - size * 2.2,
                ))
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(RichText::new("Promote to").strong());
                        for pt in PROMOTION_CHOICES {
                            let img = self
                                .images
                                .get(Piece::new(color, pt))
                                .clone()
                                .fit_to_exact_size(vec2(size * 0.8, size * 0.8));
                            let name = match pt {
                                PieceType::Queen => "Queen",
                                PieceType::Rook => "Rook",
                                PieceType::Bishop => "Bishop",
                                _ => "Knight",
                            };
                            if ui.add(egui::Button::image_and_text(img, name)).clicked() {
                                self.choose_promotion(pt);
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.promotion = None;
                        }
                    });
                });
        }
        if let Some(r) = self.game.result()
            && !self.is_reviewing()
        {
            let painter = ui
                .ctx()
                .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("chess-rs-result")));
            let banner = egui::Rect::from_center_size(rect.center(), vec2(rect.width() * 0.8, 64.0));
            painter.rect_filled(banner, 10.0, Color32::from_black_alpha(200));
            painter.text(
                banner.center(),
                egui::Align2::CENTER_CENTER,
                r.describe(),
                egui::FontId::proportional(24.0),
                Color32::WHITE,
            );
        }
    }

    fn side_panel(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        let status_color = if self.game.result().is_some() {
            Color32::from_rgb(230, 170, 60)
        } else {
            ui.visuals().strong_text_color()
        };
        ui.label(RichText::new(self.status_text()).size(16.0).color(status_color));
        // Always two lines (live while searching, else the last search), so the
        // panel doesn't jump as searches start and stop.
        if self.mode != Mode::Online {
            let (what, n, secs) = match (self.live_positions(), &self.job, &self.last_search) {
                (Some(n), Some(job), _) => {
                    let what =
                        if job.purpose == Purpose::Hint { "Hint: positions searched" } else { "Positions searched" };
                    (what, n, job.started.elapsed().as_secs_f64())
                }
                (_, _, Some(l)) => ("Last search", l.result.nodes, l.result.time.as_secs_f64()),
                _ => ("Positions searched", 0, 0.0),
            };
            ui.label(RichText::new(format!("{what}: {}", with_commas(n))).size(18.0).strong().monospace());
            ui.label(
                RichText::new(format!("{}/s · {:.1}s", with_commas((n as f64 / secs.max(1e-3)) as u64), secs))
                    .monospace()
                    .weak(),
            );
        }
        if let Some(m) = &self.message {
            ui.label(RichText::new(m).small().weak());
        }
        ui.separator();

        // ---- new game ----
        ui.label(RichText::new("Game setup").strong());
        let mut mode = self.mode;
        ui.horizontal_wrapped(|ui| {
            for m in Mode::ALL {
                ui.selectable_value(&mut mode, m, m.label()).on_hover_text(m.description());
            }
        });
        if mode != self.mode {
            self.set_mode(mode);
        }
        if self.mode == Mode::Online {
            self.online_panel(ui);
            ui.separator();
            ui.label(RichText::new("Moves").strong());
            self.move_list(ui);
            ui.separator();
            self.position_section(ui);
            return;
        }
        if self.mode == Mode::HumanVsBot {
            ui.horizontal(|ui| {
                ui.label("Play as");
                ui.radio_value(&mut self.side_pick, SidePick::White, "White");
                ui.radio_value(&mut self.side_pick, SidePick::Black, "Black");
                ui.radio_value(&mut self.side_pick, SidePick::Random, "Random");
            });
        }
        ui.horizontal(|ui| {
            if ui.button("New game").on_hover_text("Ctrl+N").clicked() {
                self.new_game();
            }
            if ui
                .add_enabled(self.game.ply_count() > 0, egui::Button::new("Game review"))
                .on_hover_text("Analyse this game: accuracy, mistakes, best moves")
                .clicked()
            {
                self.review_current_game();
            }
        });
        ui.separator();

        // ---- strength ----
        ui.label(RichText::new("Bot").strong());
        ui.add(egui::Slider::new(&mut self.depth, 1..=MAX_DEPTH).text("depth (plies)"));
        ui.label(RichText::new(strength_label(self.depth)).strong()).on_hover_text(
            "Estimated rating at this depth, from games against Stockfish at calibrated strengths \
             (CCRL scale; a rough guide to human ratings). Deeper depths are extrapolated.",
        );
        let max_threads = default_threads().max(self.threads);
        ui.add(egui::Slider::new(&mut self.threads, 1..=max_threads).text("threads"));
        if self.depth >= 22 {
            ui.label(
                RichText::new("Deep search: moves may take a long time. Use Move now to cut it short.").small().weak(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            let thinking = self.is_thinking();
            if ui
                .add_enabled(thinking, egui::Button::new("Move now"))
                .on_hover_text("Play the best move found so far (Space)")
                .clicked()
            {
                self.move_now();
            }
            if ui.add_enabled(self.game.ply_count() > 0, egui::Button::new("Undo")).on_hover_text("Ctrl+Z").clicked() {
                self.undo();
            }
            if ui.add_enabled(self.human_to_move(), egui::Button::new("Hint")).on_hover_text("H").clicked() {
                self.request_hint();
            }
            if ui.button("Flip").on_hover_text("F").clicked() {
                self.board.flipped = !self.board.flipped;
            }
            if ui
                .add_enabled(self.game.result().is_none() && self.mode != Mode::BotVsBot, egui::Button::new("Resign"))
                .clicked()
            {
                self.resign();
            }
            if self.mode == Mode::BotVsBot {
                let label = if self.paused { "Resume" } else { "Pause" };
                if ui.button(label).clicked() {
                    self.paused = !self.paused;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_legal, "Show legal moves");
            ui.checkbox(&mut self.board.animate, "Animate moves");
        });
        ui.separator();

        // ---- engine info ----
        ui.label(RichText::new("Engine").strong());
        self.engine_info(ui);
        ui.separator();

        // ---- moves ----
        ui.label(RichText::new("Moves").strong());
        self.move_list(ui);
        ui.separator();

        self.position_section(ui);

        egui::CollapsingHeader::new("Position cache").default_open(true).show(ui, |ui| {
            let s = self.cache_stats;
            if !self.cache_ready {
                ui.label("Loading cache…");
            }
            egui::Grid::new("cache_grid").num_columns(2).show(ui, |ui| {
                ui.label("Searched positions");
                ui.monospace(s.root_entries.to_string());
                ui.end_row();
                ui.label("Deep TT entries");
                ui.monospace(s.tt_entries.to_string());
                ui.end_row();
                ui.label("Instant replies");
                ui.monospace(s.hits.to_string());
                ui.end_row();
            });
            match &self.cache_path {
                Some(p) => ui.label(RichText::new(p.display().to_string()).small().weak()),
                None => ui.label(RichText::new("in-memory only (--no-cache)").small().weak()),
            };
            ui.horizontal(|ui| {
                if ui.button("Save now").clicked() {
                    self.save_cache();
                    self.message = Some("Cache saved".into());
                }
                if ui.button("Clear cache").clicked() {
                    self.clear_cache();
                    self.message = Some("Cache cleared".into());
                }
            });
        });
    }

    fn position_section(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new("Position (FEN / PGN)").default_open(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Copy FEN").clicked() {
                    ui.ctx().copy_text(self.game.position().to_fen());
                    self.message = Some("FEN copied to clipboard".into());
                }
                if ui.button("Copy PGN").clicked() {
                    ui.ctx().copy_text(self.current_pgn().to_pgn());
                    self.message = Some("PGN copied to clipboard".into());
                }
            });
            ui.add(
                egui::TextEdit::singleline(&mut self.fen_input).hint_text("Paste a FEN…").desired_width(f32::INFINITY),
            );
            // An online game's position is fixed once hosted or joined.
            if ui.add_enabled(!self.online_active(), egui::Button::new("Load FEN")).clicked() {
                let fen = self.fen_input.clone();
                match self.load_fen(&fen) {
                    Ok(()) => self.message = Some("Position loaded".into()),
                    Err(e) => self.message = Some(format!("Invalid FEN: {e}")),
                }
            }
        });
    }

    fn engine_info(&self, ui: &mut Ui) {
        if self.last_from_book && self.thinking.is_none() {
            ui.label(RichText::new("Opening book move (no search)").weak());
            return;
        }
        let base_ply = self.job.as_ref().map(|j| j.ply).or(self.last_search.as_ref().map(|l| l.ply));
        let base = base_ply.map(|p| *self.game.position_at(p));
        let (depth, seldepth, score, nodes, nps, time, pv, source, hashfull) = if let Some(i) = &self.thinking {
            let label =
                if self.job.as_ref().is_some_and(|j| j.purpose == Purpose::Hint) { "hint search" } else { "searching" };
            (i.depth, i.seldepth, i.score, i.nodes, i.nps(), i.time, i.pv.clone(), label, Some(i.hashfull))
        } else if let Some(l) = &self.last_search {
            let r = &l.result;
            let src = if l.from_cache { "position cache (instant)" } else { "search" };
            (r.depth, r.seldepth, r.score, r.nodes, r.nps(), r.time, r.pv.clone(), src, None)
        } else {
            ui.label(RichText::new("No search yet").weak());
            return;
        };
        let side = base.map_or(Color::White, |p| p.side_to_move());
        // While searching, show the live totals rather than the last finished depth's.
        let (nodes, nps, time) = match (self.live_positions(), &self.job) {
            (Some(n), Some(j)) => {
                let t = j.started.elapsed();
                (n, (n as f64 / t.as_secs_f64().max(1e-3)) as u64, t)
            }
            _ => (nodes, nps, time),
        };
        egui::Grid::new("engine_grid").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
            ui.label("Depth");
            ui.monospace(format!("{depth} / sel {seldepth}"));
            ui.end_row();
            ui.label("Score");
            ui.monospace(format_score(white_relative(score, side)));
            ui.end_row();
            ui.label("Nodes");
            ui.monospace(format_count(nodes));
            ui.end_row();
            ui.label("Speed");
            ui.monospace(format!("{}n/s", format_count(nps)));
            ui.end_row();
            ui.label("Time");
            ui.monospace(format!("{:.2}s", time.as_secs_f64()));
            ui.end_row();
            if let Some(h) = hashfull {
                ui.label("Hash");
                ui.monospace(format!("{:.1}%", h as f64 / 10.0));
                ui.end_row();
            }
            ui.label("Source");
            ui.monospace(source);
            ui.end_row();
        });
        if let Some(b) = base {
            let sans = line_to_san(&b, &pv);
            if !sans.is_empty() {
                ui.add(egui::Label::new(RichText::new(format!("PV: {}", sans.join(" "))).monospace().small()).wrap());
            }
        }
    }

    fn move_list(&mut self, ui: &mut Ui) {
        let start = *self.game.start_position();
        let sans = self.game.san_moves().to_vec();
        let shown = self.view_ply.unwrap_or(sans.len());
        egui::ScrollArea::vertical()
            .id_salt("moves")
            .max_height(220.0)
            .stick_to_bottom(true)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("moves_grid").num_columns(3).striped(true).min_col_width(60.0).show(ui, |ui| {
                    let mut number = start.fullmove_number();
                    let mut i = 0;
                    if start.side_to_move() == Color::Black && !sans.is_empty() {
                        ui.monospace(format!("{number}."));
                        ui.monospace("…");
                        if ui.selectable_label(shown == 1, &sans[0]).clicked() {
                            self.view_ply = Some(1);
                        }
                        ui.end_row();
                        number += 1;
                        i = 1;
                    }
                    while i < sans.len() {
                        ui.monospace(format!("{number}."));
                        for k in 0..2 {
                            if let Some(s) = sans.get(i + k) {
                                let ply = i + k + 1;
                                if ui.selectable_label(shown == ply, s).clicked() {
                                    self.view_ply = if ply == sans.len() { None } else { Some(ply) };
                                }
                            }
                        }
                        ui.end_row();
                        number += 1;
                        i += 2;
                    }
                });
            });
        if let Some(r) = self.game.result() {
            ui.label(RichText::new(r.pgn()).monospace().strong());
        }
    }

    /// Stop the engine thread, persisting the cache.
    pub fn shutdown(&mut self) {
        if let Some(mut net) = self.online.take() {
            net.shutdown();
        }
        self.cancel_job();
        self.engine.shutdown();
    }
}

impl eframe::App for ChessApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        self.shutdown();
    }
}
