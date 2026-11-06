//! Search validation: forced mates (cross-checked by brute force), tactics,
//! stalemate avoidance, draw handling, limits, threading and determinism.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chess_engine::search::{MATE, mate_in};
use chess_engine::{Engine, Game, Limits, Move, Position, legal_moves, parse_move, to_san};

fn engine(threads: usize) -> Engine {
    Engine::new(16, threads)
}

fn search(fen: &str, depth: u32, threads: usize) -> chess_engine::SearchResult {
    let pos = Position::from_fen(fen).unwrap();
    engine(threads).search(&pos, &[], &Limits::depth(depth), &AtomicBool::new(false), |_| {})
}

/// Exhaustive check: can the side to move force mate within `n` moves?
fn brute_force_mate(pos: &Position, n: u32) -> bool {
    legal_moves(pos).iter().any(|&m| {
        let child = pos.make_move(m);
        let replies = legal_moves(&child);
        if replies.is_empty() {
            return child.in_check();
        }
        n > 1 && replies.iter().all(|&r| brute_force_mate(&child.make_move(r), n - 1))
    })
}

const MATES: &[(&str, i32)] = &[
    ("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1", 1), // back rank
    ("7k/8/6K1/8/8/8/8/5Q2 w - - 0 1", 1),        // Qf8#, not Qf7 stalemate
    ("r2qkb1r/pp2nppp/3p4/2pNN1B1/2BnP3/3P4/PPP2PPP/R2bK2R w KQkq - 1 1", 2), // Nf6+ gxf6 Bxf7#
    ("r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4", 1), // scholar's mate
    ("6k1/pp4p1/2p5/2bp4/8/P5Pb/1P3rrP/2BRRN1K b - - 0 1", 2), // ...Rg1+ Nxg1/Kxg1 ...Rf1#
    ("3k4/8/3K4/8/8/8/8/7Q w - - 0 1", 1),        // Qh8# (x-ray through king)
    ("4r1k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 1", 1), // ...Re1#
];

#[test]
fn finds_forced_mates_with_exact_distance() {
    for &(fen, n) in MATES {
        let pos = Position::from_fen(fen).unwrap();
        // Ground truth from exhaustive search: mate in n exists, not in n-1.
        assert!(brute_force_mate(&pos, n as u32), "{fen}: no mate in {n}");
        if n > 1 {
            assert!(!brute_force_mate(&pos, n as u32 - 1), "{fen}: shorter mate exists");
        }
        for threads in [1, 4] {
            let r = search(fen, 2 * n as u32 + 2, threads);
            assert_eq!(mate_in(r.score), Some(n), "{fen} ({threads} threads): score {}", r.score);
            // Following the PV must actually deliver mate.
            let mut p = pos;
            for &m in &r.pv {
                assert!(legal_moves(&p).contains(&m), "{fen}: illegal PV move {m}");
                p = p.make_move(m);
            }
            assert!(p.in_check() && legal_moves(&p).is_empty(), "{fen}: PV {:?} does not mate", r.pv);
        }
    }
}

#[test]
fn mate_in_one_delivered_at_every_depth() {
    for depth in 1..=10 {
        let r = search("7k/8/6K1/8/8/8/8/5Q2 w - - 0 1", depth, 1);
        let pos = Position::from_fen("7k/8/6K1/8/8/8/8/5Q2 w - - 0 1").unwrap();
        let after = pos.make_move(r.best_move.unwrap());
        assert!(
            after.in_check() && legal_moves(&after).is_empty(),
            "depth {depth}: {}",
            to_san(&pos, r.best_move.unwrap())
        );
    }
}

#[test]
fn reports_being_mated() {
    // Black's only move is ...Kh7, then Qg7# — every reply allows mate in 1.
    let fen = "7k/8/5K2/8/8/8/8/6Q1 b - - 0 1";
    let pos = Position::from_fen(fen).unwrap();
    assert!(legal_moves(&pos).iter().all(|&m| brute_force_mate(&pos.make_move(m), 1)));
    let r = search(fen, 6, 1);
    assert_eq!(mate_in(r.score), Some(-1), "score {}", r.score);
}

