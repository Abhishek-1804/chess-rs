//! Alpha-beta minimax search (negamax formulation) with the usual modern
//! enhancements, run on N threads via Lazy SMP over one lock-free TT.
//!
//! * iterative deepening + aspiration windows
//! * principal variation search (null-window re-search)
//! * transposition table cutoffs and move ordering
//! * null-move pruning, reverse futility pruning, futility & late-move pruning
//! * late move reductions, check extensions, mate-distance pruning
//! * MVV-LVA + SEE capture ordering, killer moves, history heuristic
//! * quiescence search with SEE and delta pruning; full evasions when in check

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use crate::eval::{SEE_VALUE, evaluate};
use crate::movegen::{MAX_MOVES, MoveList, legal_moves, noisy_moves};
use crate::position::Position;
use crate::see::see;
use crate::tt::{Bound, TranspositionTable};
use crate::types::*;

pub const MAX_PLY: usize = 128;
pub const MAX_DEPTH: u32 = 100;
pub const INF: i32 = 32_000;
pub const MATE: i32 = 31_000;
/// Scores beyond this magnitude are mate scores.
pub const MATE_BOUND: i32 = MATE - MAX_PLY as i32;

/// Is `score` a forced mate (for either side)?
pub fn is_mate_score(score: i32) -> bool {
    score.abs() >= MATE_BOUND
}

/// Signed number of moves to mate (positive: side to move mates).
pub fn mate_in(score: i32) -> Option<i32> {
    if !is_mate_score(score) {
        return None;
    }
    let plies = MATE - score.abs();
    let moves = (plies + 1) / 2;
    Some(if score > 0 { moves } else { -moves })
}

/// Switches for the selective-search techniques (all on by default).
/// Turning them off is for diagnosis and for testing soundness.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Pruning {
    pub null_move: bool,
    pub reverse_futility: bool,
    pub futility: bool,
    pub late_move_pruning: bool,
    pub late_move_reductions: bool,
    /// Use transposition-table scores to cut off (move ordering always uses it).
    pub tt_cutoffs: bool,
}

impl Default for Pruning {
    fn default() -> Self {
        Pruning {
            null_move: true,
            reverse_futility: true,
            futility: true,
            late_move_pruning: true,
            late_move_reductions: true,
            tt_cutoffs: true,
        }
    }
}

impl Pruning {
    /// Plain alpha-beta: no forward pruning or reductions.
    pub const NONE: Pruning = Pruning {
        null_move: false,
        reverse_futility: false,
        futility: false,
        late_move_pruning: false,
        late_move_reductions: false,
        tt_cutoffs: false,
    };
}

#[derive(Clone, Debug, Default)]
pub struct Limits {
    pub pruning: Pruning,
    /// Maximum iterative-deepening depth (plies). `None` = until stopped.
    pub depth: Option<u32>,
    pub movetime: Option<Duration>,
    pub nodes: Option<u64>,
    /// Root moves to leave out (used to find the best *alternative* move).
    pub excluded: Vec<Move>,
}

impl Limits {
    pub fn depth(d: u32) -> Self {
        Limits { depth: Some(d), ..Default::default() }
    }
}

/// Progress report emitted after every completed iteration.
#[derive(Clone, Debug)]
pub struct SearchInfo {
    pub depth: u32,
    pub seldepth: u32,
    /// From the side to move's perspective.
    pub score: i32,
    pub nodes: u64,
    pub time: Duration,
    pub pv: Vec<Move>,
    pub hashfull: usize,
}

impl SearchInfo {
    pub fn nps(&self) -> u64 {
        (self.nodes as f64 / self.time.as_secs_f64().max(1e-6)) as u64
    }
}

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub best_move: Option<Move>,
    pub score: i32,
    pub depth: u32,
    pub seldepth: u32,
    pub nodes: u64,
    pub time: Duration,
    pub pv: Vec<Move>,
}

