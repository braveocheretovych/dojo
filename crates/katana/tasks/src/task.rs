use std::any::Any;
use std::future::Future;
use std::marker::PhantomData;
use std::task::Poll;

use futures::future::{BoxFuture, Either};
use thiserror::Error;
use tokio_metrics::TaskMonitor;
use tracing::error;

#[derive(Debug)]
pub struct TaskBuilder<'fut, F, T>
where
    F: Future<Output = T>,
{
    fut: F,
    metrics: bool,
    name: Option<String>,
    _marker: PhantomData<&'fut ()>,
}

impl<'fut, F, T> TaskBuilder<'fut, F, T>
where
    F: Future<Output = T>,
    F: Send + 'fut,
{
    pub fn new(fut: F) -> Self {
        Self { name: None, metrics: false, fut, _marker: PhantomData }
    }

    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    pub fn with_metrics(mut self) -> Self {
        self.metrics = true;
        self
    }

    pub fn build(self) -> Task<'fut, T> {
        if self.metrics {
            let monitor = TaskMonitor::new();
            let fut = TaskFut::new(monitor.instrument(self.fut));
            Task { fut, monitor: Some(monitor) }
        } else {
            let fut = TaskFut::new(self.fut);
            Task { fut, monitor: None }
        }
    }
}

impl<'fut, F> TaskBuilder<'fut, F, ()>
where
    F: Future<Output = ()>,
{
    pub fn critical(self) -> CriticalTaskBuilder<'fut, F> {
        CriticalTaskBuilder {
            fut: self.fut,
            name: self.name,
            metrics: self.metrics,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug)]
pub struct CriticalTaskBuilder<'fut, F>
where
    F: Future<Output = ()>,
{
    fut: F,
    metrics: bool,
    name: Option<String>,
    _marker: PhantomData<&'fut ()>,
}

impl<'fut, F> CriticalTaskBuilder<'fut, F>
where
    F: Future<Output = ()>,
    F: Send + 'fut,
{
    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    pub fn with_metrics(mut self) -> Self {
        self.metrics = true;
        self
    }

    pub fn build(self) -> Task<'fut, ()> {
        use std::panic::AssertUnwindSafe;

        use futures::{FutureExt, TryFutureExt};

        let Self { fut, name, metrics, .. } = self;

        let fut = AssertUnwindSafe(fut)
            .catch_unwind()
            .map_err(move |error| {
                let error = PanickedTaskError { error };
                error!(%error, task = name, "Critical task failed.");
            })
            .map(drop);

        if metrics {
            let monitor = TaskMonitor::new();
            let fut = TaskFut::critical(monitor.instrument(fut));
            Task { fut, monitor: Some(monitor) }
        } else {
            let fut = TaskFut::critical(fut);
            Task { fut, monitor: None }
        }
    }
}

pub struct Task<'a, T> {
    fut: TaskFut<'a, T>,
    monitor: Option<TaskMonitor>,
}

pub(crate) enum TaskFut<'a, T> {
    Normal(BoxFuture<'a, T>),
    Critical(BoxFuture<'a, ()>),
}

impl<'a, T> TaskFut<'a, T> {
    fn new(fut: impl Future<Output = T> + Send + 'a) -> Self {
        TaskFut::Normal(Box::pin(fut) as BoxFuture<'_, T>)
    }
}

impl<'a> TaskFut<'a, ()> {
    fn critical(fut: impl Future<Output = ()> + Send + 'a) -> Self {
        TaskFut::Critical(Box::pin(fut) as BoxFuture<'_, ()>)
    }
}

impl<'a, T> Future for TaskFut<'a, T> {
    type Output = T;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            TaskFut::Normal(fut) => fut.as_mut().poll(cx),
            TaskFut::Critical(fut) => match fut.as_mut().poll(cx) {
                Poll::Pending => Poll::Pending,
                // Safety: the return type of critical tasks is ()
                Poll::Ready(()) => Poll::Ready(unsafe { std::mem::zeroed() }),
            },
        }
    }
}

#[derive(Debug, Error)]
pub struct PanickedTaskError {
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

#[test]
fn foo() {
    let task = TaskBuilder::new(async { println!("ohayo") }).with_metrics().critical().build();
    matches!(task.fut, TaskFut::Critical(_));
    let task = TaskBuilder::new(async { println!("ohayo 2") }).name("task 2").build();
    matches!(task.fut, TaskFut::Normal(_));
}
