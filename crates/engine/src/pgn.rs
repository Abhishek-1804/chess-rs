//! PGN (Portable Game Notation) reading and writing.
//!
//! The reader handles real-world files: multiple games, tag pairs with
//! escapes, `{}` and `;` comments, nested `( )` variations (skipped), NAGs
//! (`$2`) and move-suffix annotations (`!?`), `%` escape lines, move numbers
//! with or without spaces (`12.e4`, `12...e5`), and custom starting positions
//! (`[SetUp "1"] [FEN "..."]`). Per-move comments and NAGs are preserved, which
//! is how game analysis (`[%eval ...]`) is stored.

use std::fmt::Write as _;

use crate::game::Game;
use crate::notation::{parse_move, to_san};
use crate::position::{Position, START_FEN};
use crate::types::{Color, Move};

/// A game as stored in PGN: tags, start position, moves and annotations.
#[derive(Clone, Debug, PartialEq)]
pub struct PgnGame {
    pub tags: Vec<(String, String)>,
    pub start: Position,
    pub moves: Vec<Move>,
    /// Comment after each ply (empty string = none).
    pub comments: Vec<String>,
    /// Numeric annotation glyph after each ply (0 = none).
    pub nags: Vec<u8>,
    /// Comment before the first move.
    pub pre_comment: String,
    /// `1-0`, `0-1`, `1/2-1/2` or `*`.
    pub result: String,
}

/// The Seven Tag Roster, written first and in this order.
const ROSTER: [&str; 7] = ["Event", "Site", "Date", "Round", "White", "Black", "Result"];

impl PgnGame {
    pub fn new(start: Position) -> Self {
        PgnGame {
            tags: Vec::new(),
            start,
            moves: Vec::new(),
            comments: Vec::new(),
            nags: Vec::new(),
            pre_comment: String::new(),
            result: "*".into(),
        }
    }

    /// Build from a played game (moves and result; no tags yet).
    pub fn from_game(game: &Game) -> Self {
        let mut p = PgnGame::new(*game.start_position());
        p.moves = game.moves().to_vec();
        p.comments = vec![String::new(); p.moves.len()];
        p.nags = vec![0; p.moves.len()];
        p.result = game.result().map_or("*", |r| r.pgn()).to_string();
        p
    }

    /// Replay into a [`Game`] (moves were validated when parsed).
    pub fn to_game(&self) -> Game {
        let mut g = Game::from_position(self.start);
        for &m in &self.moves {
            if g.play(m).is_err() {
                break;
            }
        }
        g
    }

    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn set_tag(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.tags.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.tags.push((key.to_string(), value)),
        }
    }

    /// Serialise as PGN text (one game, trailing newline).
    pub fn to_pgn(&self) -> String {
        let mut out = String::new();
        let mut tags = self.tags.clone();
        let set = |tags: &mut Vec<(String, String)>, k: &str, v: String| match tags.iter_mut().find(|(t, _)| t == k) {
            Some(slot) => slot.1 = v,
            None => tags.push((k.into(), v)),
        };
        set(&mut tags, "Result", self.result.clone());
        if self.start.to_fen() != START_FEN {
            set(&mut tags, "SetUp", "1".into());
            set(&mut tags, "FEN", self.start.to_fen());
        }
        for key in ROSTER {
            // Missing roster tags are written as "?", the PGN "unknown" value.
            let v = tags.iter().find(|(k, _)| k == key).map_or("?", |(_, v)| v.as_str());
            writeln!(out, "[{key} \"{}\"]", escape(v)).unwrap();
        }
        for (k, v) in tags.iter().filter(|(k, _)| !ROSTER.contains(&k.as_str())) {
            writeln!(out, "[{k} \"{}\"]", escape(v)).unwrap();
        }
        out.push('\n');

        let mut tokens: Vec<String> = Vec::new();
        if !self.pre_comment.is_empty() {
            tokens.push(format!("{{{}}}", self.pre_comment));
        }
        let mut pos = self.start;
        let mut number = pos.fullmove_number();
        let mut need_number = true;
        for (i, &m) in self.moves.iter().enumerate() {
            let white = pos.side_to_move() == Color::White;
            if white {
                tokens.push(format!("{number}."));
            } else if need_number {
                tokens.push(format!("{number}..."));
            }
            tokens.push(to_san(&pos, m));
            need_number = false;
            if let Some(&nag) = self.nags.get(i)
                && nag != 0
            {
                tokens.push(format!("${nag}"));
            }
            if let Some(c) = self.comments.get(i)
                && !c.is_empty()
            {
                tokens.push(format!("{{{c}}}"));
                need_number = true;
            }
            if !white {
                number += 1;
            }
            pos = pos.make_move(m);
        }
        tokens.push(self.result.clone());

        // Wrap movetext at 80 columns.
        let mut line = String::new();
        for t in tokens {
            if !line.is_empty() && line.len() + 1 + t.len() > 80 {
                out.push_str(&line);
                out.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&t);
        }
        out.push_str(&line);
        out.push('\n');
        out
    }
}

