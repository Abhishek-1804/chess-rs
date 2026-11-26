//! Background game-review thread with its own engine, so analysing a game
//! never blocks play (and the bot's transposition table stays untouched).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use chess_engine::analysis::{GameReview, PositionEval, analyze};
use chess_engine::{Engine, Game};

pub struct ReviewJob {
    pub id: u64,
    pub game: Game,
    pub depth: u32,
    pub threads: usize,
    pub stop: Arc<AtomicBool>,
}

#[derive(Debug)]
pub enum ReviewEvent {
    Progress { id: u64, done: usize, total: usize, index: usize, eval: PositionEval },
    Done { id: u64, review: Option<GameReview> },
}

pub struct ReviewHandle {
    tx: Option<Sender<ReviewJob>>,
    rx: Receiver<ReviewEvent>,
    thread: Option<JoinHandle<()>>,
    next_id: u64,
    /// Cancellation flag of the most recent job (raised on drop).
    last_stop: Option<Arc<AtomicBool>>,
}

impl ReviewHandle {
    pub fn spawn(hash_mb: usize, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, jobs) = mpsc::channel::<ReviewJob>();
        let (events, rx) = mpsc::channel::<ReviewEvent>();
        let thread = std::thread::Builder::new()
            .name("chess-review".into())
            .spawn(move || {
                let mut engine = Engine::new(hash_mb, 1);
                while let Ok(job) = jobs.recv() {
                    if job.stop.load(Ordering::Relaxed) {
                        continue;
                    }
                    engine.set_threads(job.threads);
                    let id = job.id;
                    let review = analyze(&engine, &job.game, job.depth, &job.stop, |(done, total), index, eval| {
                        let _ = events.send(ReviewEvent::Progress { id, done, total, index, eval: eval.clone() });
                        wake();
                    });
                    let _ = events.send(ReviewEvent::Done { id, review });
                    wake();
                }
            })
            .expect("spawn review thread");
        ReviewHandle { tx: Some(tx), rx, thread: Some(thread), next_id: 0, last_stop: None }
    }

    /// Queue a review; returns its id and cancellation flag.
    pub fn start(&mut self, game: Game, depth: u32, threads: usize) -> (u64, Arc<AtomicBool>) {
        self.next_id += 1;
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(prev) = self.last_stop.replace(stop.clone()) {
            prev.store(true, Ordering::Relaxed); // one review at a time
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(ReviewJob { id: self.next_id, game, depth, threads, stop: stop.clone() });
        }
        (self.next_id, stop)
    }

    pub fn try_recv(&self) -> Option<ReviewEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for ReviewHandle {
    fn drop(&mut self) {
        if let Some(stop) = &self.last_stop {
            stop.store(true, Ordering::Relaxed);
        }
        self.tx.take(); // closes the channel; the thread exits
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
