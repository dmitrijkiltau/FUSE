/// Lock-free atomic metrics for the `spawn`/`await` concurrency model.
///
/// All counters use `Ordering::Relaxed` — the metrics are advisory/observational
/// and do not need to synchronise with any non-metric operations.  In the
/// worst case a counter lags by one on a core; that is acceptable for
/// dashboards and diagnostics.
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};

/// Tasks that are currently executing (spawned but not yet completed).
static ACTIVE_TASKS: AtomicI64 = AtomicI64::new(0);

/// Total tasks enqueued since process start.
static TOTAL_SPAWNED: AtomicU64 = AtomicU64::new(0);

/// Total tasks that have finished (ok, error, or runtime error).
static TOTAL_COMPLETED: AtomicU64 = AtomicU64::new(0);

/// Tasks that have been submitted to a worker channel but whose job closure
/// has not yet started executing (i.e., sitting in the channel buffer).
static QUEUE_DEPTH: AtomicI64 = AtomicI64::new(0);

/// Sum of all task wall-clock durations in microseconds (enqueue → complete).
static TOTAL_LATENCY_US: AtomicU64 = AtomicU64::new(0);

/// Task completion latency histogram (wall-clock from enqueue to completion).
///
/// Buckets: [0] <1 ms, [1] 1–10 ms, [2] 10–100 ms, [3] 100 ms–1 s, [4] ≥1 s.
static LATENCY_HIST: [AtomicU64; 5] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

/// Number of worker threads in the task pool (set once at pool initialisation).
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0);

// ---------------------------------------------------------------------------
// Recording helpers (called from task_pool and spawn hostcall)
// ---------------------------------------------------------------------------

/// Called when a task is submitted to the pool (before channel send).
pub(crate) fn record_task_enqueued() {
    TOTAL_SPAWNED.fetch_add(1, Ordering::Relaxed);
    QUEUE_DEPTH.fetch_add(1, Ordering::Relaxed);
    ACTIVE_TASKS.fetch_add(1, Ordering::Relaxed);
}

/// Called when a worker thread picks up the job (job closure starts).
pub(crate) fn record_task_started() {
    QUEUE_DEPTH.fetch_add(-1, Ordering::Relaxed);
}

/// Called when a task's job closure has returned.
/// `elapsed_us` is the wall-clock duration from enqueue to completion in µs.
pub(crate) fn record_task_completed(elapsed_us: u64) {
    ACTIVE_TASKS.fetch_add(-1, Ordering::Relaxed);
    TOTAL_COMPLETED.fetch_add(1, Ordering::Relaxed);
    TOTAL_LATENCY_US.fetch_add(elapsed_us, Ordering::Relaxed);
    LATENCY_HIST[latency_bucket(elapsed_us)].fetch_add(1, Ordering::Relaxed);
}

/// Store the worker count once, at pool initialisation.
pub(crate) fn set_worker_count(n: usize) {
    WORKER_COUNT.store(n, Ordering::Relaxed);
}

fn latency_bucket(elapsed_us: u64) -> usize {
    match elapsed_us {
        0..=999 => 0,
        1_000..=9_999 => 1,
        10_000..=99_999 => 2,
        100_000..=999_999 => 3,
        _ => 4,
    }
}

// ---------------------------------------------------------------------------
// Snapshot (called by observability layer and --diagnostics json)
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of all concurrency metrics.
#[derive(Debug, Clone)]
pub struct ConcurrencySnapshot {
    pub active_tasks: i64,
    pub total_spawned: u64,
    pub total_completed: u64,
    pub queue_depth: i64,
    /// Latency histogram buckets: <1 ms, 1–10 ms, 10–100 ms, 100 ms–1 s, ≥1 s.
    pub latency_hist: [u64; 5],
    /// Mean task latency in microseconds (0 if no tasks have completed).
    pub avg_latency_us: f64,
    pub worker_count: usize,
}

pub fn snapshot() -> ConcurrencySnapshot {
    let total_completed = TOTAL_COMPLETED.load(Ordering::Relaxed);
    let total_latency_us = TOTAL_LATENCY_US.load(Ordering::Relaxed);
    let avg_latency_us = if total_completed > 0 {
        total_latency_us as f64 / total_completed as f64
    } else {
        0.0
    };
    ConcurrencySnapshot {
        active_tasks: ACTIVE_TASKS.load(Ordering::Relaxed),
        total_spawned: TOTAL_SPAWNED.load(Ordering::Relaxed),
        total_completed,
        queue_depth: QUEUE_DEPTH.load(Ordering::Relaxed),
        latency_hist: [
            LATENCY_HIST[0].load(Ordering::Relaxed),
            LATENCY_HIST[1].load(Ordering::Relaxed),
            LATENCY_HIST[2].load(Ordering::Relaxed),
            LATENCY_HIST[3].load(Ordering::Relaxed),
            LATENCY_HIST[4].load(Ordering::Relaxed),
        ],
        avg_latency_us,
        worker_count: WORKER_COUNT.load(Ordering::Relaxed),
    }
}
