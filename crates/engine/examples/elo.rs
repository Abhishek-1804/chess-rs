//! Measure the engine's Elo at each search depth by playing it against
//! Stockfish at calibrated strength levels (`UCI_LimitStrength` / `UCI_Elo`,
//! which Stockfish anchors to the CCRL 40/4 rating list).
//!
//! ```sh
//! mise run elo:calibrate -- --stockfish /path/to/stockfish --depths 1-12
//! ```
//!
//! For each depth it plays batches of games against Stockfish, starting at a
//! guessed level and re-centring each batch on the running estimate. Every
//! batch plays each line of the opening book (`chess_engine::book`) twice,
//! once with each colour. The rating is the maximum-likelihood fit of all games at that depth
//! (logistic Elo model), with a standard error.
//!
//! `--selfplay` instead plays depth d against depth d+1, which measures how
//! much one more ply is worth, for depths beyond Stockfish's 3190 ceiling.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use chess_engine::book::LINES as OPENINGS;
use chess_engine::{Engine, Game, Limits, Move, parse_move};

const MAX_PLIES: usize = 400;
const SF_MIN: i32 = 1320;
const SF_MAX: i32 = 3190;

const USAGE: &str = "\
elo — measure chess-rs's Elo per search depth against Stockfish

USAGE:
  elo --stockfish <PATH> [OPTIONS]
  elo --selfplay [OPTIONS]

OPTIONS:
      --stockfish <PATH>  Stockfish binary (any version with UCI_Elo)
      --depths <A-B>      Depths to measure                     [default: 1-12]
      --batches <N>       Batches per depth (2 games per book line) [default: 2]
      --movetime <MS>     Stockfish time per move at levels of 1700+; half
                          that below, where it searches shallowly anyway [default: 500]
      --workers <N>       Games played in parallel              [default: all cores]
      --start <ELO>       Stockfish level for the first depth   [default: 1320]
      --selfplay          Play depth d vs d+1 instead (no Stockfish)
";

// ---------------------------------------------------------------- Stockfish

struct Stockfish {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
}

impl Stockfish {
    fn spawn(path: &str) -> Self {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("could not start {path}: {e}"));
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        let mut sf = Stockfish { child, stdin, out };
        sf.send("uci");
        sf.wait("uciok");
        for opt in ["Threads value 1", "Hash value 16", "UCI_LimitStrength value true"] {
            sf.send(&format!("setoption name {opt}"));
        }
        sf
    }

    fn send(&mut self, cmd: &str) {
        writeln!(self.stdin, "{cmd}").expect("write to stockfish");
        self.stdin.flush().expect("flush stockfish");
    }

    fn wait(&mut self, prefix: &str) -> String {
        let mut line = String::new();
        loop {
            line.clear();
            if self.out.read_line(&mut line).expect("read stockfish") == 0 {
                panic!("stockfish exited");
            }
            if line.starts_with(prefix) {
                return line.trim().to_string();
            }
        }
    }

    fn new_game(&mut self, elo: i32) {
        self.send(&format!("setoption name UCI_Elo value {elo}"));
        self.send("ucinewgame");
        self.send("isready");
        self.wait("readyok");
    }

    fn best_move(&mut self, moves: &[Move], movetime: u64) -> String {
        let list: Vec<String> = moves.iter().map(|m| m.to_uci()).collect();
        self.send(&format!("position startpos moves {}", list.join(" ")));
        self.send(&format!("go movetime {movetime}"));
        let line = self.wait("bestmove");
        line.split_whitespace().nth(1).unwrap_or("").to_string()
    }
}

