//! `chess-cli`: the chess-rs engine from the terminal.
//!
//! Review PGN games (from chess.com, lichess, or chess-rs), analyse positions,
//! solve mates, and run the engine's correctness/speed tools.

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use chess_engine::analysis::{GameReview, MoveClass, analyze, annotate};
use chess_engine::movegen::{perft_divide, perft_parallel};
use chess_engine::notation::{line_to_san, to_san};
use chess_engine::pgn::{PgnGame, parse_pgn};
use chess_engine::search::{Pruning, mate_in};
use chess_engine::{Color, Engine, Game, Limits, Position, SearchInfo, default_threads};

const USAGE: &str = "\
chess-cli - the chess-rs engine from the terminal

USAGE:
  chess-cli <COMMAND> [OPTIONS]
  chess-cli help                  show this help (also: <COMMAND> --help)

COMMANDS:
  review <FILE.pgn | -> [MORE FILES...]
                          Review games like chess.com's Game Review: accuracy per player,
                          a rating for every move, and the better move for each mistake.
                          Reads PGN files (any number), or stdin with '-'.
      --depth <N>         analysis depth in plies                        [default: 14]
      --game <N>          only review game N of a multi-game file         [default: all]
      --format <F>        'text' (readable report) or 'pgn' (annotated PGN)  [default: text]
      --out <FILE>        also write the annotated PGN to FILE
      --threads <N>       search threads; use 1 for reproducible results [default: all cores]
      --hash <MB>         transposition table size                       [default: 256]

  search                  Analyse one position: prints each depth's score and best line.
      --fen <FEN>         position to analyse                            [default: start]
      --pgn <FILE>        analyse the final position of a PGN game instead
      --depth <N>         search depth                                    [default: 12]
      --threads <N>, --hash <MB>
      --no-pruning        plain alpha-beta (also --no-nmp --no-rfp --no-futility
                          --no-lmp --no-lmr) for comparison/debugging

  mate <N>                Exhaustive forced-mate solver (no pruning): is there a mate in
                          at most N moves, and what is the first move?
      --fen <FEN>         position                                        [default: start]

  perft <DEPTH>           Count leaf nodes of the legal move tree (move-generator check).
      --fen <FEN>         position                                        [default: start]
      --divide            print the count for each first move

  bench                   Search-speed benchmark on 8 fixed positions.
      --depth <N>         [default: 12]   --threads <N>   --hash <MB>

