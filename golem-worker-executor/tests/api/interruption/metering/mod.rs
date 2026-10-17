use super::*;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;
use golem_worker_executor::worker::Worker;
use golem_worker_executor::workerctx::default::Context as WorkerContext;

#[derive(Debug)]
struct ObservedUsageFlusher {
    owner: std::sync::Weak<Worker<WorkerContext>>,
    send: tokio::sync::mpsc::UnboundedSender<(u64, u64)>,
}

impl ResourceUsageFlusher for ObservedUsageFlusher {
    fn flush_usage(&self) {
        if let Some(owner) = self.owner.upgrade()
            && owner.monthly_window_active_for_test()
        {
            let current = owner.current_monthly_proposal_for_test();
            let _ = self
                .send
                .send((current.diagnostic_window_id, current.policy_revision));
        }
    }
}

mod compute;
mod configuration;
mod monitor;
mod reconstruction;
mod storage;

test_r::tag_suite!(compute, group5);
test_r::tag_suite!(monitor, group5);
test_r::tag_suite!(reconstruction, group5);
test_r::tag_suite!(storage, group5);
