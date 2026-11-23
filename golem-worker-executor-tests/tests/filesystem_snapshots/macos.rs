use super::native::*;
use super::*;
use test_r::{test, timeout};

#[test]
#[test_r::tag(local_filesystem)]
#[timeout("4m")]
async fn apfs_restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_filesystem_snapshots_on_apfs;

    let root = tempfile::tempdir()?;
    let context = TestContext::new(last_unique_id);
    restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
        &context,
        initial_file_system,
        "apfs-replay",
        |snapshots| {
            start_with_filesystem_snapshots_on_apfs(
                deps,
                &context,
                Box::from(root.path()),
                snapshots,
            )
        },
    )
    .await
}
