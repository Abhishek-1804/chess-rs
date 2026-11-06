//! The chess board widget: painting, click-to-move and drag-and-drop.

use chess_engine::types::{bb, file_of, make_sq, rank_of};
use chess_engine::{Color, Move, PieceType, Position, Square};
use egui::{Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};

use crate::pieces::PieceImages;

pub const LIGHT: Color32 = Color32::from_rgb(238, 216, 182);
pub const DARK: Color32 = Color32::from_rgb(178, 132, 98);
const LAST_MOVE: Color32 = Color32::from_rgba_unmultiplied_const(205, 210, 60, 115);
const SELECTED: Color32 = Color32::from_rgba_unmultiplied_const(20, 90, 40, 120);
const HOVER: Color32 = Color32::from_rgba_unmultiplied_const(255, 255, 255, 50);
const MARKER: Color32 = Color32::from_rgba_unmultiplied_const(15, 35, 15, 70);
const HINT: Color32 = Color32::from_rgba_unmultiplied_const(30, 110, 200, 200);

/// Interaction state that persists between frames.
#[derive(Debug)]
pub struct BoardState {
    pub selected: Option<Square>,
    pub dragging: Option<Square>,
    /// Last pointer position seen while dragging (robust drop target).
    pub drag_pos: Option<Pos2>,
    pub flipped: bool,
    /// Screen rect of the board in the last frame (used by tests and overlays).
    pub rect: Rect,
}

impl Default for BoardState {
    fn default() -> Self {
        BoardState { selected: None, dragging: None, drag_pos: None, flipped: false, rect: Rect::NOTHING }
    }
}

impl BoardState {
    pub fn square_size(&self) -> f32 {
        self.rect.width() / 8.0
    }

    /// Screen rect of a square.
    pub fn square_rect(&self, sq: Square) -> Rect {
        let s = self.square_size();
        let (f, r) = (file_of(sq) as f32, rank_of(sq) as f32);
        let (col, row) = if self.flipped { (7.0 - f, r) } else { (f, 7.0 - r) };
        Rect::from_min_size(self.rect.min + vec2(col * s, row * s), Vec2::splat(s))
    }

    pub fn square_at(&self, p: Pos2) -> Option<Square> {
        if !self.rect.contains(p) {
            return None;
        }
        let s = self.square_size();
        let col = (((p.x - self.rect.min.x) / s) as u8).min(7);
        let row = (((p.y - self.rect.min.y) / s) as u8).min(7);
        Some(if self.flipped { make_sq(7 - col, row) } else { make_sq(col, 7 - row) })
    }
}

pub struct BoardView<'a> {
    pub pos: &'a Position,
    pub last_move: Option<Move>,
    /// Moves the user may make right now (empty = board is read-only).
    pub legal: &'a [Move],
    pub hint: Option<Move>,
    pub show_legal: bool,
    pub images: &'a PieceImages,
    /// Extra arrows, e.g. the engine's best move in game review.
    pub arrows: &'a [(Move, Color32)],
    /// A rating badge (square, colour, symbol) on a square's corner.
    pub badge: Option<(Square, Color32, &'a str)>,
}

