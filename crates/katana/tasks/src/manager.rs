use std::{any::Any, future::Future};

use thiserror::Error;
use tokio::{runtime::Handle, task::JoinHandle};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::error;

pub type TaskHandle<T> = JoinHandle<TaskResult<T>>;

#[derive(Debug, Error)]
pub struct PanickedTaskError {
    task_name: &'static str,
    error: Box<dyn Any + Send>,
}

impl std::fmt::Display for PanickedTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.task_name;
        match self.error.downcast_ref::<String>() {
            Some(msg) => write!(f, "Task `{name}` panicked with error: {msg}"),
            None => write!(f, "Task `{name}` panicked"),
        }
    }
}

struct TaskManager {
    handle: Handle,
    tracker: TaskTracker,
    on_cancel: CancellationToken,
}

impl TaskManager {
    pub fn new(handle: Handle) -> Self {
        Self { handle, tracker: TaskTracker::new(), on_cancel: CancellationToken::new() }
    }

    // spawn a task
    //
    // normal task can only get cancelled but cannot cancel other tasks unlike critical tasks
    pub fn spawn<F>(&self, task: F) -> TaskHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let is_cancelled = self.on_cancel.clone();
        self.tracker.spawn_on(
            async move {
                tokio::select! {
                    res = task => TaskResult::Completed(res),
                    _ = is_cancelled.cancelled() => TaskResult::Cancelled,
                }
            },
            &self.handle,
        )
    }

    // spawn a critical task with the given name
    //
    // critical tasks can cancel other tasks when they panic
    pub fn spawn_critical<F>(&self, name: &'static str, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let task = self.create_critical_task(name, task);
        let is_cancelled = self.on_cancel.clone();

        self.tracker.spawn_on(
            async move {
                tokio::select! {
                    res = task => TaskResult::Completed(res),
                    _ = is_cancelled.cancelled() => TaskResult::Cancelled,
                }
            },
            &self.handle,
        );
    }

    fn create_critical_task<F>(&self, task_name: &'static str, task: F) -> impl Future<Output = ()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        use futures::{FutureExt, TryFutureExt};
        use std::panic::AssertUnwindSafe;

        // upon panic, signal to manager to cancel all tasks
        let ct = self.on_cancel.clone();
        AssertUnwindSafe(task)
            .catch_unwind()
            .map_err(move |error| {
                ct.cancel();
                let error = PanickedTaskError { task_name, error };
                error!(%error, "Critical task failed.");
            })
            .map(drop)
    }

    pub async fn wait_shutdown(&self) {
        // need to close the tracker first before waiting
        let _ = self.tracker.close();
        let _ = self.on_cancel.cancelled().await;
        self.tracker.wait().await;
    }
}

#[derive(Debug, Clone)]
pub enum TaskResult<T> {
    Completed(T),
    Cancelled,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn goofy_ahh() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let manager = TaskManager::new(rt.handle().clone());

        manager.spawn_critical("task 1", async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!("task 1")
        });

        manager.spawn_critical("task 2", async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!("task 2")
        });

        manager.spawn_critical("task 3", async {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                println!("im doing stuff in task 3")
            }
        });

        manager.spawn_critical("task 4", async {
            tokio::time::sleep(Duration::from_secs(3)).await;
            panic!("ahh i panicked")
        });

        manager.spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                println!("im doing stuff")
            }
        });

        rt.block_on(manager.wait_shutdown());
    }
}
