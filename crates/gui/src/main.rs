//! chess-rs: play chess against a fast engine.

use std::process::ExitCode;
use std::time::Duration;

use chess_rs::{AppConfig, ChessApp, Mode, SidePick, default_cache_path};

const USAGE: &str = "\
chess-rs — play chess against a multi-threaded alpha-beta engine

USAGE: chess-rs [OPTIONS]

OPTIONS:
  -d, --depth <N>      Bot search depth in plies (1-40)            [default: 12]
  -c, --color <C>      Your side: white | black | random           [default: white]
  -m, --mode <M>       human-vs-bot | bot-vs-bot | human-vs-human  [default: human-vs-bot]
  -t, --threads <N>    Search threads                              [default: all cores]
      --hash <MB>      Transposition table size in MiB             [default: 1024]
      --fen <FEN>      Start from a custom position
      --cache <PATH>   Position cache file      [default: <project>/cache/positions.bin]
      --no-cache       Do not read or write the on-disk position cache
  -h, --help           Show this help
";

fn parse_args() -> Result<AppConfig, String> {
    let mut cfg = AppConfig { cache_path: Some(default_cache_path()), ..AppConfig::default() };
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
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if let Some(f) = &cfg.fen {
        chess_engine::Position::from_fen(f).map_err(|e| format!("invalid --fen: {e}"))?;
    }
    Ok(cfg)
}

fn main() -> ExitCode {
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