/// Draw the board; returns a (from, to) pair when the user completes a move.
pub fn board_ui(ui: &mut Ui, state: &mut BoardState, view: &BoardView, size: f32) -> Option<(Square, Square)> {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    state.rect = rect;
    let s = state.square_size();
    let painter = ui.painter_at(rect.expand(2.0));
    let pos = view.pos;
    let interactive = !view.legal.is_empty();
    let has_moves = |sq: Square| view.legal.iter().any(|m| m.from() == sq);
    let is_target = |from: Square, to: Square| view.legal.iter().any(|m| m.from() == from && m.to() == to);
    let pointer = resp.interact_pointer_pos().or(ui.ctx().pointer_latest_pos());
    if state.dragging.is_some()
        && let Some(p) = pointer
    {
        state.drag_pos = Some(p);
    }

    if !interactive {
        state.selected = None;
        state.dragging = None;
    }

    // ---- interaction ----
    let mut result = None;
    if interactive {
        // Use where the button went down, not where the pointer is now: a fast
        // flick can leave the origin square before egui decides it's a drag.
        let press_origin = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started()
            && let Some(sq) = press_origin.or(resp.interact_pointer_pos()).and_then(|p| state.square_at(p))
            && has_moves(sq)
        {
            state.dragging = Some(sq);
            state.drag_pos = resp.interact_pointer_pos();
            state.selected = Some(sq);
        }
        if resp.drag_stopped()
            && let Some(from) = state.dragging.take()
        {
            let drop_at = pointer.or(state.drag_pos.take());
            if let Some(to) = drop_at.and_then(|p| state.square_at(p))
                && to != from
            {
                if is_target(from, to) {
                    result = Some((from, to));
                    state.selected = None;
                } else if has_moves(to) {
                    state.selected = Some(to);
                }
            }
        }
        if resp.clicked()
            && let Some(sq) = resp.interact_pointer_pos().and_then(|p| state.square_at(p))
        {
            match state.selected {
                Some(sel) if is_target(sel, sq) => {
                    result = Some((sel, sq));
                    state.selected = None;
                }
                Some(sel) if sel == sq => state.selected = None,
                _ => state.selected = has_moves(sq).then_some(sq),
            }
        }
        if let Some(p) = resp.hover_pos()
            && let Some(sq) = state.square_at(p)
            && (has_moves(sq) || state.dragging.is_some())
        {
            ui.ctx().set_cursor_icon(if state.dragging.is_some() { CursorIcon::Grabbing } else { CursorIcon::Grab });
        }
    }

    // ---- squares and highlights ----
    for sq in 0..64u8 {
        let r = state.square_rect(sq);
        let light = (file_of(sq) + rank_of(sq)) % 2 == 1;
        painter.rect_filled(r, 0.0, if light { LIGHT } else { DARK });
    }
    if let Some(m) = view.last_move {
        painter.rect_filled(state.square_rect(m.from()), 0.0, LAST_MOVE);
        painter.rect_filled(state.square_rect(m.to()), 0.0, LAST_MOVE);
    }
    if let Some(sel) = state.selected {
        painter.rect_filled(state.square_rect(sel), 0.0, SELECTED);
    }
    if state.dragging.is_some()
        && let Some(sq) = pointer.and_then(|p| state.square_at(p))
    {
        painter.rect_filled(state.square_rect(sq), 0.0, HOVER);
    }
    if pos.in_check() {
        let k = state.square_rect(pos.king_sq(pos.side_to_move()));
        for (i, alpha) in [(0.50, 60u8), (0.40, 90), (0.30, 140)] {
            let c = Color32::from_rgba_unmultiplied(200, 30, 20, alpha);
            painter.circle_filled(k.center(), s * i, c);
        }
    }

    // ---- coordinates ----
    let font = FontId::proportional((s * 0.17).max(9.0));
    for i in 0..8u8 {
        let file_sq = if state.flipped { make_sq(7 - i, 7) } else { make_sq(i, 0) };
        let r = state.square_rect(file_sq);
        let light = (file_of(file_sq) + rank_of(file_sq)) % 2 == 1;
        let c = if light { DARK } else { LIGHT };
        painter.text(
            r.max - vec2(s * 0.06, s * 0.03),
            Align2::RIGHT_BOTTOM,
            (b'a' + file_of(file_sq)) as char,
            font.clone(),
            c,
        );
        let rank_sq = if state.flipped { make_sq(7, i) } else { make_sq(0, i) };
        let r = state.square_rect(rank_sq);
        let light = (file_of(rank_sq) + rank_of(rank_sq)) % 2 == 1;
        let c = if light { DARK } else { LIGHT };
        painter.text(
            r.min + vec2(s * 0.06, s * 0.03),
            Align2::LEFT_TOP,
            (b'1' + rank_of(rank_sq)) as char,
            font.clone(),
            c,
        );
    }

    // ---- legal-move markers ----
    if view.show_legal
        && let Some(sel) = state.selected
    {
        for m in view.legal.iter().filter(|m| m.from() == sel) {
            let r = state.square_rect(m.to());
            if m.is_capture() {
                painter.circle_stroke(r.center(), s * 0.44, Stroke::new(s * 0.08, MARKER));
            } else {
                painter.circle_filled(r.center(), s * 0.15, MARKER);
            }
        }
    }

    // ---- pieces ----
    for sq in 0..64u8 {
        let Some(p) = pos.piece_on(sq) else { continue };
        if Some(sq) == state.dragging {
            continue;
        }
        view.images.get(p).paint_at(ui, state.square_rect(sq));
    }

    // ---- hint and review arrows ----
    if let Some(h) = view.hint {
        arrow(&painter, state.square_rect(h.from()).center(), state.square_rect(h.to()).center(), s, s * 0.14, HINT);
    }
    for &(m, c) in view.arrows {
        arrow(&painter, state.square_rect(m.from()).center(), state.square_rect(m.to()).center(), s, s * 0.14, c);
    }

    // ---- rating badge (top-right corner of the square) ----
    if let Some((sq, color, symbol)) = view.badge {
        let r = state.square_rect(sq);
        let center = r.right_top() + vec2(-s * 0.12, s * 0.12);
        painter.circle_filled(center, s * 0.17, color);
        painter.circle_stroke(center, s * 0.17, Stroke::new(1.5, Color32::WHITE));
        painter.text(center, Align2::CENTER_CENTER, symbol, FontId::proportional(s * 0.2), Color32::WHITE);
    }

    // ---- dragged piece follows the pointer ----
    if let Some(from) = state.dragging
        && let (Some(p), Some(ptr)) = (pos.piece_on(from), pointer)
    {
        let r = Rect::from_center_size(ptr, Vec2::splat(s * 1.12));
        view.images.get(p).paint_at(ui, r);
        ui.ctx().request_repaint();
    }

    painter.rect_stroke(rect, 2.0, Stroke::new(2.0, Color32::from_gray(40)), egui::StrokeKind::Outside);
    result
}