impl SearchResult {
    pub fn nps(&self) -> u64 {
        (self.nodes as f64 / self.time.as_secs_f64().max(1e-6)) as u64
    }
}

/// The engine: a shared transposition table plus a thread count.
/// `search` takes `&self`, so one engine can be shared across threads.
pub struct Engine {
    tt: TranspositionTable,
    threads: usize,
    /// Positions searched so far by the running (or last) search. Shared so
    /// other threads, e.g. a UI, can display progress live.
    nodes: Arc<AtomicU64>,
}

impl Engine {
    pub fn new(hash_mb: usize, threads: usize) -> Self {
        Engine { tt: TranspositionTable::new(hash_mb), threads: threads.max(1), nodes: Arc::new(AtomicU64::new(0)) }
    }

    /// Live counter of positions searched by the current search (updated by
    /// every search thread in batches of 2048 nodes; reset when a search starts).
    pub fn node_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.nodes)
    }

    pub fn tt(&self) -> &TranspositionTable {
        &self.tt
    }

    pub fn threads(&self) -> usize {
        self.threads
    }

    pub fn set_threads(&mut self, n: usize) {
        self.threads = n.max(1);
    }

    pub fn clear(&self) {
        self.tt.clear();
    }

    /// Search `pos`. `history` holds the hashes of the game positions *before*
    /// `pos` (oldest first) for repetition detection. `stop` may be raised by
    /// another thread at any time; the best fully-searched move is returned.
    pub fn search(
        &self,
        pos: &Position,
        history: &[u64],
        limits: &Limits,
        stop: &AtomicBool,
        mut on_info: impl FnMut(&SearchInfo),
    ) -> SearchResult {
        let start = Instant::now();
        let mut root_moves = legal_moves(pos);
        if !limits.excluded.is_empty() {
            let mut kept = MoveList::new();
            for &m in root_moves.iter().filter(|m| !limits.excluded.contains(m)) {
                kept.push(m);
            }
            if kept.is_empty() {
                // Every legal move excluded: nothing to search.
                return SearchResult {
                    best_move: None,
                    score: 0,
                    depth: 0,
                    seldepth: 0,
                    nodes: 0,
                    time: start.elapsed(),
                    pv: Vec::new(),
                };
            }
            root_moves = kept;
        }
        let mut result = SearchResult {
            best_move: root_moves.first().copied(),
            score: 0,
            depth: 0,
            seldepth: 0,
            nodes: 0,
            time: Duration::ZERO,
            pv: root_moves.first().copied().into_iter().collect(),
        };
        if root_moves.is_empty() {
            result.score = if pos.in_check() { -MATE } else { 0 };
            return result;
        }
        self.tt.new_search();
        let max_depth = limits.depth.unwrap_or(MAX_DEPTH).clamp(1, MAX_DEPTH);
        let nodes = &*self.nodes;
        nodes.store(0, Relaxed);
        let helpers_stop = AtomicBool::new(false);
        let shared = Shared { tt: &self.tt, stop, helpers_stop: &helpers_stop, nodes, start, limits };

        let root_moves = &root_moves;
        let main = std::thread::scope(|s| {
            for id in 1..self.threads {
                s.spawn(move || {
                    let mut w = Worker::new(shared, history, pos, id);
                    // Odd helpers search one ply deeper to desynchronise threads.
                    w.iterate(pos, root_moves, max_depth + (id as u32 & 1), &mut |_| {});
                });
            }
            let mut w = Worker::new(shared, history, pos, 0);
            let r = w.iterate(pos, root_moves, max_depth, &mut on_info);
            helpers_stop.store(true, Relaxed);
            r
        });
        result = main;
        result.nodes = nodes.load(Relaxed);
        result.time = start.elapsed();
        result
    }
}

