use super::*;
use golem_worker_executor::services::resource_limits::ResourceUsageFlusher;

mod compute;
mod configuration;
mod monitor;
mod reconstruction;
mod storage;

test_r::tag_suite!(compute, group5);
test_r::tag_suite!(monitor, group5);
test_r::tag_suite!(reconstruction, group5);
test_r::tag_suite!(storage, group5);