fn arrow(painter: &egui::Painter, a: Pos2, b: Pos2, s: f32, width: f32, color: Color32) {
    let dir = (b - a).normalized();
    let tip = b - dir * s * 0.18;
    painter.line_segment([a + dir * s * 0.2, tip - dir * s * 0.25], Stroke::new(width, color));
    let perp = vec2(-dir.y, dir.x);
    let head = width / 0.14;
    painter.add(egui::Shape::convex_polygon(
        vec![
            tip + dir * head * 0.12,
            tip - dir * head * 0.3 + perp * head * 0.24,
            tip - dir * head * 0.3 - perp * head * 0.24,
        ],
        color,
        Stroke::NONE,
    ));
}

/// Vertical evaluation bar; `white_frac` in [0,1] is White's share.
pub fn eval_bar(ui: &mut Ui, height: f32, white_frac: f32, label: &str, flipped: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(34.0, height), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_gray(35));
    let h = rect.height() * white_frac.clamp(0.0, 1.0);
    let white_rect = if flipped {
        Rect::from_min_size(rect.min, vec2(rect.width(), h))
    } else {
        Rect::from_min_max(pos2(rect.min.x, rect.max.y - h), rect.max)
    };
    p.rect_filled(white_rect, 3.0, Color32::from_gray(235));
    // Label sits at the far edge of whichever side is ahead, in contrast colour.
    let white_ahead = white_frac >= 0.5;
    let at_bottom = white_ahead != flipped;
    let color = if white_ahead { Color32::from_gray(30) } else { Color32::from_gray(230) };
    let (anchor, at) = if at_bottom {
        (Align2::CENTER_BOTTOM, rect.center_bottom() - vec2(0.0, 4.0))
    } else {
        (Align2::CENTER_TOP, rect.center_top() + vec2(0.0, 4.0))
    };
    p.text(at, anchor, label, FontId::proportional(10.0), color);
}

/// Promotion pieces in the order offered to the user.
pub const PROMOTION_CHOICES: [PieceType; 4] = [PieceType::Queen, PieceType::Rook, PieceType::Bishop, PieceType::Knight];

/// Does any legal move from `from` to `to` promote?
pub fn is_promotion(legal: &[Move], from: Square, to: Square) -> bool {
    legal.iter().any(|m| m.from() == from && m.to() == to && m.is_promotion())
}

pub fn find_move(legal: &[Move], from: Square, to: Square, promo: Option<PieceType>) -> Option<Move> {
    legal.iter().copied().find(|m| m.from() == from && m.to() == to && m.promotion() == promo)
}

/// Color of the piece on `sq`, if any.
pub fn color_on(pos: &Position, sq: Square) -> Option<Color> {
    (pos.occupied() & bb(sq) != 0).then(|| pos.piece_on(sq).map(|p| p.color())).flatten()
}
