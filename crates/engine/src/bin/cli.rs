//! Engine command-line tools: perft, benchmarks and one-off searches.

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use chess_engine::movegen::{perft_divide, perft_parallel};
use chess_engine::notation::{line_to_san, to_san};
use chess_engine::search::mate_in;
use chess_engine::{Engine, Limits, Position, SearchInfo, default_threads};

const USAGE: &str = "\
chess-cli — engine tools

USAGE:
  chess-cli perft <depth> [--fen <FEN>] [--divide]
  chess-cli bench [--depth <n>] [--threads <n>] [--hash <MB>]
  chess-cli search [--fen <FEN>] [--depth <n>] [--threads <n>] [--hash <MB>]
                   [--no-nmp|--no-rfp|--no-futility|--no-lmp|--no-lmr|--no-pruning]
  chess-cli mate <moves> [--fen <FEN>]      exhaustive forced-mate solver (no pruning)
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

struct Opts {
    fen: String,
    depth: Option<u32>,
    threads: usize,
    hash: usize,
    divide: bool,
    pruning: chess_engine::search::Pruning,
    positional: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        fen: chess_engine::START_FEN.to_string(),
        depth: None,
        threads: default_threads(),
        hash: 256,
        divide: false,
        pruning: Default::default(),
        positional: vec![],
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--fen" => o.fen = val("--fen")?,
            "--depth" | "-d" => o.depth = Some(val("--depth")?.parse().map_err(|_| "bad depth")?),
            "--threads" | "-t" => o.threads = val("--threads")?.parse().map_err(|_| "bad thread count")?,
            "--hash" => o.hash = val("--hash")?.parse().map_err(|_| "bad hash size")?,
            "--divide" => o.divide = true,
            "--no-nmp" => o.pruning.null_move = false,
            "--no-rfp" => o.pruning.reverse_futility = false,
            "--no-futility" => o.pruning.futility = false,
            "--no-lmp" => o.pruning.late_move_pruning = false,
            "--no-lmr" => o.pruning.late_move_reductions = false,
            "--no-pruning" => o.pruning = chess_engine::search::Pruning::NONE,
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => o.positional.push(s.to_string()),
        }
    }
    Ok(o)
}

/// Exhaustive search: a first move forcing mate within `n` moves, if any.
fn mate_move(pos: &Position, n: u32) -> Option<chess_engine::Move> {
    fn forced(pos: &Position, n: u32) -> bool {
        chess_engine::legal_moves(pos).iter().any(|&m| {
            let child = pos.make_move(m);
            let replies = chess_engine::legal_moves(&child);
            if replies.is_empty() {
                return child.in_check();
            }
            n > 1 && replies.iter().all(|&r| forced(&child.make_move(r), n - 1))
        })
    }
    chess_engine::legal_moves(pos).iter().copied().find(|&m| {
        let child = pos.make_move(m);
        let replies = chess_engine::legal_moves(&child);
        if replies.is_empty() {
            return child.in_check();
        }
        n > 1 && replies.iter().all(|&r| forced(&child.make_move(r), n - 1))
    })
}

fn fmt_score(score: i32) -> String {
    match mate_in(score) {
        Some(n) => format!("mate {n}"),
        None => format!("cp {score}"),
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let pos = match Position::from_fen(&opts.fen) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: invalid FEN: {e}");
            return ExitCode::FAILURE;
        }
    };
    match cmd.as_str() {
        "perft" => {
            let Some(depth) = opts.positional.first().and_then(|d| d.parse().ok()) else {
                eprintln!("error: perft needs a depth\n\n{USAGE}");
                return ExitCode::FAILURE;
            };
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
            let engine = Engine::new(opts.hash, opts.threads);
            let stop = AtomicBool::new(false);
            let limits = Limits { pruning: opts.pruning, ..Limits::depth(opts.depth.unwrap_or(12)) };
            let r = engine.search(&pos, &[], &limits, &stop, |i| print_info(&pos, i));
            println!("bestmove {}", r.best_move.map_or("(none)".into(), |m| to_san(&pos, m)));
        }
        "mate" => {
            let Some(n) = opts.positional.first().and_then(|d| d.parse::<u32>().ok()) else {
                eprintln!("error: mate needs a move count\n\n{USAGE}");
                return ExitCode::FAILURE;
            };
            let t = Instant::now();
            match (1..=n).find_map(|k| mate_move(&pos, k).map(|m| (k, m))) {
                Some((k, m)) => println!("mate in {k}: {} ({:.2}s)", to_san(&pos, m), t.elapsed().as_secs_f64()),
                None => println!("no forced mate in {n} ({:.2}s)", t.elapsed().as_secs_f64()),
            }
        }
        _ => {
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
