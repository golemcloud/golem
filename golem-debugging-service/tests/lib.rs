pub mod debug_mode;
pub mod debug_tests;
pub mod services;

pub use debug_mode::dsl::TestDslDebugMode;
pub use debug_mode::start_debug_worker_executor;
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_worker_executor_test_utils::{
    LastUniqueId, LastUniqueIdOwner, WorkerExecutorTestDependencies,
};
use std::fmt::Debug;
use test_r::test_dep;

test_r::enable!();

#[derive(Debug)]
pub struct Tracing;

// Dependencies
#[test_dep(scope = PerWorker)]
pub fn tracing() -> Tracing {
    let _ = rustls::crypto::ring::default_provider().install_default();

    init_tracing_with_default_debug_env_filter(&TracingConfig::test_pretty_without_time(
        "debugging-executor-tests",
    ));

    Tracing
}

// Globally unique id allocator served by a single AtomicU64 in the
// parent process. Workers receive a `LastUniqueId` stub that round-trips
// each `next()` call to the parent — no per-worker partitioning, no
// `u16` saturation risk, and uniqueness holds across the whole suite
// regardless of `--test-threads`.
#[test_dep(scope = HostedRpc, stub = LastUniqueId)]
pub fn last_unique_id_owner() -> LastUniqueIdOwner {
    LastUniqueIdOwner::new()
}

// Phase 3.4: `WorkerExecutorTestDependencies` is a Hosted dep so workers
// can run in parallel under capture without each spawning its own Redis,
// TempDirs, and component cache. The parent constructs once and ships a
// descriptor; each worker reconstructs an equivalent struct that attaches
// to the parent's resources via `HostedDep::from_descriptor`.
//
// Hosted owner constructors cannot depend on other test_deps, so we drop
// the `&Tracing` parameter here. Tracing remains a separate PerWorker
// dep installed inside each worker subprocess.
#[test_dep(scope = Hosted)]
pub async fn test_dependencies() -> WorkerExecutorTestDependencies {
    WorkerExecutorTestDependencies::new().await
}
