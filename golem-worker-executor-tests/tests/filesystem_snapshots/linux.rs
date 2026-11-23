use super::native::*;
use super::*;
use pretty_assertions::assert_eq;
use test_r::{test, timeout};

fn reflink_xfs_test_root() -> PathBuf {
    std::env::var_os("GOLEM_REFLINK_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_REFLINK_XFS_TEST_ROOT must name the mounted XFS test root without quotas")
}

fn managed_xfs_test_root() -> PathBuf {
    std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root")
}

const MANAGED_XFS_DISK_SPACE: u64 = 64 * 1024 * 1024;

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("4m")]
async fn managed_xfs_restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_filesystem_snapshots_on_managed_xfs;

    let context = TestContext::new(last_unique_id);
    restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
        &context,
        initial_file_system,
        "managed-xfs-replay",
        |snapshots| {
            start_with_filesystem_snapshots_on_managed_xfs(
                deps,
                &context,
                MANAGED_XFS_DISK_SPACE,
                managed_xfs_test_root(),
                snapshots,
            )
        },
    )
    .await
}

#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("4m")]
async fn reflink_xfs_restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_filesystem_snapshots_on_reflink_xfs;

    let context = TestContext::new(last_unique_id);
    restart_from_a_snapshot_gives_the_tree_of_a_full_replay(
        &context,
        initial_file_system,
        "reflink-xfs-replay",
        |snapshots| {
            start_with_filesystem_snapshots_on_reflink_xfs(
                deps,
                &context,
                reflink_xfs_test_root(),
                snapshots,
            )
        },
    )
    .await
}

#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("2m")]
async fn reflink_xfs_with_filesystem_metering_fails_at_startup(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_filesystem_metering_on_reflink_xfs;

    let context = TestContext::new(last_unique_id);
    match start_with_filesystem_metering_on_reflink_xfs(deps, &context, reflink_xfs_test_root())
        .await
    {
        Ok(_) => Err(anyhow!(
            "the executor started with filesystem metering on XFS storage without project quotas"
        )),
        Err(error) => {
            let message = format!("{error:#}");
            assert!(
                message.contains("filesystem metering requires XFS storage with project quotas"),
                "{message}"
            );
            Ok(())
        }
    }
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("4m")]
async fn managed_xfs_manual_update_brings_the_files_into_the_target_revision(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_filesystem_snapshots_on_managed_xfs;

    let context = TestContext::new(last_unique_id);
    let start = || {
        start_with_filesystem_snapshots_on_managed_xfs(
            deps,
            &context,
            MANAGED_XFS_DISK_SPACE,
            managed_xfs_test_root(),
            enabled_snapshots(),
        )
    };
    let executor = start().await?;
    let agent = Agent::start(
        &executor,
        &context,
        initial_file_system,
        "managed-xfs-manual-update",
        &[entry(
            "foo.txt",
            "/ro-old.txt",
            AgentFilePermissions::ReadOnly,
        )],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[
                Operation::Mkdir { path: "work" },
                Operation::Write {
                    path: "work/state.db",
                    content: "rows",
                },
            ],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let updated = executor
        .update_component_with_files(
            &agent.component.id,
            AGENT_TYPE,
            "it_initial_file_system_release",
            vec![entry(
                "bar.txt",
                "/ro-new.txt",
                AgentFilePermissions::ReadOnly,
            )],
        )
        .await?;
    executor
        .manual_update_worker(&agent.worker_id, updated.revision, false)
        .await?;
    executor
        .wait_for_component_revision(&agent.worker_id, updated.revision, Duration::from_secs(60))
        .await?;
    let updated_agent = Agent {
        component: updated,
        agent: agent.agent.clone(),
        worker_id: agent.worker_id.clone(),
    };
    let after_update = updated_agent.describe(&executor).await?;
    executor.release().await?;
    let restarted = start().await?;

    assert_eq!(
        after_update,
        [
            r#"ro-new.txt file links=1 writable=false content="bar\n""#,
            "work dir",
            r#"work/state.db file links=1 writable=true content="rows""#,
        ]
        .map(String::from)
    );
    assert_eq!(updated_agent.describe(&restarted).await?, after_update);
    Ok(())
}