EXAMPLES:
  chess-cli review my_game.pgn
  chess-cli review games/*.pgn --depth 10
  chess-cli review lichess_export.pgn --game 3 --depth 18
  chess-cli review game.pgn --format pgn > game_annotated.pgn
  cat game.pgn | chess-cli review - --out annotated.pgn
  chess-cli search --fen \"r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3\" --depth 20
  chess-cli mate 2 --fen \"r2qkb1r/pp2nppp/3p4/2pNN1B1/2BnP3/3P4/PPP2PPP/R2bK2R w KQkq - 1 1\"
  chess-cli perft 6

EXIT STATUS: 0 on success, 1 on bad arguments or unreadable input.
";

/// Mixed middlegame/endgame positions for the speed benchmark.
const BENCH_FENS: &[&str] = &[
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P3/2NP1N2/PPP1QPPP/R4RK1 w - - 0 10",
    "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
    "2r3k1/pp3ppp/4p3/3pP3/1P1P4/P4N2/5PPP/2R3K1 w - - 0 25",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "6k1/5ppp/8/8/8/8/1r3PPP/3R2K1 b - - 0 1",
];

#[derive(Copy, Clone, PartialEq, Eq)]
enum Format {
    Text,
    Pgn,
}

struct Opts {
    fen: String,
    pgn: Option<String>,
    depth: Option<u32>,
    threads: usize,
    hash: usize,
    divide: bool,
    pruning: Pruning,
    game: Option<usize>,
    format: Format,
    out: Option<String>,
    help: bool,
    positional: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        fen: chess_engine::START_FEN.to_string(),
        pgn: None,
        depth: None,
        threads: default_threads(),
        hash: 256,
        divide: false,
        pruning: Pruning::default(),
        game: None,
        format: Format::Text,
        out: None,
        help: false,
        positional: vec![],
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => o.help = true,
            "--fen" => o.fen = val("--fen")?,
            "--pgn" => o.pgn = Some(val("--pgn")?),
            "--depth" | "-d" => {
                let d: u32 = val("--depth")?.parse().map_err(|_| "--depth must be a number")?;
                if !(1..=chess_engine::search::MAX_DEPTH).contains(&d) {
                    return Err(format!("--depth must be between 1 and {}", chess_engine::search::MAX_DEPTH));
                }
                o.depth = Some(d);
            }
            "--threads" | "-t" => {
                o.threads = val("--threads")?.parse::<usize>().map_err(|_| "--threads must be a number")?.max(1)
            }
            "--hash" => o.hash = val("--hash")?.parse().map_err(|_| "--hash must be a number (MB)")?,
            "--game" => {
                let n: usize = val("--game")?.parse().map_err(|_| "--game must be a number")?;
                if n == 0 {
                    return Err("--game counts from 1".into());
                }
                o.game = Some(n);
            }
            "--format" => {
                o.format = match val("--format")?.as_str() {
                    "text" => Format::Text,
                    "pgn" => Format::Pgn,
                    f => return Err(format!("--format must be 'text' or 'pgn', not '{f}'")),
                }
            }
            "--out" | "-o" => o.out = Some(val("--out")?),
            "--divide" => o.divide = true,
            "--no-nmp" => o.pruning.null_move = false,
            "--no-rfp" => o.pruning.reverse_futility = false,
            "--no-futility" => o.pruning.futility = false,
            "--no-lmp" => o.pruning.late_move_pruning = false,
            "--no-lmr" => o.pruning.late_move_reductions = false,
            "--no-pruning" => o.pruning = Pruning::NONE,
            "-" => o.positional.push("-".into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => o.positional.push(s.to_string()),
        }
    }
    Ok(o)
}

/// Exhaustive search: a first move forcing mate within `n` moves, if any.
fn mate_move(pos: &Position, n: u32) -> Option<chess_engine::Move> {
    fn forced(pos: &Position, n: u32) -> bool {
        chess_engine::legal_moves(pos).iter().any(|&m| mates_after(pos, m, n))
    }
    fn mates_after(pos: &Position, m: chess_engine::Move, n: u32) -> bool {
        let child = pos.make_move(m);
        let replies = chess_engine::legal_moves(&child);
        if replies.is_empty() {
            return child.in_check();
        }
        n > 1 && replies.iter().all(|&r| forced(&child.make_move(r), n - 1))
    }
    chess_engine::legal_moves(pos).iter().copied().find(|&m| mates_after(pos, m, n))
}

fn fmt_score(score: i32) -> String {
    match mate_in(score) {
        Some(n) => format!("mate {n}"),
        None => format!("cp {score}"),
    }
}

/// White-relative evaluation for people: +0.35, -1.20, #3, #-2.
fn fmt_eval(score: i32) -> String {
    match mate_in(score) {
        Some(0) => "mate".into(), // the game ended in checkmate
        Some(n) => format!("#{n}"),
        None => format!("{:+.2}", score as f64 / 100.0),
    }
}

fn print_info(pos: &Position, i: &SearchInfo) {
    println!(
        "depth {:>2} seldepth {:>2} score {:>9} nodes {:>11} nps {:>10} time {:>7}ms hashfull {:>4} pv {}",
        i.depth,
        i.seldepth,
        fmt_score(i.score),
        i.nodes,
        i.nps(),
        i.time.as_millis(),
        i.hashfull,
        line_to_san(pos, &i.pv).join(" ")
    );
}

fn read_input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| format!("reading stdin: {e}"))?;
        Ok(s)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
    }
}

/// The text report for one reviewed game.
fn review_report(pgn: &PgnGame, game: &Game, r: &GameReview, index: usize, total: usize) -> String {
    let tag = |k: &str| pgn.tag(k).unwrap_or("?").to_string();
    let mut s = String::new();
    s.push_str(&format!(
        "Game {index}/{total}: {} vs {} ({})  {} {}\n",
        tag("White"),
        tag("Black"),
        pgn.result,
        tag("Site"),
        tag("Date")
    ));
    s.push_str(&format!("Reviewed at depth {}, {} plies\n\n", r.depth, game.ply_count()));
    s.push_str(&format!("{:<18}{:>8}{:>8}\n", "", "White", "Black"));
    s.push_str(&format!(
        "{:<18}{:>8.1}{:>8.1}\n",
        "Accuracy",
        r.accuracy[Color::White.idx()],
        r.accuracy[Color::Black.idx()]
    ));
    for class in MoveClass::ALL {
        let label = format!("{} ({})", class.label(), class.symbol());
        s.push_str(&format!("{:<18}{:>8}{:>8}\n", label, r.count(Color::White, class), r.count(Color::Black, class)));
    }
    s.push_str("\nMoves (evaluation after the move, from White's side):\n");
    for (i, m) in r.moves.iter().enumerate() {
        let before = game.position_at(i);
        let number = if before.side_to_move() == Color::White {
            format!("{}.", before.fullmove_number())
        } else {
            format!("{}...", before.fullmove_number())
        };
        let mut line = format!(
            "{:>6} {:<8} {:<14} {:>7}",
            number,
            game.san_moves()[i],
            format!("{} {}", m.class.label(), m.class.symbol()),
            fmt_eval(r.evals[i + 1].score)
        );
        if m.class.is_error()
            && let Some(best) = m.best
        {
            let pv = &r.evals[i].pv;
            let shown = if pv.first() == Some(&best) { pv.as_slice() } else { std::slice::from_ref(&best) };
            let sans = line_to_san(before, shown);
            line.push_str(&format!("   better: {}", sans.first().cloned().unwrap_or_default()));
            if sans.len() > 1 {
                line.push_str(&format!("  ({})", sans.iter().take(6).cloned().collect::<Vec<_>>().join(" ")));
            }
        }
        s.push_str(line.trim_end());
        s.push('\n');
    }
    s
}

fn cmd_review(opts: &Opts) -> Result<(), String> {
    if opts.positional.is_empty() {
        return Err("review needs a PGN file (or '-' for stdin)".into());
    }
    let depth = opts.depth.unwrap_or(14);
    let engine = Engine::new(opts.hash, opts.threads);
    let mut annotated = Vec::new();
    let mut first = true;
    for path in &opts.positional {
        review_file(opts, path, &engine, depth, &mut annotated, &mut first)?;
    }
    if let Some(out) = &opts.out {
        std::fs::write(out, annotated.join("\n")).map_err(|e| format!("{out}: {e}"))?;
        eprintln!("annotated PGN written to {out}");
    }
    Ok(())
}

fn review_file(
    opts: &Opts,
    path: &str,
    engine: &Engine,
    depth: u32,
    annotated: &mut Vec<String>,
    first: &mut bool,
) -> Result<(), String> {
    let text = read_input(path)?;
    let mut games = parse_pgn(&text).map_err(|e| format!("{path}: {e}"))?;
    if games.is_empty() {
        return Err(format!("{path}: no games found"));
    }
    let total = games.len();
    let selected: Vec<usize> = match opts.game {
        Some(n) if n > total => return Err(format!("--game {n}: the file has only {total} game(s)")),
        Some(n) => vec![n - 1],
        None => (0..total).collect(),
    };
    let stop = AtomicBool::new(false);
    let show_progress = std::io::stderr().is_terminal();
    let mut stdout = std::io::stdout().lock();
    for (k, &i) in selected.iter().enumerate() {
        let pgn = &mut games[i];
        let game = pgn.to_game();
        if game.ply_count() == 0 {
            eprintln!("game {}: no moves, skipped", i + 1);
            continue;
        }
        let review = analyze(engine, &game, depth, &stop, |(done, n), _, _| {
            if show_progress {
                eprint!("\r{path}: reviewing game {}/{} ... {done}/{n} positions", k + 1, selected.len());
            }
        })
        .ok_or("analysis was interrupted")?;
        if show_progress {
            eprintln!();
        }
        annotate(pgn, &review);
        let out = match opts.format {
            Format::Text => review_report(pgn, &game, &review, i + 1, total),
            Format::Pgn => pgn.to_pgn(),
        };
        if !*first {
            writeln!(stdout).map_err(|e| e.to_string())?;
        }
        *first = false;
        if opts.positional.len() > 1 && opts.format == Format::Text {
            writeln!(stdout, "== {path}").map_err(|e| e.to_string())?;
        }
        write!(stdout, "{out}").map_err(|e| e.to_string())?;
        annotated.push(pgn.to_pgn());
    }
    Ok(())
}

fn run(cmd: &str, opts: &Opts) -> Result<(), String> {
    let position = || -> Result<(Position, Vec<u64>), String> {
        if let Some(path) = &opts.pgn {
            let text = read_input(path)?;
            let games = parse_pgn(&text).map_err(|e| format!("{path}: {e}"))?;
            let pgn = games.get(opts.game.unwrap_or(1) - 1).ok_or(format!("{path}: game not found"))?;
            let game = pgn.to_game();
            return Ok((*game.position(), game.history_hashes()));
        }
        Ok((Position::from_fen(&opts.fen).map_err(|e| format!("invalid FEN: {e}"))?, Vec::new()))
    };
    match cmd {
        "review" => cmd_review(opts)?,
        "perft" => {
            let depth: u32 =
                opts.positional.first().and_then(|d| d.parse().ok()).ok_or("perft needs a depth, e.g. perft 5")?;
            let (pos, _) = position()?;
            let t = Instant::now();
            let nodes = if opts.divide {
                let mut split = perft_divide(&pos, depth);
                split.sort_by_key(|(m, _)| m.to_uci());
                for (m, n) in &split {
                    println!("{m}: {n}");
                }
                split.iter().map(|(_, n)| n).sum()
            } else {
                perft_parallel(&pos, depth)
            };
            let secs = t.elapsed().as_secs_f64();
            println!("nodes {nodes} time {:.3}s nps {:.0}", secs, nodes as f64 / secs.max(1e-9));
        }
        "bench" => {
            let depth = opts.depth.unwrap_or(12);
            let engine = Engine::new(opts.hash, opts.threads);
            println!("bench: depth {depth}, {} thread(s), {} MB hash", engine.threads(), engine.tt().size_mb());
            let stop = AtomicBool::new(false);
            let t = Instant::now();
            let mut total = 0;
            for fen in BENCH_FENS {
                engine.clear();
                let p = Position::from_fen(fen).unwrap();
                let r = engine.search(&p, &[], &Limits::depth(depth), &stop, |_| {});
                total += r.nodes;
                println!(
                    "{:>12} nodes {:>8.0} knps {:>6}ms  {:>8} {:>10}  {}",
                    r.nodes,
                    r.nps() as f64 / 1e3,
                    r.time.as_millis(),
                    r.best_move.map_or("-".into(), |m| to_san(&p, m)),
                    fmt_score(r.score),
                    fen
                );
            }
            let secs = t.elapsed().as_secs_f64();
            println!("total: {total} nodes in {secs:.2}s = {:.0} nps", total as f64 / secs);
        }
        "search" => {
            let (pos, history) = position()?;
            let engine = Engine::new(opts.hash, opts.threads);
            let stop = AtomicBool::new(false);
            let limits = Limits { pruning: opts.pruning, ..Limits::depth(opts.depth.unwrap_or(12)) };
            let r = engine.search(&pos, &history, &limits, &stop, |i| print_info(&pos, i));
            println!("bestmove {}", r.best_move.map_or("(none)".into(), |m| to_san(&pos, m)));
        }
        "mate" => {
            let n: u32 = opts
                .positional
                .first()
                .and_then(|d| d.parse().ok())
                .ok_or("mate needs a move count, e.g. mate 3 --fen \"...\"")?;
            let (pos, _) = position()?;
            let t = Instant::now();
            match (1..=n).find_map(|k| mate_move(&pos, k).map(|m| (k, m))) {
                Some((k, m)) => println!("mate in {k}: {} ({:.2}s)", to_san(&pos, m), t.elapsed().as_secs_f64()),
                None => println!("no forced mate in {n} ({:.2}s)", t.elapsed().as_secs_f64()),
            }
        }
        other => return Err(format!("unknown command '{other}'")),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    if matches!(cmd.as_str(), "help" | "-h" | "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n\nRun 'chess-cli help' for usage.");
            return ExitCode::FAILURE;
        }
    };
    if opts.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(cmd, &opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}\n\nRun 'chess-cli help' for usage.");
            ExitCode::FAILURE
        }
    }
}
