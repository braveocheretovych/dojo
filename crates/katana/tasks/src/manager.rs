pub trait TaskRuntime {
    fn spawn_task(&self);

    fn spawn_blocking_task(&self);

    fn spawn_critical_task(&self);
}
