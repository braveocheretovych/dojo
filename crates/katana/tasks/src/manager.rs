use std::{future::Future, sync::Arc};

use tokio::{runtime::Handle, sync::Notify, task::JoinSet};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

struct CriticalTasks {
    handle: Handle,
    shutdown_signal: Arc<Notify>,
    cancellation_token: CancellationToken,
    critical_tasks: JoinSet<()>,
}

impl CriticalTasks {
    fn spawn<F>(&mut self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        use futures::{FutureExt, TryFutureExt};
        use std::panic::AssertUnwindSafe;

        let ct = self.cancellation_token.clone();
        let shutdown_signal = self.shutdown_signal.clone();
        let task = AssertUnwindSafe(task)
            .catch_unwind()
            .map_err(move |error| {
                println!("error happened: {error:?}");
                // let _ = shutdown_signal.notify_one();
                ct.cancel();
                println!("send cancel signal")
            })
            .map(drop);

        self.critical_tasks.spawn_on(task, &self.handle);
    }
}

struct TokioManager {
    handle: Handle,
    on_shutdown: Arc<Notify>,
    critical_tasks: CriticalTasks,
}

impl TokioManager {
    fn new(handle: Handle) -> Self {
        let notify = Arc::new(Notify::new());
        let cancellation_token = CancellationToken::new();

        let c = CriticalTasks {
            cancellation_token,
            handle: handle.clone(),
            critical_tasks: JoinSet::new(),
            shutdown_signal: Arc::clone(&notify),
        };

        Self { critical_tasks: c, handle, on_shutdown: notify }
    }

    fn spawn_critical<F>(&mut self, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let _ = self.critical_tasks.spawn(task);
    }

    async fn wait_shutdown(mut self) {
        let _ = self.on_shutdown.notified().await;
        self.critical_tasks.critical_tasks.shutdown().await;
    }

    fn wait_shutdown_or_ctrl_c_signal(mut self) {}
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    use super::*;

    #[test]
    fn goofy_ahh() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(8)
            .thread_name_fn(|| {
                static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
                let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
                format!("my-pool-{}", id)
            })
            .enable_all()
            .on_thread_stop(|| {
                println!("thread stopped {:?}", std::thread::current().name());
            })
            .build()
            .unwrap();

        let mut manager = TokioManager::new(rt.handle().clone());

        manager.spawn_critical(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!("task 1")
        });

        manager.spawn_critical(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!("task 2")
        });

        manager.spawn_critical(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            println!("task 3")
        });

        manager.spawn_critical(async {
            tokio::time::sleep(Duration::from_secs(3)).await;
            panic!("ahh i panicked")
        });

        manager.spawn_critical(async {
            println!("thread {:?}", std::thread::current().name());

            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                println!("im doing stuff")
            }
        });

        manager.wait_shutdown();
    }
}