impl Drop for Stockfish {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------- games

#[derive(Clone, Copy, Debug)]
enum Opponent {
    Stockfish {
        elo: i32,
    },
    /// chess-rs at another depth.
    Depth(u32),
}

#[derive(Clone, Copy)]
struct Task {
    depth: u32,
    opponent: Opponent,
    opening: usize,
    bot_white: bool,
}

struct Outcome {
    /// 1 = the measured depth won, 0.5 draw, 0 lost.
    score: f64,
    plies: usize,
}

fn play(task: Task, bot: &Engine, other: &Engine, sf: &mut Option<Stockfish>, movetime: u64) -> Outcome {
    let mut game = Game::new();
    for san in OPENINGS[task.opening].split_whitespace() {
        let m = parse_move(game.position(), san).expect("book move");
        game.play(m).unwrap();
    }
    bot.clear();
    other.clear();
    if let (Opponent::Stockfish { elo }, Some(sf)) = (task.opponent, sf.as_mut()) {
        sf.new_game(elo);
    }
    let stop = AtomicBool::new(false);
    while game.result().is_none() && game.ply_count() < MAX_PLIES {
        let white_to_move = game.side_to_move() == chess_engine::Color::White;
        let bots_turn = white_to_move == task.bot_white;
        let m = if bots_turn {
            let r = bot.search(game.position(), &game.history_hashes(), &Limits::depth(task.depth), &stop, |_| {});
            r.best_move.expect("a legal move")
        } else {
            match task.opponent {
                Opponent::Depth(d) => {
                    let r = other.search(game.position(), &game.history_hashes(), &Limits::depth(d), &stop, |_| {});
                    r.best_move.expect("a legal move")
                }
                Opponent::Stockfish { elo } => {
                    // Stockfish's levels are calibrated at 60s+0.6s; give the strong ones
                    // enough time to search as deeply as they would there.
                    let t = if elo >= 1700 { movetime } else { movetime / 2 };
                    let uci = sf.as_mut().unwrap().best_move(game.moves(), t);
                    parse_move(game.position(), &uci).unwrap_or_else(|e| panic!("stockfish move {uci}: {e}"))
                }
            }
        };
        game.play(m).unwrap();
    }
    let score = match game.result().and_then(|r| r.winner()) {
        Some(c) if (c == chess_engine::Color::White) == task.bot_white => 1.0,
        Some(_) => 0.0,
        None => 0.5,
    };
    Outcome { score, plies: game.ply_count() }
}

// ---------------------------------------------------------------- rating fit

fn expected(r: f64, opp: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf((opp - r) / 400.0))
}

/// Maximum-likelihood rating against opponents of known rating, with its
/// standard error. `None` if every game was won (or lost): no finite estimate.
fn fit(games: &[(f64, f64)]) -> Option<(f64, f64)> {
    let total: f64 = games.iter().map(|g| g.1).sum();
    if total == 0.0 || total == games.len() as f64 {
        return None;
    }
    // The log-likelihood is concave: bisect on its derivative.
    let (mut lo, mut hi) = (-2000.0, 6000.0);
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        let grad: f64 = games.iter().map(|&(opp, s)| s - expected(mid, opp)).sum();
        if grad > 0.0 { lo = mid } else { hi = mid }
    }
    let r = (lo + hi) / 2.0;
    let k = std::f64::consts::LN_10 / 400.0;
    let info: f64 = games.iter().map(|&(opp, _)| k * k * expected(r, opp) * (1.0 - expected(r, opp))).sum();
    Some((r, 1.0 / info.sqrt()))
}

// ---------------------------------------------------------------- driver

struct Pool {
    tasks: mpsc::Sender<Task>,
    results: mpsc::Receiver<Outcome>,
}

impl Pool {
    fn new(workers: usize, stockfish: Option<String>, movetime: u64) -> Self {
        let (task_tx, task_rx) = mpsc::channel::<Task>();
        let (res_tx, results) = mpsc::channel();
        let task_rx = Arc::new(Mutex::new(task_rx));
        for _ in 0..workers {
            let task_rx = task_rx.clone();
            let res_tx = res_tx.clone();
            let stockfish = stockfish.clone();
            std::thread::spawn(move || {
                let bot = Engine::new(16, 1);
                let other = Engine::new(16, 1);
                let mut sf = stockfish.as_deref().map(Stockfish::spawn);
                loop {
                    let task = { task_rx.lock().unwrap().recv() };
                    let Ok(task) = task else { return };
                    if res_tx.send(play(task, &bot, &other, &mut sf, movetime)).is_err() {
                        return;
                    }
                }
            });
        }
        Pool { tasks: task_tx, results }
    }

    /// Play every opening with both colours against `opponent`.
    fn batch(&self, depth: u32, opponent: Opponent) -> Vec<Outcome> {
        let mut n = 0;
        for opening in 0..OPENINGS.len() {
            for bot_white in [true, false] {
                self.tasks.send(Task { depth, opponent, opening, bot_white }).unwrap();
                n += 1;
            }
        }
        (0..n).map(|_| self.results.recv().unwrap()).collect()
    }
}

