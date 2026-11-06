//! End-to-end tests of the `chess-cli` binary: real processes, real files,
//! stdin/stdout, exit codes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use chess_engine::analysis::{MoveClass, review_from_pgn};
use chess_engine::pgn::{parse_one, parse_pgn};

const FOOLS_MATE: &str = "[White \"Foolish\"]\n[Black \"Fortunate\"]\n\n1. f3 e5 2. g4 Qh4# 0-1\n";

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_chess-cli")).args(args).output().expect("run chess-cli")
}

fn cli_stdin(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_chess-cli"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chess-cli");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn famous() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/famous_games.pgn").to_string()
}

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("chess-cli-test-{}-{name}", std::process::id()))
}

#[test]
fn help_lists_every_command() {
    for args in [&["help"][..], &["--help"], &["review", "--help"]] {
        let o = cli(args);
        assert!(o.status.success(), "{args:?}");
        let out = stdout(&o);
        for cmd in ["review", "search", "mate", "perft", "bench", "EXAMPLES"] {
            assert!(out.contains(cmd), "help is missing {cmd}");
        }
    }
    let o = cli(&[]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("USAGE"));
}

#[test]
fn review_text_report() {
    let o = cli(&["review", &famous(), "--game", "2", "--depth", "8", "--threads", "1"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("Game 2/2: Paul Morphy vs Duke Karl / Count Isouard (1-0)"), "{out}");
    assert!(out.contains("Accuracy"));
    assert!(out.contains("Forced"), "16...Nxb8 is forced");
    assert!(out.contains("Qb8+") && out.contains("Rd8#"));
    // One line per move after the summary.
    assert_eq!(out.lines().filter(|l| l.contains("Nxb8")).count(), 1);
}

#[test]
fn review_stdin_to_annotated_pgn() {
    let o = cli_stdin(&["review", "-", "--format", "pgn", "--depth", "6", "--threads", "1"], FOOLS_MATE);
    assert!(o.status.success(), "{}", stderr(&o));
    let pgn = parse_one(&stdout(&o)).expect("stdout is valid PGN");
    assert_eq!(pgn.tag("White"), Some("Foolish"));
    let review = review_from_pgn(&pgn).expect("PGN carries the review");
    assert_eq!(review.moves[2].class, MoveClass::Blunder, "2.g4??");
    assert!(stdout(&o).contains("g4 $4"), "blunder NAG");
}

#[test]
fn review_all_games_with_out_file() {
    let out = tmp("annotated.pgn");
    let o = cli(&["review", &famous(), "--depth", "5", "--threads", "1", "--out", out.to_str().unwrap()]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("Game 1/2") && stdout(&o).contains("Game 2/2"));
    assert!(stderr(&o).contains("annotated PGN written"));
    let games = parse_pgn(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(games.len(), 2);
    for g in &games {
        assert!(review_from_pgn(g).is_some(), "{:?} lacks a review", g.tag("White"));
        assert!(g.tag("WhiteAccuracy").is_some());
    }
    let _ = std::fs::remove_file(out);
}

#[test]
fn review_several_files() {
    let a = tmp("a.pgn");
    let b = tmp("b.pgn");
    std::fs::write(&a, FOOLS_MATE).unwrap();
    std::fs::write(&b, "1. e4 e5 2. Qh5 Nc6 3. Bc4 Nf6 4. Qxf7# 1-0\n").unwrap();
    let o = cli(&["review", a.to_str().unwrap(), b.to_str().unwrap(), "--depth", "5", "--threads", "1"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains(&format!("== {}", a.display())) && out.contains(&format!("== {}", b.display())), "{out}");
    assert_eq!(out.matches("Accuracy").count(), 2);
    // A bad file among good ones fails the run with a clear message.
    let o = cli(&["review", a.to_str().unwrap(), "/no/such.pgn"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("/no/such.pgn"));
    let _ = (std::fs::remove_file(a), std::fs::remove_file(b));
}

#[test]
fn review_errors_are_reported_with_exit_code_1() {
    let cases: [(&[&str], &str); 5] = [
        (&["review"], "needs a PGN file"),
        (&["review", "/definitely/not/here.pgn"], "not/here.pgn"),
        (&["review", &famous(), "--game", "9"], "only 2 game"),
        (&["review", &famous(), "--format", "xml"], "--format"),
        (&["review", &famous(), "--depth", "0"], "--depth"),
    ];
    for (args, msg) in cases {
        let o = cli(args);
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(stderr(&o).contains(msg), "{args:?}: {}", stderr(&o));
    }
    let o = cli_stdin(&["review", "-"], "1. e4 e5 2. Ke3 *");
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Ke3"), "points at the illegal move: {}", stderr(&o));
    let o = cli(&["frobnicate"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("unknown command"));
}

#[test]
fn search_mate_and_perft_commands() {
    let o = cli(&["perft", "4"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("nodes 197281"), "{}", stdout(&o));
    let o = cli(&["perft", "1", "--divide"]);
    assert_eq!(stdout(&o).lines().filter(|l| l.contains(": 1")).count(), 20);

    let o = cli(&["mate", "2", "--fen", "r2qkb1r/pp2nppp/3p4/2pNN1B1/2BnP3/3P4/PPP2PPP/R2bK2R w KQkq - 1 1"]);
    assert!(stdout(&o).contains("mate in 2: Nf6+"), "{}", stdout(&o));
    let o = cli(&["mate", "1"]);
    assert!(stdout(&o).contains("no forced mate in 1"));

    let o = cli(&["search", "--depth", "6", "--threads", "1", "--fen", "6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1"]);
    assert!(stdout(&o).contains("bestmove Rd8#"), "{}", stdout(&o));
    // --pgn analyses the final position of a game: here Black is checkmated.
    let o = cli(&["search", "--depth", "3", "--threads", "1", "--pgn", &famous(), "--game", "2"]);
    assert!(stdout(&o).contains("bestmove (none)"), "{}", stdout(&o));
    let o = cli(&["search", "--fen", "not a fen"]);
    assert_eq!(o.status.code(), Some(1));
}
