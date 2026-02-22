use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

type Job = Box<dyn FnOnce() + Send + 'static>;

pub(crate) fn submit<F>(job: F)
where
    F: FnOnce() + Send + 'static,
{
    TaskPool::global().submit(Box::new(job));
}

struct TaskPool {
    tx: Sender<Job>,
}

impl TaskPool {
    fn global() -> &'static Self {
        static POOL: OnceLock<TaskPool> = OnceLock::new();
        POOL.get_or_init(Self::new)
    }

    fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let workers = thread::available_parallelism()
            .map(|n| n.get().max(2))
            .unwrap_or(2);
        let shared_rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        for idx in 0..workers {
            let worker_rx = Arc::clone(&shared_rx);
            let name = format!("fuse-task-{idx}");
            let _ = thread::Builder::new().name(name).spawn(move || {
                loop {
                    let job = {
                        let guard = match worker_rx.lock() {
                            Ok(guard) => guard,
                            Err(_) => break,
                        };
                        match guard.recv() {
                            Ok(job) => job,
                            Err(_) => break,
                        }
                    };
                    job();
                }
            });
        }
        Self { tx }
    }

    fn submit(&self, job: Job) {
        let _ = self.tx.send(job);
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
