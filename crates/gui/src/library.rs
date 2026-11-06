//! Saved games: one standard PGN file per game in the games folder.
//!
//! Files are written atomically (temp file + rename) after every move, so a
//! crash never loses or corrupts a game. They open in any chess program.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chess_engine::pgn::{PgnGame, parse_one};

/// Summary of a saved game for the games list.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedGame {
    pub path: PathBuf,
    pub date: String,
    pub time: String,
    pub white: String,
    pub black: String,
    pub result: String,
    pub termination: String,
    pub plies: usize,
    /// (white, black) accuracy if the game has been reviewed.
    pub accuracy: Option<(String, String)>,
}

impl SavedGame {
    pub fn moves(&self) -> usize {
        self.plies.div_ceil(2)
    }
}

pub struct GameLibrary {
    dir: PathBuf,
}

/// Keep file names portable: letters, digits and dashes only.
fn slug(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if s.is_empty() { "player".into() } else { s }
}

impl GameLibrary {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        GameLibrary { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A fresh, unused file path for a game with these tags.
    pub fn new_path(&self, pgn: &PgnGame) -> PathBuf {
        let date = pgn.tag("Date").unwrap_or("0000.00.00").replace('.', "-");
        let time = pgn.tag("Time").unwrap_or("000000").replace(':', "");
        let base = format!(
            "{date}_{time}_{}-vs-{}",
            slug(pgn.tag("White").unwrap_or("White")),
            slug(pgn.tag("Black").unwrap_or("Black"))
        );
        let mut path = self.dir.join(format!("{base}.pgn"));
        let mut n = 2;
        while path.exists() {
            path = self.dir.join(format!("{base}-{n}.pgn"));
            n += 1;
        }
        path
    }

    /// Atomically write `pgn` to `path`.
    pub fn save(&self, path: &Path, pgn: &PgnGame) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = path.with_extension("pgn.tmp");
        fs::write(&tmp, pgn.to_pgn())?;
        fs::rename(&tmp, path)
    }

    pub fn load(&self, path: &Path) -> Result<PgnGame, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        parse_one(&text)
    }

    pub fn delete(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }

    /// All readable saved games, newest first. Unparsable files are skipped.
    pub fn list(&self) -> Vec<SavedGame> {
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        let mut games: Vec<SavedGame> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pgn"))
            .filter_map(|path| {
                let pgn = self.load(&path).ok()?;
                let tag = |k: &str| pgn.tag(k).unwrap_or("?").to_string();
                let accuracy = match (pgn.tag("WhiteAccuracy"), pgn.tag("BlackAccuracy")) {
                    (Some(w), Some(b)) => Some((w.to_string(), b.to_string())),
                    _ => None,
                };
                Some(SavedGame {
                    date: tag("Date"),
                    time: tag("Time"),
                    white: tag("White"),
                    black: tag("Black"),
                    result: pgn.result.clone(),
                    termination: pgn.tag("Termination").unwrap_or("").to_string(),
                    plies: pgn.moves.len(),
                    accuracy,
                    path,
                })
            })
            .collect();
        games.sort_by(|a, b| (&b.date, &b.time, &b.path).cmp(&(&a.date, &a.time, &a.path)));
        games
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chess_engine::{Game, parse_move};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chess-rs-lib-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn sample(date: &str, time: &str, moves: &[&str]) -> PgnGame {
        let mut g = Game::new();
        for s in moves {
            g.play(parse_move(g.position(), s).unwrap()).unwrap();
        }
        let mut p = PgnGame::from_game(&g);
        p.set_tag("Date", date);
        p.set_tag("Time", time);
        p.set_tag("White", "Human Player");
        p.set_tag("Black", "chess-rs");
        p
    }

    #[test]
    fn save_list_load_delete() {
        let dir = tmp("crud");
        let lib = GameLibrary::new(&dir);
        assert!(lib.list().is_empty(), "missing folder lists nothing");
        let older = sample("2026.01.02", "10:00:00", &["e4", "e5"]);
        let newer = sample("2026.03.04", "09:30:00", &["f3", "e5", "g4", "Qh4#"]);
        let p1 = lib.new_path(&older);
        assert!(p1.file_name().unwrap().to_str().unwrap().starts_with("2026-01-02_100000_Human-Player-vs-chess-rs"));
        lib.save(&p1, &older).unwrap();
        let p2 = lib.new_path(&newer);
        lib.save(&p2, &newer).unwrap();
        // Same timestamp and players get a unique name.
        let p3 = lib.new_path(&newer);
        assert_ne!(p3, p2);

        let list = lib.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, p2, "newest first");
        assert_eq!(list[0].result, "0-1");
        assert_eq!(list[0].plies, 4);
        assert_eq!(list[0].moves(), 2);
        assert_eq!(list[1].white, "Human Player");
        assert_eq!(lib.load(&p2).unwrap().moves, newer.moves);

        // Overwriting in place (autosave) keeps one file.
        let mut more = older.clone();
        more.moves.extend(sample("x", "y", &["e4", "e5", "Nf3"]).moves[2..].iter().copied());
        lib.save(&p1, &more).unwrap();
        assert_eq!(lib.list().len(), 2);
        assert_eq!(lib.load(&p1).unwrap().moves.len(), 3);

        // Junk files are ignored, not fatal.
        fs::write(dir.join("broken.pgn"), "1. e4 e5 2. Ke9 *").unwrap();
        assert_eq!(lib.list().len(), 2);

        lib.delete(&p1).unwrap();
        lib.delete(&p1).unwrap(); // idempotent
        assert_eq!(lib.list().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Human Player"), "Human-Player");
        assert_eq!(slug("chess-rs (depth 12)"), "chess-rs-depth-12");
        assert_eq!(slug("../../etc"), "etc");
        assert_eq!(slug("!!"), "player");
    }
}
