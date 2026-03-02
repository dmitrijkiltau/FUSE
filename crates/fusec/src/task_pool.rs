use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Instant;

type Job = Box<dyn FnOnce() + Send + 'static>;

pub(crate) fn submit<F>(job: F)
where
    F: FnOnce() + Send + 'static,
{
    TaskPool::global().submit(Box::new(job));
}

/// Per-worker-channel round-robin task pool.
///
/// The previous design used a single `Arc<Mutex<Receiver<Job>>>` shared
/// across all workers.  Under parallel-spawn workloads every job dispatch
/// acquired that mutex, serialising job pickup and creating thundering-herd
/// wakeups.
///
/// This design gives each worker its own `mpsc::channel`.  The dispatcher
/// round-robins assignments with an `AtomicUsize`, so no mutex is ever
/// taken on the critical submit path.  Trade-off: if one worker is busy
/// other workers may sit idle.  For short-lived Fuse tasks (compute,
/// DB-round-trip) this is almost always faster than shared-queue contention.
struct TaskPool {
    senders: Vec<Sender<Job>>,
    next: AtomicUsize,
}

impl TaskPool {
    fn global() -> &'static Self {
        static POOL: OnceLock<TaskPool> = OnceLock::new();
        POOL.get_or_init(Self::new)
    }

    fn new() -> Self {
        let workers = thread::available_parallelism()
            .map(|n| n.get().max(2))
            .unwrap_or(2);
        crate::concurrency_metrics::set_worker_count(workers);
        let mut senders = Vec::with_capacity(workers);
        for idx in 0..workers {
            let (tx, rx) = mpsc::channel::<Job>();
            senders.push(tx);
            let _ = thread::Builder::new()
                .name(format!("fuse-task-{idx}"))
                .spawn(move || {
                    for job in rx {
                        job();
                    }
                });
        }
        Self {
            senders,
            next: AtomicUsize::new(0),
        }
    }

    fn submit(&self, job: Job) {
        // Record enqueue before wrapping so the metric is always incremented
        // even if the channel send fails.
        crate::concurrency_metrics::record_task_enqueued();
        let enqueue_time = Instant::now();
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.senders.len();
        let wrapped: Job = Box::new(move || {
            crate::concurrency_metrics::record_task_started();
            job();
            crate::concurrency_metrics::record_task_completed(
                enqueue_time.elapsed().as_micros() as u64,
            );
        });
        // If send fails (worker thread has exited), the job is silently
        // dropped.  This matches the previous behaviour.
        let _ = self.senders[idx].send(wrapped);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    use super::submit;

    #[test]
    fn runs_multiple_jobs_concurrently() {
        let barrier = Arc::new(Barrier::new(3));
        let (tx, rx) = mpsc::channel::<String>();

        for name in ["a", "b"] {
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            submit(move || {
                let _ = tx.send(format!("start:{name}"));
                barrier.wait();
                let _ = tx.send(format!("done:{name}"));
            });
        }

        let mut started = HashSet::new();
        while started.len() < 2 {
            let msg = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("expected both jobs to start");
            if let Some(name) = msg.strip_prefix("start:") {
                started.insert(name.to_string());
            }
        }
        assert_eq!(started.len(), 2, "expected two distinct started jobs");

        barrier.wait();

        let mut finished = HashSet::new();
        while finished.len() < 2 {
            let msg = rx
                .recv_timeout(Duration::from_secs(2))
                .expect("expected both jobs to finish");
            if let Some(name) = msg.strip_prefix("done:") {
                finished.insert(name.to_string());
            }
        }
        assert_eq!(finished.len(), 2, "expected two distinct finished jobs");
    }
}