fn escape(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Map a move-suffix annotation to its NAG.
fn suffix_nag(s: &str) -> u8 {
    match s {
        "!" => 1,
        "?" => 2,
        "!!" => 3,
        "??" => 4,
        "!?" => 5,
        "?!" => 6,
        _ => 0,
    }
}

/// Parse every game in a PGN text.
pub fn parse_pgn(text: &str) -> Result<Vec<PgnGame>, String> {
    let mut games = Vec::new();
    let mut p = Parser { chars: text.chars().collect(), i: 0, line: 1 };
    loop {
        p.skip_ws();
        if p.done() {
            break;
        }
        match p.parse_game()? {
            Some(g) => games.push(g),
            None => break,
        }
    }
    Ok(games)
}

/// Parse exactly one game (the first in the text).
pub fn parse_one(text: &str) -> Result<PgnGame, String> {
    parse_pgn(text)?.into_iter().next().ok_or_else(|| "no game found".to_string())
}

struct Parser {
    chars: Vec<char>,
    i: usize,
    line: usize,
}

impl Parser {
    fn done(&self) -> bool {
        self.i >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn at_line_start(&self) -> bool {
        self.i == 0 || self.chars[self.i - 1] == '\n'
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump();
            } else if c == '%' && self.at_line_start() {
                self.skip_line();
            } else {
                break;
            }
        }
    }

    fn skip_line(&mut self) {
        while let Some(c) = self.bump() {
            if c == '\n' {
                break;
            }
        }
    }

    fn err(&self, msg: impl std::fmt::Display) -> String {
        format!("PGN line {}: {msg}", self.line)
    }

    fn read_tag(&mut self) -> Result<(String, String), String> {
        self.bump(); // '['
        self.skip_ws();
        let mut key = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == '"' || c == ']' {
                break;
            }
            key.push(c);
            self.bump();
        }
        self.skip_ws();
        if self.bump() != Some('"') {
            return Err(self.err(format!("tag {key} has no quoted value")));
        }
        let mut value = String::new();
        loop {
            match self.bump() {
                Some('\\') => {
                    if let Some(c) = self.bump() {
                        value.push(c);
                    }
                }
                Some('"') => break,
                Some(c) => value.push(c),
                None => return Err(self.err("unterminated tag value")),
            }
        }
        while let Some(c) = self.bump() {
            if c == ']' {
                break;
            }
        }
        Ok((key, value))
    }

    fn read_brace_comment(&mut self) -> String {
        self.bump(); // '{'
        let mut s = String::new();
        while let Some(c) = self.bump() {
            if c == '}' {
                break;
            }
            s.push(c);
        }
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn skip_variation(&mut self) -> Result<(), String> {
        let mut depth = 0;
        while let Some(c) = self.peek() {
            match c {
                '(' => {
                    depth += 1;
                    self.bump();
                }
                ')' => {
                    depth -= 1;
                    self.bump();
                    if depth == 0 {
                        return Ok(());
                    }
                }
                '{' => {
                    self.read_brace_comment();
                }
                ';' => self.skip_line(),
                _ => {
                    self.bump();
                }
            }
        }
        Err(self.err("unterminated variation"))
    }

    fn read_token(&mut self) -> String {
        let mut t = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, '{' | '}' | '(' | ')' | ';' | '[' | ']' | '$') {
                break;
            }
            t.push(c);
            self.bump();
        }
        t
    }

    fn parse_game(&mut self) -> Result<Option<PgnGame>, String> {
        let mut tags = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some('[') {
                tags.push(self.read_tag()?);
            } else {
                break;
            }
        }
        if tags.is_empty() && self.done() {
            return Ok(None);
        }
        let start = match tags.iter().find(|(k, _)| k == "FEN") {
            Some((_, fen)) => Position::from_fen(fen).map_err(|e| self.err(format!("bad FEN tag: {e}")))?,
            None => Position::startpos(),
        };
        let mut game = PgnGame::new(start);
        game.tags = tags;
        let mut pos = start;

        loop {
            self.skip_ws();
            let Some(c) = self.peek() else { break };
            match c {
                '[' => break, // next game's tags without a result token
                '{' => {
                    let comment = self.read_brace_comment();
                    attach_comment(&mut game, comment);
                }
                ';' => {
                    self.bump();
                    let mut s = String::new();
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        s.push(c);
                        self.bump();
                    }
                    attach_comment(&mut game, s.trim().to_string());
                }
                '(' => self.skip_variation()?,
                ')' | '}' | ']' => {
                    self.bump();
                }
                '$' => {
                    self.bump();
                    let n = self.read_token();
                    if let (Ok(v), Some(last)) = (n.parse::<u8>(), game.nags.last_mut()) {
                        *last = v;
                    }
                }
                _ => {
                    let tok = self.read_token();
                    if tok.is_empty() {
                        self.bump();
                        continue;
                    }
                    if matches!(tok.as_str(), "1-0" | "0-1" | "1/2-1/2" | "*") {
                        game.result = tok;
                        break;
                    }
                    // Strip a leading move number: "12.", "12...", "12.e4".
                    let san = match tok.find(|c: char| !c.is_ascii_digit()) {
                        Some(k) if k > 0 && tok[k..].starts_with('.') => tok[k..].trim_start_matches('.'),
                        _ => tok.as_str(),
                    };
                    if san.is_empty() {
                        continue;
                    }
                    let core = san.trim_end_matches(['!', '?']);
                    let nag = suffix_nag(&san[core.len()..]);
                    let m = parse_move(&pos, core).map_err(|e| {
                        self.err(format!(
                            "move {} ({core}): {e}",
                            game.moves.len() / 2 + pos.fullmove_number() as usize
                        ))
                    })?;
                    pos = pos.make_move(m);
                    game.moves.push(m);
                    game.comments.push(String::new());
                    game.nags.push(nag);
                }
            }
        }
        if game.result == "*"
            && let Some(r) = game.tag("Result")
            && matches!(r, "1-0" | "0-1" | "1/2-1/2")
        {
            game.result = r.to_string();
        }
        Ok(Some(game))
    }
}