#[test]
fn wins_hanging_material() {
    // Free queen on an open file.
    let r = search("4k3/8/8/3q4/8/8/8/3QK3 w - - 0 1", 6, 1);
    assert_eq!(r.best_move.unwrap().to_uci(), "d1d5");
    assert!(r.score > 700, "score {}", r.score);
    // Knight fork: Nc7+ wins the rook on a8.
    let r = search("r3k3/8/8/1N6/8/8/8/4K3 w - - 0 1", 6, 1);
    assert_eq!(r.best_move.unwrap().to_uci(), "b5c7");
}

#[test]
fn avoids_stalemate_when_winning() {
    // Qg6?? / Qf7?? would stalemate; any search depth must avoid it.
    let fen = "7k/5K2/8/8/8/8/8/6Q1 w - - 0 1";
    let pos = Position::from_fen(fen).unwrap();
    for depth in 1..=8 {
        let r = search(fen, depth, 1);
        let after = pos.make_move(r.best_move.unwrap());
        let stalemate = legal_moves(&after).is_empty() && !after.in_check();
        assert!(!stalemate, "depth {depth}: stalemated with {}", r.best_move.unwrap());
    }
}

#[test]
fn draws_score_zero() {
    // Bare kings and K+N v K are dead draws.
    for fen in ["4k3/8/8/8/8/8/8/4K3 w - - 0 1", "4k3/8/8/8/8/8/8/3NK3 b - - 0 1"] {
        let r = search(fen, 10, 1);
        assert_eq!(r.score, 0, "{fen}");
        assert!(r.best_move.is_some());
    }
}

#[test]
fn losing_side_steers_into_repetition() {
    // Lone black king vs K+Q. After ...Kh8 Qc1 ...Kg8 Qb1 the start position
    // recurs, so ...Kh8 now repeats a position: the only non-losing move.
    let fen = "6k1/8/8/8/8/2K5/8/1Q6 b - - 0 1";
    let mut g = Game::from_fen(fen).unwrap();
    for s in ["Kh8", "Qc1", "Kg8", "Qb1"] {
        let m = parse_move(g.position(), s).unwrap();
        g.play(m).unwrap();
    }
    assert_eq!(g.position().hash(), Position::from_fen(fen).unwrap().hash());
    let pos = *g.position();
    let r = engine(1).search(&pos, &g.history_hashes(), &Limits::depth(8), &AtomicBool::new(false), |_| {});
    assert_eq!(r.best_move.unwrap().to_uci(), "g8h8");
    assert_eq!(r.score, 0);
    // Without the history the same position is simply lost.
    let r = engine(1).search(&pos, &[], &Limits::depth(8), &AtomicBool::new(false), |_| {});
    assert!(r.score < -500, "score {}", r.score);
}

#[test]
fn no_moves_returns_none() {
    let mated = search("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1", 5, 1);
    assert_eq!(mated.best_move, None);
    assert_eq!(mated.score, -MATE);
    let stalemate = search("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1", 5, 1);
    assert_eq!(stalemate.best_move, None);
    assert_eq!(stalemate.score, 0);
}

#[test]
fn reaches_requested_depth_and_reports_each_iteration() {
    let pos = Position::startpos();
    let mut seen = Vec::new();
    let r = engine(1).search(&pos, &[], &Limits::depth(9), &AtomicBool::new(false), |i| seen.push(i.depth));
    assert_eq!(r.depth, 9);
    assert_eq!(seen, (1..=9).collect::<Vec<_>>());
    assert!(r.nodes > 0 && !r.pv.is_empty());
}

#[test]
fn single_thread_search_is_deterministic() {
    let fen = "r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4";
    let a = search(fen, 9, 1);
    let b = search(fen, 9, 1);
    assert_eq!((a.best_move, a.score, a.nodes, a.pv.clone()), (b.best_move, b.score, b.nodes, b.pv));
}

#[test]
fn movetime_limit_is_respected() {
    let pos = Position::startpos();
    let limits = Limits { movetime: Some(Duration::from_millis(300)), ..Limits::default() };
    let t = Instant::now();
    let r = engine(2).search(&pos, &[], &limits, &AtomicBool::new(false), |_| {});
    assert!(t.elapsed() < Duration::from_millis(600), "took {:?}", t.elapsed());
    assert!(r.best_move.is_some() && r.depth >= 1);
}

