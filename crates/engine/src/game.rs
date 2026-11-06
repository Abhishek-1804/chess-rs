//! A game in progress: move history, undo, and rule-based results.

use crate::movegen::{MoveList, legal_moves};
use crate::notation::to_san;
use crate::position::{Position, START_FEN};
use crate::types::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GameResult {
    Checkmate { winner: Color },
    Resignation { winner: Color },
    Stalemate,
    FiftyMoveRule,
    ThreefoldRepetition,
    InsufficientMaterial,
}

impl GameResult {
    pub fn winner(&self) -> Option<Color> {
        match *self {
            GameResult::Checkmate { winner } | GameResult::Resignation { winner } => Some(winner),
            _ => None,
        }
    }

    /// PGN result token.
    pub fn pgn(&self) -> &'static str {
        match self.winner() {
            Some(Color::White) => "1-0",
            Some(Color::Black) => "0-1",
            None => "1/2-1/2",
        }
    }

    pub fn describe(&self) -> String {
        match *self {
            GameResult::Checkmate { winner } => format!("Checkmate — {} wins", winner.name()),
            GameResult::Resignation { winner } => format!("{} resigns — {} wins", winner.flip().name(), winner.name()),
            GameResult::Stalemate => "Draw by stalemate".into(),
            GameResult::FiftyMoveRule => "Draw by the fifty-move rule".into(),
            GameResult::ThreefoldRepetition => "Draw by threefold repetition".into(),
            GameResult::InsufficientMaterial => "Draw by insufficient material".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Game {
    /// positions[0] is the start; the last element is the current position.
    positions: Vec<Position>,
    moves: Vec<Move>,
    sans: Vec<String>,
    resigned: Option<Color>,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    pub fn new() -> Self {
        Self::from_position(Position::startpos())
    }

    pub fn from_fen(fen: &str) -> Result<Self, String> {
        Ok(Self::from_position(Position::from_fen(fen)?))
    }

    pub fn from_position(pos: Position) -> Self {
        Game { positions: vec![pos], moves: Vec::new(), sans: Vec::new(), resigned: None }
    }

    #[inline]
    pub fn position(&self) -> &Position {
        self.positions.last().expect("game always has a position")
    }

    pub fn start_position(&self) -> &Position {
        &self.positions[0]
    }

    /// Position after `n` plies (0 = start).
    pub fn position_at(&self, n: usize) -> &Position {
        &self.positions[n.min(self.moves.len())]
    }

    pub fn side_to_move(&self) -> Color {
        self.position().side_to_move()
    }

    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    pub fn san_moves(&self) -> &[String] {
        &self.sans
    }

    pub fn last_move(&self) -> Option<Move> {
        self.moves.last().copied()
    }

    pub fn ply_count(&self) -> usize {
        self.moves.len()
    }

    pub fn legal_moves(&self) -> MoveList {
        legal_moves(self.position())
    }

    /// Hashes of every position before the current one (for the search).
    pub fn history_hashes(&self) -> Vec<u64> {
        self.positions[..self.positions.len() - 1].iter().map(Position::hash).collect()
    }

    /// Play a move if it is legal and the game is not over.
    pub fn play(&mut self, m: Move) -> Result<(), String> {
        if let Some(r) = self.result() {
            return Err(format!("game is over: {}", r.describe()));
        }
        if !self.legal_moves().contains(&m) {
            return Err(format!("illegal move {m}"));
        }
        let pos = *self.position();
        self.sans.push(to_san(&pos, m));
        self.moves.push(m);
        self.positions.push(pos.make_move(m));
        Ok(())
    }

    pub fn undo(&mut self) -> Option<Move> {
        self.resigned = None;
        let m = self.moves.pop()?;
        self.sans.pop();
        self.positions.pop();
        Some(m)
    }

    pub fn resign(&mut self, side: Color) {
        if self.result().is_none() {
            self.resigned = Some(side);
        }
    }

    /// How many times the current position has occurred (including now).
    pub fn repetition_count(&self) -> usize {
        let cur = self.position();
        let window = cur.halfmove_clock() as usize;
        self.positions.iter().rev().take(window + 1).step_by(2).filter(|p| p.hash() == cur.hash()).count()
    }

    pub fn result(&self) -> Option<GameResult> {
        if let Some(side) = self.resigned {
            return Some(GameResult::Resignation { winner: side.flip() });
        }
        let pos = self.position();
        if legal_moves(pos).is_empty() {
            return Some(if pos.in_check() {
                GameResult::Checkmate { winner: pos.side_to_move().flip() }
            } else {
                GameResult::Stalemate
            });
        }
        if pos.insufficient_material() {
            return Some(GameResult::InsufficientMaterial);
        }
        if self.repetition_count() >= 3 {
            return Some(GameResult::ThreefoldRepetition);
        }
        if pos.halfmove_clock() >= 100 {
            return Some(GameResult::FiftyMoveRule);
        }
        None
    }

    /// Export as PGN.
    pub fn to_pgn(&self, white: &str, black: &str) -> String {
        let result = self.result().map_or("*", |r| r.pgn());
        let start = self.start_position();
        let mut out = String::new();
        out.push_str("[Event \"chess-rs game\"]\n[Site \"local\"]\n");
        out.push_str(&format!("[White \"{white}\"]\n[Black \"{black}\"]\n[Result \"{result}\"]\n"));
        let custom = start.to_fen() != START_FEN;
        if custom {
            out.push_str(&format!("[SetUp \"1\"]\n[FEN \"{}\"]\n", start.to_fen()));
        }
        out.push('\n');
        let mut line = String::new();
        let mut number = start.fullmove_number();
        let mut side = start.side_to_move();
        for (i, san) in self.sans.iter().enumerate() {
            let token = if side == Color::White {
                format!("{number}. {san}")
            } else if i == 0 {
                format!("{number}... {san}")
            } else {
                san.clone()
            };
            if side == Color::Black {
                number += 1;
            }
            side = side.flip();
            if line.len() + token.len() + 1 > 80 {
                out.push_str(line.trim_end());
                out.push('\n');
                line.clear();
            }
            line.push_str(&token);
            line.push(' ');
        }
        line.push_str(result);
        out.push_str(&line);
        out.push('\n');
        out
    }
}
