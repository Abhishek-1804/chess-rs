//! chess-rs: play chess against a fast engine.

use std::process::ExitCode;

use chess_rs::library::GameLibrary;
use chess_rs::selfplay::{SelfPlay, generate};
use chess_rs::{AppConfig, ChessApp, Mode, SidePick, default_cache_path, default_games_dir};

const USAGE: &str = "\
chess-rs — play chess against a multi-threaded alpha-beta engine

USAGE:
  chess-rs [OPTIONS]                 open the game
  chess-rs selfplay [SELFPLAY OPTS]  play bot-vs-bot games headlessly into the games folder
  chess-rs online host|join ...      play someone over the internet from the terminal
                                     (chess-rs online --help for details)

OPTIONS:
  -d, --depth <N>      Bot search depth in plies (1-40)            [default: 12]
                       Strength: depth 1 ≈ 1300 Elo, 4 ≈ 1650, 6 ≈ 2000, 8 ≈ 2400,
                       12 ≈ 2750
                       (chess-rs --elo lists every depth)
  -c, --color <C>      Your side: white | black | random           [default: white]
  -m, --mode <M>       human-vs-bot | bot-vs-bot | human-vs-human | online
                                                                   [default: human-vs-bot]
  -t, --threads <N>    Search threads                              [default: all cores]
      --hash <MB>      Transposition table size in MiB             [default: 1024]
      --fen <FEN>      Start from a custom position
      --cache <PATH>   Position cache file      [default: <project>/cache/positions.bin]
      --no-cache       Do not read or write the on-disk position cache
      --games-dir <P>  Where games are saved                   [default: <project>/games]
      --no-games       Do not save games
      --review-depth <N>  Search depth for Game Review (1-40)   [default: 14]
      --elo            List the estimated Elo of every depth
  -h, --help           Show this help

SELFPLAY OPTS:
      --games <N>      Number of games                             [default: 6]
  -d, --depth <N>      Search depth for both sides                 [default: 4]
  -t, --threads <N>    Search threads                              [default: all cores]
      --games-dir <P>  Output folder                   [default: <project>/games]
";

fn parse_args() -> Result<AppConfig, String> {
    let mut cfg = AppConfig {
        cache_path: Some(default_cache_path()),
        games_dir: Some(default_games_dir()),
        ..AppConfig::default()
    };
    if let Ok(p) = std::env::var("CHESS_RS_CACHE") {
        cfg.cache_path = Some(p.into());
    }
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-d" | "--depth" => {
                cfg.depth = val("--depth")?.parse().map_err(|_| "depth must be a number")?;
                if !(1..=chess_rs::app::MAX_DEPTH).contains(&cfg.depth) {
                    return Err(format!("depth must be between 1 and {}", chess_rs::app::MAX_DEPTH));
                }
            }
            "-c" | "--color" => cfg.side = SidePick::parse(&val("--color")?)?,
            "-m" | "--mode" => cfg.mode = Mode::parse(&val("--mode")?)?,
            "-t" | "--threads" => cfg.threads = val("--threads")?.parse().map_err(|_| "threads must be a number")?,
            "--hash" => cfg.hash_mb = val("--hash")?.parse().map_err(|_| "hash must be a number (MiB)")?,
            "--fen" => cfg.fen = Some(val("--fen")?),
            "--cache" => cfg.cache_path = Some(val("--cache")?.into()),
            "--no-cache" => cfg.cache_path = None,
            "--games-dir" => cfg.games_dir = Some(val("--games-dir")?.into()),
            "--no-games" => cfg.games_dir = None,
            "--review-depth" => {
                cfg.review_depth = val("--review-depth")?.parse().map_err(|_| "review depth must be a number")?
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if let Some(f) = &cfg.fen {
        chess_engine::Position::from_fen(f).map_err(|e| format!("invalid --fen: {e}"))?;
    }
    Ok(cfg)
}

fn selfplay(args: &[String]) -> Result<(), String> {
    let mut cfg = SelfPlay {
        games: 6,
        depth: 4,
        threads: chess_engine::default_threads(),
        seed: chess_rs::Rng::from_time().next_u64(),
    };
    let mut dir = default_games_dir();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut num = |name: &str| -> Result<u64, String> {
            it.next().ok_or(format!("{name} needs a value"))?.parse().map_err(|_| format!("{name} must be a number"))
        };
        match a.as_str() {
            "--games" => cfg.games = num("--games")? as usize,
            "-d" | "--depth" => cfg.depth = num("--depth")?.clamp(1, 40) as u32,
            "-t" | "--threads" => cfg.threads = num("--threads")?.max(1) as usize,
            "--games-dir" => dir = it.next().ok_or("--games-dir needs a value")?.into(),
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown selfplay argument '{other}'")),
        }
    }
    let lib = GameLibrary::new(&dir);
    println!("Playing {} games at depth {} into {}", cfg.games, cfg.depth, dir.display());
    generate(&lib, &cfg, |i, path, summary| {
        println!("  game {}: {summary} -> {}", i + 1, path.file_name().unwrap_or_default().to_string_lossy());
    })
    .map_err(|e| e.to_string())?;
    println!("Done. Open the Games tab (mise run play) to browse and review them.");
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--elo") {
        use std::io::Write;
        let mut out = String::from("depth  estimated Elo\n");
        for d in 1..=chess_rs::app::MAX_DEPTH {
            out += &format!("{d:>5}  {}\n", chess_rs::app::strength_label(d).trim_start_matches("≈ "));
        }
        let _ = std::io::stdout().write_all(out.as_bytes()); // ignore a closed pipe (| head)
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) == Some("online") {
        return match chess_rs::online_cli::run(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("selfplay") {
        return match selfplay(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}\n\n{USAGE}");
                ExitCode::FAILURE
            }
        };
    }
    let cfg = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("chess-rs")
            .with_app_id("chess-rs")
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([760.0, 560.0]),
        ..Default::default()
    };
    let result =
        eframe::run_native("chess-rs", options, Box::new(move |cc| Ok(Box::new(ChessApp::new(&cc.egui_ctx, cfg)))));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