#[derive(Copy, Clone)]
struct Shared<'a> {
    tt: &'a TranspositionTable,
    stop: &'a AtomicBool,
    helpers_stop: &'a AtomicBool,
    nodes: &'a AtomicU64,
    start: Instant,
    limits: &'a Limits,
}

struct Worker<'a> {
    sh: Shared<'a>,
    id: usize,
    nodes: u64,
    unflushed: u64,
    stopped: bool,
    seldepth: usize,
    completed: u32,
    hashes: Vec<u64>,
    killers: [[Move; 2]; MAX_PLY + 1],
    history: Box<[[[i32; 64]; 64]; 2]>,
    pv: Box<[[Move; MAX_PLY + 1]; MAX_PLY + 1]>,
    pv_len: [usize; MAX_PLY + 1],
    lmr: Box<[[u8; 64]; 64]>,
}

const HISTORY_MAX: i32 = 16_384;

impl<'a> Worker<'a> {
    fn new(sh: Shared<'a>, history: &[u64], root: &Position, id: usize) -> Self {
        let mut hashes = Vec::with_capacity(history.len() + MAX_PLY + 1);
        hashes.extend_from_slice(history);
        hashes.push(root.hash());
        let mut lmr = Box::new([[0u8; 64]; 64]);
        for (d, row) in lmr.iter_mut().enumerate().skip(1) {
            for (m, r) in row.iter_mut().enumerate().skip(1) {
                *r = (0.75 + (d as f64).ln() * (m as f64).ln() / 2.25) as u8;
            }
        }
        Worker {
            sh,
            id,
            nodes: 0,
            unflushed: 0,
            stopped: false,
            seldepth: 0,
            completed: 0,
            hashes,
            killers: [[Move::NULL; 2]; MAX_PLY + 1],
            history: Box::new([[[0; 64]; 64]; 2]),
            pv: Box::new([[Move::NULL; MAX_PLY + 1]; MAX_PLY + 1]),
            pv_len: [0; MAX_PLY + 1],
            lmr,
        }
    }

    fn is_main(&self) -> bool {
        self.id == 0
    }

    fn iterate(
        &mut self,
        pos: &Position,
        root_moves: &MoveList,
        max_depth: u32,
        on_info: &mut dyn FnMut(&SearchInfo),
    ) -> SearchResult {
        let mut best = SearchResult {
            best_move: Some(root_moves[0]),
            score: 0,
            depth: 0,
            seldepth: 0,
            nodes: 0,
            time: Duration::ZERO,
            pv: vec![root_moves[0]],
        };
        let mut prev = 0;
        for depth in 1..=max_depth {
            let score = self.aspiration(pos, depth as i32, prev);
            if self.stopped {
                break;
            }
            self.completed = depth;
            prev = score;
            let pv: Vec<Move> = self.pv[0][..self.pv_len[0]].to_vec();
            if let Some(&m) = pv.first() {
                best.best_move = Some(m);
                best.pv = pv;
            }
            best.score = score;
            best.depth = depth;
            best.seldepth = self.seldepth as u32;
            if self.is_main() {
                self.flush_nodes();
                let info = SearchInfo {
                    depth,
                    seldepth: self.seldepth as u32,
                    score,
                    nodes: self.sh.nodes.load(Relaxed),
                    time: self.sh.start.elapsed(),
                    pv: best.pv.clone(),
                    hashfull: self.sh.tt.hashfull(),
                };
                on_info(&info);
                // Selective search can find a longer mate before the shortest one,
                // so only stop early once we've searched well past the mate length.
                if let Some(n) = mate_in(score)
                    && n > 0
                    && 2 * (2 * n as u32 - 1) + 4 <= depth
                {
                    break;
                }
                if let Some(t) = self.sh.limits.movetime
                    && self.sh.start.elapsed() * 2 > t
                {
                    break; // next iteration would very likely not finish
                }
            }
        }
        self.flush_nodes();
        best
    }

