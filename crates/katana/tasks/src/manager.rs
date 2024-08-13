use std::any::Any;
use std::future::Future;

use thiserror::Error;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::error;

pub type TaskHandle<T> = JoinHandle<TaskResult<T>>;

#[derive(Debug, Error)]
pub struct PanickedTaskError {
    /// The name of the panicked task.
    task_name: &'static str,
    /// The error that caused the panic. It is a boxed `dyn Any` due to the future returned by
    /// [`catch_unwind`](futures::future::FutureExt::catch_unwind).
    error: Box<dyn Any + Send>,
}

impl std::fmt::Display for PanickedTaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.error.downcast_ref::<String>() {
            None => Ok(()),
            Some(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Debug)]
struct TaskManager {
    /// A handle to the Tokio runtime.
    handle: Handle,
    /// Keep track of currently running tasks.
    tracker: TaskTracker,
    /// Used to cancel all running tasks.
    ///
    /// This is passed to all the tasks spawned by the manager.
    on_cancel: CancellationToken,
}

impl TaskManager {
    /// Create a new [`TaskManager`] from the given Tokio runtime handle.
    pub fn new(handle: Handle) -> Self {
        Self { handle, tracker: TaskTracker::new(), on_cancel: CancellationToken::new() }
    }

    /// Spawn a normal task.
    ///
    /// Normal task can only get cancelled but cannot cancel other tasks unlike critical tasks.
    pub fn spawn<F>(&self, task: F) -> TaskHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.spawn_task(task)
    }

    /// Spawn a critical task with the given name.
    ///
    /// Critical tasks will cancel other tasks when panicked.
    pub fn spawn_critical<F>(&self, name: &'static str, task: F) -> TaskHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_task(self.critical_task(name, task))
    }

    /// Wait until all tasks are shutdown due to cancellation.
    pub async fn wait_shutdown(&self) {
        // need to close the tracker first before waiting
        let _ = self.tracker.close();
        let _ = self.on_cancel.cancelled().await;
        self.tracker.wait().await;
    }

    /// Return the handle to the Tokio runtime that the manager is associated with.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    fn spawn_task<F>(&self, task: F) -> TaskHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let ct = self.on_cancel.clone();
        self.tracker.spawn_on(
            async move {
                tokio::select! {
                    res = task => TaskResult::Completed(res),
                    _ = ct.cancelled() => TaskResult::Cancelled,
                }
            },
            &self.handle,
        )
    }

    fn critical_task<F>(&self, task_name: &'static str, fut: F) -> impl Future<Output = ()>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        use std::panic::AssertUnwindSafe;

        use futures::{FutureExt, TryFutureExt};

        let ct = self.on_cancel.clone();
        AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                // signal to manager to cancel all tasks upon panic
                ct.cancel();
                let error = PanickedTaskError { task_name, error };
                error!(%error, task = %task_name, "Critical task failed.");
            })
            .map(drop)
    }
}

impl Drop for TaskManager {
    fn drop(&mut self) {
        self.on_cancel.cancel();
    }
}

/// A task result that can be either completed or cancelled.
#[derive(Debug, Copy, Clone)]
pub enum TaskResult<T> {
    /// The task completed successfully with the given result.
    Completed(T),
    /// The task was cancelled.
    Cancelled,
}

impl<T> TaskResult<T> {
    /// Returns true if the task was cancelled.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, TaskResult::Cancelled)
    }
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