fn summary(out: &[Outcome]) -> String {
    let w = out.iter().filter(|o| o.score == 1.0).count();
    let d = out.iter().filter(|o| o.score == 0.5).count();
    let l = out.len() - w - d;
    let avg_plies = out.iter().map(|o| o.plies).sum::<usize>() / out.len().max(1);
    format!("+{w} ={d} -{l}, avg {avg_plies} plies")
}

fn parse_range(s: &str) -> (u32, u32) {
    match s.split_once('-') {
        Some((a, b)) => (a.parse().expect("depth"), b.parse().expect("depth")),
        None => {
            let d = s.parse().expect("depth");
            (d, d)
        }
    }
}

fn main() {
    let mut stockfish = None;
    let (mut from, mut to) = (1, 12);
    let mut batches = 2;
    let mut movetime = 500;
    let mut workers = chess_engine::default_threads();
    let mut start = SF_MIN;
    let mut selfplay = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| panic!("{a} needs a value"));
        match a.as_str() {
            "--stockfish" => stockfish = Some(val()),
            "--depths" => (from, to) = parse_range(&val()),
            "--batches" => batches = val().parse().expect("--batches"),
            "--movetime" => movetime = val().parse().expect("--movetime"),
            "--workers" => workers = val().parse().expect("--workers"),
            "--start" => start = val().parse().expect("--start"),
            "--selfplay" => selfplay = true,
            _ => {
                print!("{USAGE}");
                std::process::exit(if a == "-h" || a == "--help" { 0 } else { 2 });
            }
        }
    }
    if stockfish.is_none() && !selfplay {
        print!("{USAGE}");
        std::process::exit(2);
    }
    let pool = Pool::new(workers, if selfplay { None } else { stockfish.clone() }, movetime);
    let t0 = Instant::now();

    if selfplay {
        println!("depth  vs depth+1  score   Elo gained by the extra ply");
        for d in from..to {
            let mut out = Vec::new();
            for _ in 0..batches {
                out.extend(pool.batch(d, Opponent::Depth(d + 1)));
            }
            let games: Vec<(f64, f64)> = out.iter().map(|o| (0.0, o.score)).collect();
            let score = games.iter().map(|g| g.1).sum::<f64>() / games.len() as f64;
            let line = match fit(&games) {
                Some((r, se)) => {
                    format!("{:>5}  {:<10} {:>5.1}%  {:+.0} ± {:.0}", d, summary(&out), score * 100.0, -r, se)
                }
                None => format!("{d:>5}  {:<10} {:>5.1}%  (no finite estimate)", summary(&out), score * 100.0),
            };
            println!("{line}  [{:.0}s]", t0.elapsed().as_secs_f64());
        }
        return;
    }

    println!("# Stockfish movetime {movetime} ms (half below 1700), {} games per batch", OPENINGS.len() * 2);
    println!("depth  Elo   ±SE  games  (vs Stockfish levels)");
    let mut guess = start;
    for depth in from..=to {
        let mut games: Vec<(f64, f64)> = Vec::new();
        let mut levels = Vec::new();
        let mut est = None;
        for b in 0..batches {
            let level = guess.clamp(SF_MIN, SF_MAX);
            let out = pool.batch(depth, Opponent::Stockfish { elo: level });
            games.extend(out.iter().map(|o| (level as f64, o.score)));
            est = fit(&games);
            let score = out.iter().map(|o| o.score).sum::<f64>() / out.len() as f64;
            eprintln!(
                "  depth {depth} batch {}: vs SF {level}: {} ({:.0}%) -> {} [{:.0}s]",
                b + 1,
                summary(&out),
                score * 100.0,
                est.map_or("unbounded".into(), |(r, se)| format!("{r:.0} ± {se:.0}")),
                t0.elapsed().as_secs_f64()
            );
            levels.push(level);
            guess = match est {
                Some((r, _)) => r.round() as i32,
                None if score >= 0.5 => level + 400,
                None => level - 400,
            };
        }
        let levels: Vec<String> = levels.iter().map(|l| l.to_string()).collect();
        match est {
            Some((r, se)) => {
                println!("{depth:>5}  {r:>4.0}  {se:>4.0}  {:>5}  ({})", games.len(), levels.join(", "));
                guess = r.round() as i32 + 100;
            }
            None => println!("{depth:>5}  out of Stockfish's range  ({})", levels.join(", ")),
        }
        let _ = std::io::stdout().flush();
    }
}