    fn aspiration(&mut self, pos: &Position, depth: i32, prev: i32) -> i32 {
        if depth < 5 || is_mate_score(prev) {
            return self.negamax(pos, depth, -INF, INF, 0, false);
        }
        let mut window = 20;
        let (mut alpha, mut beta) = (prev - window, prev + window);
        loop {
            let score = self.negamax(pos, depth, alpha, beta, 0, false);
            if self.stopped {
                return score;
            }
            if score <= alpha {
                beta = (alpha + beta) / 2;
                alpha = (score - window).max(-INF);
            } else if score >= beta {
                beta = (score + window).min(INF);
            } else {
                return score;
            }
            window += window / 2 + 10;
            if window > 1000 {
                alpha = -INF;
                beta = INF;
            }
        }
    }

    #[inline(always)]
    fn flush_nodes(&mut self) {
        if self.unflushed > 0 {
            self.sh.nodes.fetch_add(self.unflushed, Relaxed);
            self.unflushed = 0;
        }
    }

    #[inline(always)]
    fn tick(&mut self) -> bool {
        self.nodes += 1;
        self.unflushed += 1;
        if self.nodes & 2047 == 0 {
            self.flush_nodes();
            if self.sh.stop.load(Relaxed) || (!self.is_main() && self.sh.helpers_stop.load(Relaxed)) {
                self.stopped = true;
            } else if self.is_main() && self.completed >= 1 {
                let lim = self.sh.limits;
                let out_of_time = lim.movetime.is_some_and(|t| self.sh.start.elapsed() >= t);
                let out_of_nodes = lim.nodes.is_some_and(|n| self.sh.nodes.load(Relaxed) >= n);
                if out_of_time || out_of_nodes {
                    self.stopped = true;
                }
            }
        }
        self.stopped
    }

    /// Has the current position occurred before since the last irreversible move?
    #[inline]
    fn is_repetition(&self, pos: &Position) -> bool {
        let n = self.hashes.len();
        let lim = n.saturating_sub(pos.halfmove_clock() as usize + 1);
        let h = pos.hash();
        let mut i = n as isize - 3;
        while i >= lim as isize {
            if self.hashes[i as usize] == h {
                return true;
            }
            i -= 2;
        }
        false
    }

    #[inline(always)]
    fn update_history(&mut self, side: Color, m: Move, bonus: i32) {
        let h = &mut self.history[side.idx()][m.from() as usize][m.to() as usize];
        *h += bonus - *h * bonus.abs() / HISTORY_MAX;
    }

    fn score_moves(&self, pos: &Position, moves: &MoveList, tt_move: Move, ply: usize, scores: &mut [i32; MAX_MOVES]) {
        let side = pos.side_to_move().idx();
        for (i, &m) in moves.iter().enumerate() {
            scores[i] = if m == tt_move {
                10_000_000
            } else if m.is_capture() {
                let victim = if m.is_ep() {
                    PieceType::Pawn
                } else {
                    pos.piece_on(m.to()).map_or(PieceType::Pawn, |p| p.kind())
                };
                let attacker = pos.piece_on(m.from()).map_or(PieceType::Pawn, |p| p.kind());
                let mvv_lva = SEE_VALUE[victim.idx()] * 8 - attacker.idx() as i32;
                let promo = if m.promotion() == Some(PieceType::Queen) { 5_000 } else { 0 };
                if see(pos, m) >= 0 { 5_000_000 + mvv_lva + promo } else { -1_000_000 + mvv_lva }
            } else if m.promotion() == Some(PieceType::Queen) {
                4_000_000
            } else if m.is_promotion() {
                -2_000_000 // underpromotions last
            } else if m == self.killers[ply][0] {
                3_000_000
            } else if m == self.killers[ply][1] {
                2_900_000
            } else {
                self.history[side][m.from() as usize][m.to() as usize]
            };
        }
    }

