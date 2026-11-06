//! Background engine thread. The UI never blocks on search: it sends
//! [`Request`]s over a channel and receives [`Response`]s, with each search
//! carrying its own cancellation flag. The worker owns the engine (and its
//! multi-GB transposition table) and the persistent position cache.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Instant;

use chess_engine::cache::CacheStats;
use chess_engine::{Engine, Limits, Move, Position, PositionCache, RootRecord, SearchInfo, SearchResult, legal_moves};

/// What a search is for: the bot's move, or a hint for the human.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    BotMove,
    Hint,
}

pub struct SearchJob {
    pub id: u64,
    pub purpose: Purpose,
    pub pos: Position,
    pub history: Vec<u64>,
    pub depth: u32,
    pub threads: usize,
    pub stop: Arc<AtomicBool>,
}

pub enum Request {
    Search(Box<SearchJob>),
    /// Merge deep TT entries into the persistent cache and write it to disk.
    SaveCache,
    ClearCache,
    Shutdown,
}

#[derive(Debug)]
pub enum Response {
    Info { id: u64, info: SearchInfo },
    Done { id: u64, purpose: Purpose, result: SearchResult, from_cache: bool },
    CacheReady { stats: CacheStats, warm_ms: u128 },
    CacheSaved { stats: CacheStats, error: Option<String> },
    CacheStats(CacheStats),
}

pub struct EngineConfig {
    pub hash_mb: usize,
    pub threads: usize,
    pub cache_path: Option<PathBuf>,
}

pub struct EngineHandle {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    thread: Option<JoinHandle<()>>,
    next_id: u64,
    nodes: Arc<AtomicU64>,
}

impl EngineHandle {
    /// Spawn the worker. `wake` is called after every response (the UI
    /// passes `ctx.request_repaint`).
    pub fn spawn(cfg: EngineConfig, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, worker_rx) = mpsc::channel::<Request>();
        let (worker_tx, rx) = mpsc::channel::<Response>();
        // The engine (and its table) is built here and moved into the worker,
        // keeping a handle on its live node counter for the UI.
        let engine = Engine::new(cfg.hash_mb, cfg.threads);
        let nodes = engine.node_counter();
        let thread = std::thread::Builder::new()
            .name("chess-engine".into())
            .spawn(move || worker(engine, cfg, worker_rx, worker_tx, wake))
            .expect("spawn engine thread");
        EngineHandle { tx, rx, thread: Some(thread), next_id: 1, nodes }
    }

    /// Positions searched so far by the running (or most recent) search.
    pub fn nodes_searched(&self) -> u64 {
        self.nodes.load(Ordering::Relaxed)
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    pub fn try_recv(&self) -> Option<Response> {
        self.rx.try_recv().ok()
    }

    /// Blocking receive with timeout (used by tests and shutdown).
    pub fn recv_timeout(&self, d: std::time::Duration) -> Option<Response> {
        self.rx.recv_timeout(d).ok()
    }

    /// Save the cache and stop the worker, waiting for it to finish.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Request::SaveCache);
        let _ = self.tx.send(Request::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.shutdown();
        }
    }
}

/// A cached root result may only be reused when the game history cannot
/// change its value: no prior occurrence of the position (repetition) and
/// the fifty-move counter far from triggering.
fn cache_usable(job: &SearchJob) -> bool {
    job.pos.halfmove_clock() < 80 && !job.history.contains(&job.pos.hash())
}

fn worker(mut engine: Engine, cfg: EngineConfig, rx: Receiver<Request>, tx: Sender<Response>, wake: impl Fn()) {
    let send = |r: Response| {
        let _ = tx.send(r);
        wake();
    };
    let mut cache = match &cfg.cache_path {
        Some(p) => PositionCache::open(p),
        None => PositionCache::in_memory(),
    };
    let t = Instant::now();
    cache.warm(engine.tt());
    send(Response::CacheReady { stats: cache.stats(), warm_ms: t.elapsed().as_millis() });

    while let Ok(req) = rx.recv() {
        match req {
            Request::Search(job) => {
                if job.stop.load(Ordering::Relaxed) {
                    continue; // cancelled before it started
                }
                let legal = legal_moves(&job.pos);
                if cache_usable(&job)
                    && let Some(rec) = cache.lookup(job.pos.hash(), job.depth)
                    && legal.contains(&rec.mv)
                {
                    let result = SearchResult {
                        best_move: Some(rec.mv),
                        score: rec.score,
                        depth: rec.depth,
                        seldepth: rec.depth,
                        nodes: 0,
                        time: std::time::Duration::ZERO,
                        pv: vec![rec.mv],
                    };
                    send(Response::Done { id: job.id, purpose: job.purpose, result, from_cache: true });
                    send(Response::CacheStats(cache.stats()));
                    continue;
                }
                engine.set_threads(job.threads);
                let id = job.id;
                let result = engine.search(&job.pos, &job.history, &Limits::depth(job.depth), &job.stop, |info| {
                    send(Response::Info { id, info: info.clone() });
                });
                let completed = !job.stop.load(Ordering::Relaxed) && result.depth >= job.depth;
                if completed && let Some(mv) = result.best_move {
                    cache.record(job.pos.hash(), RootRecord { mv, score: result.score, depth: result.depth });
                }
                send(Response::Done { id, purpose: job.purpose, result, from_cache: false });
                send(Response::CacheStats(cache.stats()));
            }
            Request::SaveCache => {
                cache.absorb(engine.tt());
                let error = cache.save().err().map(|e| e.to_string());
                send(Response::CacheSaved { stats: cache.stats(), error });
            }
            Request::ClearCache => {
                cache.clear();
                engine.clear();
                let error = cache.save().err().map(|e| e.to_string());
                send(Response::CacheSaved { stats: cache.stats(), error });
            }
            Request::Shutdown => break,
        }
    }
}

/// Convenience for tests: is `m` legal in `pos`?
pub fn is_legal(pos: &Position, m: Move) -> bool {
    legal_moves(pos).contains(&m)
}
