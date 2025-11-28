pub mod debug_mode;
pub mod debug_tests;
pub mod services;

pub use debug_mode::dsl::TestDslDebugMode;
pub use debug_mode::start_debug_worker_executor;
use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_worker_executor_test_utils::{LastUniqueId, WorkerExecutorTestDependencies};
use std::fmt::Debug;
use std::sync::atomic::AtomicU16;
use test_r::test_dep;

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

// Dependencies
#[test_dep]
pub fn tracing() -> Tracing {
    init_tracing_with_default_debug_env_filter(&TracingConfig::test_pretty_without_time(
        "debugging-executor-tests",
    ));

    Tracing
}

#[test_dep]
pub fn last_unique_id() -> LastUniqueId {
    LastUniqueId {
        id: AtomicU16::new(0),
    }
}

#[test_dep]
pub async fn test_dependencies(_tracing: &Tracing) -> WorkerExecutorTestDependencies {
    WorkerExecutorTestDependencies::new().await
}