    /// Selection sort step: bring the best remaining move to index `i`.
    #[inline(always)]
    fn pick(moves: &mut MoveList, scores: &mut [i32; MAX_MOVES], i: usize) -> Move {
        let mut best = i;
        for j in i + 1..moves.len() {
            if scores[j] > scores[best] {
                best = j;
            }
        }
        if best != i {
            moves.swap(i, best);
            scores.swap(i, best);
        }
        moves[i]
    }

    fn negamax(
        &mut self,
        pos: &Position,
        mut depth: i32,
        mut alpha: i32,
        mut beta: i32,
        ply: usize,
        can_null: bool,
    ) -> i32 {
        self.pv_len[ply] = ply;
        let in_check = pos.in_check();
        if in_check && ply < MAX_PLY / 2 {
            depth += 1;
        }
        if depth <= 0 {
            return self.qsearch(pos, alpha, beta, ply);
        }
        if self.tick() {
            return 0;
        }
        self.seldepth = self.seldepth.max(ply);
        let root = ply == 0;
        let pv_node = beta - alpha > 1;

        if !root {
            if pos.insufficient_material() || self.is_repetition(pos) {
                return 0;
            }
            if pos.halfmove_clock() >= 100 && !(in_check && legal_moves(pos).is_empty()) {
                return 0;
            }
            if ply >= MAX_PLY - 1 {
                return evaluate(pos);
            }
            // Mate-distance pruning.
            alpha = alpha.max(-MATE + ply as i32);
            beta = beta.min(MATE - ply as i32 - 1);
            if alpha >= beta {
                return alpha;
            }
        }

        let tte = self.sh.tt.probe(pos.hash());
        let tt_move = tte.map_or(Move::NULL, |e| e.mv);
        if let Some(e) = tte
            && self.sh.limits.pruning.tt_cutoffs
            && !pv_node
            && e.depth >= depth
        {
            let s = score_from_tt(e.score, ply);
            match e.bound {
                Bound::Exact => return s,
                Bound::Lower if s >= beta => return s,
                Bound::Upper if s <= alpha => return s,
                _ => {}
            }
        }

        let static_eval = if in_check {
            -INF
        } else if let Some(e) = tte {
            e.eval
        } else {
            evaluate(pos)
        };

        let mut moves = legal_moves(pos);
        if moves.is_empty() {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        let excluding = root && !self.sh.limits.excluded.is_empty();
        if excluding {
            let mut kept = MoveList::new();
            for &m in moves.iter().filter(|m| !self.sh.limits.excluded.contains(m)) {
                kept.push(m);
            }
            moves = kept;
        }
        // With very few legal moves zugzwang is likely: "passing" (null move)
        // and "the static eval is good enough" (RFP) are both unsound there.
        let zugzwang_risk = moves.len() <= 6;

        let volatile = volatile_eval(pos);
        if !pv_node && !in_check && !zugzwang_risk {
            // Reverse futility pruning: far above beta at shallow depth.
            let pr = self.sh.limits.pruning;
            if pr.reverse_futility
                && !volatile
                && depth <= 7
                && static_eval - 75 * depth >= beta
                && !is_mate_score(beta)
            {
                return static_eval;
            }
            // Null-move pruning. Never against a mate bound (a pass cannot
            // prove mate), and the reduced search keeps >= 1 ply so quiet
            // mate threats are still seen.
            if pr.null_move
                && can_null
                && depth >= 3
                && static_eval >= beta
                && !is_mate_score(beta)
                && pos.has_non_pawn_material(pos.side_to_move())
            {
                let r = (2 + depth / 4 + ((static_eval - beta) / 200).min(2)).min(depth - 2);
                let child = pos.make_null();
                self.hashes.push(child.hash());
                let score = -self.negamax(&child, depth - 1 - r, -beta, -beta + 1, ply + 1, false);
                self.hashes.pop();
                if self.stopped {
                    return 0;
                }
                if score >= beta {
                    return if is_mate_score(score) { beta } else { score };
                }
            }
        }

        let mut scores = [0i32; MAX_MOVES];
        self.score_moves(pos, &moves, tt_move, ply, &mut scores);

        let side = pos.side_to_move();
        let orig_alpha = alpha;
        let mut best_score = -INF;
        let mut best_move = Move::NULL;
        let mut searched = 0usize;
        let mut quiets: [Move; 64] = [Move::NULL; 64];
        let mut n_quiets = 0;

        for i in 0..moves.len() {
            let m = Self::pick(&mut moves, &mut scores, i);
            let quiet = !m.is_noisy();

            let child = pos.make_move(m);
            self.sh.tt.prefetch(child.hash());
            let gives_check = child.in_check();

            // Shallow-depth pruning of quiet moves once we have a non-losing score.
            // Disabled while hunting/defending mates: margins are meaningless there.
            if !root && !in_check && quiet && !gives_check && best_score > -MATE_BOUND && !is_mate_score(alpha) {
                if self.sh.limits.pruning.late_move_pruning
                    && depth <= 4
                    && searched >= (3 + depth * depth * 2) as usize
                {
                    continue; // late move pruning
                }
                if self.sh.limits.pruning.futility && !volatile && depth <= 3 && static_eval + 120 * depth + 60 <= alpha
                {
                    continue; // futility pruning
                }
            }

            self.hashes.push(child.hash());
            let new_depth = depth - 1;
            let score = if searched == 0 {
                -self.negamax(&child, new_depth, -beta, -alpha, ply + 1, true)
            } else {
                let mut r = 0;
                if self.sh.limits.pruning.late_move_reductions
                    && depth >= 3
                    && searched >= 2
                    && quiet
                    && !in_check
                    && !gives_check
                {
                    r = self.lmr[(depth as usize).min(63)][searched.min(63)] as i32;
                    if pv_node {
                        r -= 1;
                    }
                    if m == self.killers[ply][0] || m == self.killers[ply][1] {
                        r -= 1;
                    }
                    r -= (self.history[side.idx()][m.from() as usize][m.to() as usize] / 6000).clamp(-1, 2);
                    r = r.clamp(0, new_depth - 1);
                }
                let mut s = -self.negamax(&child, new_depth - r, -alpha - 1, -alpha, ply + 1, true);
                if s > alpha && r > 0 {
                    s = -self.negamax(&child, new_depth, -alpha - 1, -alpha, ply + 1, true);
                }
                if s > alpha && s < beta {
                    s = -self.negamax(&child, new_depth, -beta, -alpha, ply + 1, true);
                }
                s
            };
            self.hashes.pop();
            if self.stopped {
                return 0;
            }
            searched += 1;

            if score > best_score {
                best_score = score;
                best_move = m;
                if score > alpha {
                    alpha = score;
                    self.pv[ply][ply] = m;
                    let child_len = self.pv_len[ply + 1].max(ply + 1);
                    for j in ply + 1..child_len {
                        self.pv[ply][j] = self.pv[ply + 1][j];
                    }
                    self.pv_len[ply] = child_len;
                    if score >= beta {
                        if quiet {
                            if self.killers[ply][0] != m {
                                self.killers[ply][1] = self.killers[ply][0];
                                self.killers[ply][0] = m;
                            }
                            let bonus = (depth * depth * 16).min(1600);
                            self.update_history(side, m, bonus);
                            for &q in &quiets[..n_quiets] {
                                self.update_history(side, q, -bonus);
                            }
                        }
                        break;
                    }
                }
            }
            if quiet && n_quiets < quiets.len() {
                quiets[n_quiets] = m;
                n_quiets += 1;
            }
        }

        let bound = if best_score >= beta {
            Bound::Lower
        } else if best_score > orig_alpha {
            Bound::Exact
        } else {
            Bound::Upper
        };
        let eval_store = if in_check { 0 } else { static_eval };
        // A root searched without some moves is not this position's true value.
        if !excluding {
            self.sh.tt.store(pos.hash(), best_move, score_to_tt(best_score, ply), eval_store, depth, bound);
        }
        best_score
    }

    fn qsearch(&mut self, pos: &Position, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.pv_len[ply] = ply;
        if self.tick() {
            return 0;
        }
        self.seldepth = self.seldepth.max(ply);
        if ply >= MAX_PLY - 1 {
            return evaluate(pos);
        }
        let in_check = pos.in_check();
        let stand_pat = if in_check { -MATE + ply as i32 } else { evaluate(pos) };
        if stand_pat >= beta {
            return stand_pat;
        }
        alpha = alpha.max(stand_pat);
        let mut best = stand_pat;

        let volatile = volatile_eval(pos);
        let mut moves = if in_check { legal_moves(pos) } else { noisy_moves(pos) };
        if in_check && moves.is_empty() {
            return -MATE + ply as i32;
        }
        let mut scores = [0i32; MAX_MOVES];
        for (i, &m) in moves.iter().enumerate() {
            let victim =
                if m.is_ep() { SEE_VALUE[0] } else { pos.piece_on(m.to()).map_or(0, |p| SEE_VALUE[p.kind().idx()]) };
            let attacker = pos.piece_on(m.from()).map_or(0, |p| p.kind().idx() as i32);
            let promo = m.promotion().map_or(0, |p| SEE_VALUE[p.idx()]);
            scores[i] = (victim + promo) * 8 - attacker;
        }

        for i in 0..moves.len() {
            let m = Self::pick(&mut moves, &mut scores, i);
            if !in_check {
                if m.is_promotion() && m.promotion() != Some(PieceType::Queen) {
                    continue;
                }
                // Delta pruning: even winning this piece cannot raise alpha.
                let gain = scores[i] / 8 + 200;
                if !volatile && !m.is_promotion() && stand_pat + gain <= alpha {
                    continue;
                }
                if see(pos, m) < 0 {
                    continue;
                }
            }
            let child = pos.make_move(m);
            let score = -self.qsearch(&child, -beta, -alpha, ply + 1);
            if self.stopped {
                return 0;
            }
            if score > best {
                best = score;
                if score > alpha {
                    alpha = score;
                    if score >= beta {
                        break;
                    }
                }
            }
        }
        best
    }
}

/// In sparse endgames the evaluation can swing by hundreds of centipawns on
/// a single quiet move or pawn capture (fortresses, KPK, wrong bishop), so
/// margin-based pruning (futility, reverse futility, delta) is unsound there.
#[inline(always)]
fn volatile_eval(pos: &Position) -> bool {
    pos.phase() <= 4
}

/// Mate scores are stored relative to the node, not the root.
#[inline(always)]
fn score_to_tt(s: i32, ply: usize) -> i32 {
    if s >= MATE_BOUND {
        s + ply as i32
    } else if s <= -MATE_BOUND {
        s - ply as i32
    } else {
        s
    }
}

#[inline(always)]
fn score_from_tt(s: i32, ply: usize) -> i32 {
    if s >= MATE_BOUND {
        s - ply as i32
    } else if s <= -MATE_BOUND {
        s + ply as i32
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mate_in_conversion() {
        assert_eq!(mate_in(MATE - 1), Some(1));
        assert_eq!(mate_in(MATE - 3), Some(2));
        assert_eq!(mate_in(-MATE + 2), Some(-1));
        assert_eq!(mate_in(-MATE + 4), Some(-2));
        assert_eq!(mate_in(350), None);
    }

    #[test]
    fn tt_mate_adjustment_roundtrip() {
        for s in [MATE - 5, -MATE + 7, 123, -40] {
            for ply in [0, 3, 17] {
                assert_eq!(score_from_tt(score_to_tt(s, ply), ply), s);
            }
        }
    }
}