fn attach_comment(game: &mut PgnGame, comment: String) {
    if comment.is_empty() {
        return;
    }
    let slot = match game.comments.last_mut() {
        Some(c) => c,
        None => &mut game.pre_comment,
    };
    if slot.is_empty() {
        *slot = comment;
    } else {
        slot.push(' ');
        slot.push_str(&comment);
    }
}

/// Read a `[%cmd value]` command embedded in a comment (lichess/chess.com style).
pub fn comment_command<'a>(comment: &'a str, cmd: &str) -> Option<&'a str> {
    let needle = format!("[%{cmd} ");
    let start = comment.find(&needle)? + needle.len();
    let end = comment[start..].find(']')? + start;
    Some(comment[start..end].trim())
}

/// Current UTC date and time as PGN strings: ("YYYY.MM.DD", "HH:MM:SS").
pub fn utc_now() -> (String, String) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    utc_from_unix(secs)
}

/// Unix seconds to UTC ("YYYY.MM.DD", "HH:MM:SS") — Hinnant's civil-from-days.
pub fn utc_from_unix(secs: u64) -> (String, String) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (format!("{y:04}.{m:02}.{d:02}"), format!("{:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_annotations() {
        let mut g = Game::new();
        for s in ["e4", "e5", "Nf3", "Nc6", "Bb5", "a6"] {
            g.play(parse_move(g.position(), s).unwrap()).unwrap();
        }
        let mut p = PgnGame::from_game(&g);
        p.set_tag("Event", "Test \"quoted\"");
        p.set_tag("White", "Alice");
        p.comments[2] = "[%eval 0.31] developing".into();
        p.nags[5] = 6;
        p.pre_comment = "Ruy Lopez".into();
        let text = p.to_pgn();
        assert!(text.contains("[Event \"Test \\\"quoted\\\"\"]"));
        assert!(
            text.contains("{Ruy Lopez} 1. e4 e5 2. Nf3 {[%eval 0.31] developing} 2... Nc6 3. Bb5 a6 $6 *"),
            "{text}"
        );
        let back = parse_one(&text).unwrap();
        assert_eq!(back.moves, p.moves);
        assert_eq!(back.comments, p.comments);
        assert_eq!(back.nags, p.nags);
        assert_eq!(back.pre_comment, "Ruy Lopez");
        assert_eq!(back.tag("Event"), Some("Test \"quoted\""));
        assert_eq!(back.tag("Round"), Some("?"));
    }

    #[test]
    fn parses_real_world_syntax() {
        let text = r#"
% escape line ignored
[Event "Casual"]
[Site "?"]
[Result "1-0"]

1.e4 e5 2.Bc4!? {Bishop's opening} (2.Nf3 Nc6 (2...d6 {Philidor}) 3.Bb5) 2...Nc6
3.Qh5 Nf6?? $4 ; missed the threat
4.Qxf7# 1-0

[Event "Castling with zeros"]

1. e4 e5 2. Nf3 Nf6 3. Bc4 Bc5 4. 0-0 0-0 *

[Event "Second"]
[SetUp "1"]
[FEN "4k3/8/8/8/8/8/4P3/4K3 b - - 0 12"]

12... Kd7 13. e4 1/2-1/2
"#;
        let games = parse_pgn(text).unwrap();
        assert_eq!(games.len(), 3);
        let g = &games[0];
        assert_eq!(g.moves.len(), 7);
        assert_eq!(g.result, "1-0");
        assert_eq!(g.nags[2], 5); // Bc4!?
        assert_eq!(g.nags[5], 4); // Nf6?? (suffix) then $4
        assert_eq!(g.comments[2], "Bishop's opening");
        assert_eq!(g.comments[5], "missed the threat");
        assert!(g.to_game().result().is_some_and(|r| r.winner() == Some(Color::White)));
        assert_eq!(games[1].to_game().san_moves()[6..], ["O-O", "O-O"]);
        let g2 = &games[2];
        assert_eq!(g2.start.fullmove_number(), 12);
        assert_eq!(g2.moves.len(), 2);
        assert_eq!(g2.result, "1/2-1/2");
        // Custom start positions round-trip with SetUp/FEN tags.
        let again = parse_one(&g2.to_pgn()).unwrap();
        assert_eq!(again.start, g2.start);
        assert_eq!(again.moves, g2.moves);
    }

    #[test]
    fn reports_illegal_moves() {
        let err = parse_one("1. e4 e5 2. Ke3 *").unwrap_err();
        assert!(err.contains("Ke3"), "{err}");
        assert!(parse_one("[FEN \"garbage\"]\n1. e4 *").is_err());
    }

    #[test]
    fn exports_from_game_object_match_game_pgn() {
        let mut g = Game::new();
        for s in ["f3", "e5", "g4", "Qh4#"] {
            g.play(parse_move(g.position(), s).unwrap()).unwrap();
        }
        let p = PgnGame::from_game(&g);
        assert_eq!(p.result, "0-1");
        let text = p.to_pgn();
        assert!(text.contains("1. f3 e5 2. g4 Qh4# 0-1"), "{text}");
        assert_eq!(parse_one(&text).unwrap().to_game().result(), g.result());
    }

    #[test]
    fn comment_commands() {
        let c = "[%eval -1.25] [%best e2e4] text [%class Mistake]";
        assert_eq!(comment_command(c, "eval"), Some("-1.25"));
        assert_eq!(comment_command(c, "best"), Some("e2e4"));
        assert_eq!(comment_command(c, "class"), Some("Mistake"));
        assert_eq!(comment_command(c, "clk"), None);
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc_from_unix(0), ("1970.01.01".into(), "00:00:00".into()));
        assert_eq!(utc_from_unix(951_782_400), ("2000.02.29".into(), "00:00:00".into())); // leap day
        assert_eq!(utc_from_unix(1_700_000_000), ("2023.11.14".into(), "22:13:20".into()));
        assert_eq!(utc_from_unix(4_102_444_799), ("2099.12.31".into(), "23:59:59".into()));
    }
}
