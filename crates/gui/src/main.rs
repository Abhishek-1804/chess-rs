//! chess-rs: play chess against a fast engine.

use std::process::ExitCode;
use std::time::Duration;

use chess_rs::library::GameLibrary;
use chess_rs::selfplay::{SelfPlay, generate};
use chess_rs::{AppConfig, ChessApp, Mode, SidePick, default_cache_path, default_games_dir};

const USAGE: &str = "\
chess-rs — play chess against a multi-threaded alpha-beta engine

USAGE:
  chess-rs [OPTIONS]                 open the game
  chess-rs selfplay [SELFPLAY OPTS]  play bot-vs-bot games headlessly into the games folder

OPTIONS:
  -d, --depth <N>      Bot search depth in plies (1-40)            [default: 12]
  -c, --color <C>      Your side: white | black | random           [default: white]
  -m, --mode <M>       human-vs-bot | bot-vs-bot | human-vs-human  [default: human-vs-bot]
  -t, --threads <N>    Search threads                              [default: all cores]
      --hash <MB>      Transposition table size in MiB             [default: 1024]
      --fen <FEN>      Start from a custom position
      --cache <PATH>   Position cache file      [default: <project>/cache/positions.bin]
      --no-cache       Do not read or write the on-disk position cache
      --games-dir <P>  Where games are saved                   [default: <project>/games]
      --no-games       Do not save games
      --review-depth <N>  Search depth for Game Review (1-40)   [default: 14]
  -h, --help           Show this help

SELFPLAY OPTS:
      --games <N>      Number of games                             [default: 6]
  -d, --depth <N>      Search depth for both sides                 [default: 4]
      --random <N>     Random opening plies, for variety           [default: 4]
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
            "-c" | "--color" => {
                cfg.side = match val("--color")?.to_lowercase().as_str() {
                    "w" | "white" => SidePick::White,
                    "b" | "black" => SidePick::Black,
                    "r" | "random" => SidePick::Random,
                    c => return Err(format!("unknown color '{c}'")),
                }
            }
            "-m" | "--mode" => {
                cfg.mode = match val("--mode")?.to_lowercase().as_str() {
                    "human-vs-bot" | "hvb" => Mode::HumanVsBot,
                    "bot-vs-bot" | "bvb" | "selfplay" => Mode::BotVsBot,
                    "human-vs-human" | "hvh" => Mode::HumanVsHuman,
                    m => return Err(format!("unknown mode '{m}'")),
                }
            }
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
        random_plies: 4,
        seed: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64),
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
            "--random" => cfg.random_plies = num("--random")? as usize,
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
    let cfg = AppConfig { bot_delay: Duration::from_millis(350), ..cfg };
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
