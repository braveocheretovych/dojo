use std::collections::HashMap;
use std::sync::Arc;

use dojo_metrics::Metrics;
use metrics::Histogram;
use parking_lot::Mutex;

#[derive(Metrics, Clone)]
#[metrics(scope = "task_manager.task")]
struct TaskMetrics {
    /// total_first_poll_delay
    total_first_poll_delay: Histogram,
}

type TaskIntervals = Box<dyn Iterator<Item = tokio_metrics::TaskMetrics> + Send + 'static>;

struct TaskManagerMetrics {
    // task monitor for each task
    metrics: Arc<Mutex<HashMap<&'static str, (TaskMetrics, TaskIntervals)>>>,
}

impl dojo_metrics::Report for TaskManagerMetrics {
    fn report(&self) {
        for (metrics, intervals) in self.metrics.lock().values_mut() {
            // Safety: `intervals` is an infinite iterator so it's safe to unwrap here
            let intervals = intervals.next().unwrap();
            metrics.total_first_poll_delay.record(intervals.total_first_poll_delay.as_secs_f64());
        }
    }
}