#[test]
fn external_stop_aborts_quickly_with_a_legal_move() {
    let pos = Position::startpos();
    let stop = AtomicBool::new(false);
    let eng = engine(4);
    let t = Instant::now();
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            stop.store(true, Ordering::Relaxed);
        });
        eng.search(&pos, &[], &Limits::default(), &stop, |_| {})
    });
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(legal_moves(&pos).contains(&r.best_move.unwrap()));
}

/// Random self-play at low depth with several thread counts: every move the
/// engine returns must be legal and games must terminate under the rules.
#[test]
fn self_play_always_returns_legal_moves() {
    for threads in [1, 3] {
        let eng = engine(threads);
        let mut game = Game::new();
        let mut plies = 0;
        while game.result().is_none() && plies < 400 {
            let pos = *game.position();
            let r = eng.search(&pos, &game.history_hashes(), &Limits::depth(3), &AtomicBool::new(false), |_| {});
            let m: Move = r.best_move.expect("non-terminal position must yield a move");
            game.play(m).expect("engine move must be legal");
            plies += 1;
        }
        assert!(plies > 0);
    }
}

/// Regression: delta/futility pruning used to skip the defender's drawing
/// moves in a wrong-bishop fortress under narrow windows (score flipped to
/// +6 at some depths). The value must be a stable draw at every depth.
#[test]
fn wrong_bishop_fortress_is_stable_across_depths() {
    for fen in ["1k6/8/PK6/8/8/2B5/8/8 w - - 5 73", "1k6/8/PB6/8/8/K7/8/8 b - - 0 1"] {
        for depth in 1..=16 {
            let r = search(fen, depth, 1);
            assert_eq!(r.score, 0, "{fen} depth {depth}");
        }
    }
}

/// Regression: null-move pruning in zugzwang hid 1.a5! (mate in 3); the
/// defender's pieces are all frozen, so "passing" is not a real option.
#[test]
fn zugzwang_mate_is_found() {
    let r = search("k1B5/2p5/NbN5/8/P2p4/P2p4/P1prp3/2RbK3 w - - 0 1", 8, 1);
    assert_eq!(mate_in(r.score), Some(3), "score {}", r.score);
    assert_eq!(r.best_move.unwrap().to_uci(), "a4a5");
}

/// Regression: the search must not stop at the first (longer) mate it sees.
#[test]
fn shortest_mate_is_reported() {
    let r = search("8/8/8/8/8/6P1/6k1/4KR1R w K - 0 1", 12, 1);
    assert_eq!(mate_in(r.score), Some(3), "score {}", r.score);
}

/// Each pruning switch can be disabled independently and the engine still
/// returns legal moves and finds a simple mate.
#[test]
fn pruning_switches() {
    use chess_engine::search::Pruning;
    let pos = Position::from_fen("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1").unwrap();
    let all = Pruning::default();
    for p in [
        Pruning::NONE,
        Pruning { null_move: false, ..all },
        Pruning { reverse_futility: false, ..all },
        Pruning { futility: false, ..all },
        Pruning { late_move_pruning: false, ..all },
        Pruning { late_move_reductions: false, ..all },
        Pruning { tt_cutoffs: false, ..all },
    ] {
        let lim = Limits { pruning: p, ..Limits::depth(4) };
        let r = engine(1).search(&pos, &[], &lim, &AtomicBool::new(false), |_| {});
        assert_eq!(mate_in(r.score), Some(1), "{p:?}");
    }
}

/// The live node counter is reset per search and matches the final count.
#[test]
fn live_node_counter() {
    let eng = engine(2);
    let counter = eng.node_counter();
    let pos = Position::startpos();
    let r = eng.search(&pos, &[], &Limits::depth(8), &AtomicBool::new(false), |i| {
        assert!(counter.load(Ordering::Relaxed) >= i.nodes / 2);
    });
    assert_eq!(counter.load(Ordering::Relaxed), r.nodes);
    let r2 = eng.search(&pos, &[], &Limits::depth(2), &AtomicBool::new(false), |_| {});
    assert_eq!(counter.load(Ordering::Relaxed), r2.nodes, "counter resets for each search");
    assert!(r2.nodes < r.nodes);
}
