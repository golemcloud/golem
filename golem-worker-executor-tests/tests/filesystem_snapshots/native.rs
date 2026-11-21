use super::*;
use pretty_assertions::assert_eq;

/// The settings of the store over the blob storage of the executor.
pub(super) fn enabled_snapshots() -> FilesystemSnapshotsConfig {
    FilesystemSnapshotsConfig::Enabled(Box::new(
        FilesystemSnapshotStoreConfig::new(&"5a".repeat(64), Duration::from_secs(60), 2, 2)
            .expect("valid store settings")
            .with_uploads(uploads(Duration::from_secs(30))),
    ))
}

/// Starts the agent `name` with read-only and read-write initial files on executors that `start`
/// makes, changes its files with a write, renames, a hard link and a symlink, and checks that a
/// restart from a snapshot and a restart with a full replay both give the tree of the live agent.
pub(super) async fn restart_from_a_snapshot_gives_the_tree_of_a_full_replay<F>(
    context: &TestContext,
    initial_file_system: &PrecompiledComponent,
    name: &str,
    start: impl Fn(FilesystemSnapshotsConfig) -> F,
) -> anyhow::Result<()>
where
    F: std::future::Future<Output = anyhow::Result<TestWorkerExecutor>>,
{
    let executor = start(enabled_snapshots()).await?;
    let agent = Agent::start(
        &executor,
        context,
        initial_file_system,
        name,
        &[
            entry("foo.txt", "/ro-renamed.txt", AgentFilePermissions::ReadOnly),
            entry(
                "bar.txt",
                "/dir/ro-in-dir.txt",
                AgentFilePermissions::ReadOnly,
            ),
            entry(
                "baz.txt",
                "/rw-modified.txt",
                AgentFilePermissions::ReadWrite,
            ),
        ],
    )
    .await?;
    agent
        .apply_all(
            &executor,
            &[
                Operation::Write {
                    path: "rw-modified.txt",
                    content: "modified by the agent",
                },
                Operation::Rename {
                    from: "ro-renamed.txt",
                    to: "renamed.txt",
                },
                Operation::Rename {
                    from: "dir",
                    to: "moved-dir",
                },
                Operation::Write {
                    path: "agent.txt",
                    content: "agent data",
                },
                Operation::Link {
                    existing: "agent.txt",
                    link: "agent-alias.txt",
                },
                Operation::Symlink {
                    link: "pointer",
                    target: "agent.txt",
                },
            ],
        )
        .await?;
    agent.confirmed(&executor).await?;
    let live = agent.describe(&executor).await?;
    executor.release().await?;

    let restarted = start(enabled_snapshots()).await?;
    let mut events = restarted.capture_output(&agent.worker_id).await?;
    let restored = agent.describe(&restarted).await?;
    assert_snapshot_recovery_loaded(&mut events).await;
    let restored_shape = invocation_shape(&restarted.stored_oplog(&agent.worker_id.agent_id).await);
    restarted.release().await?;
    let replaying = start(FilesystemSnapshotsConfig::default()).await?;
    let replayed = agent.describe(&replaying).await?;
    let replayed_shape = invocation_shape(&replaying.stored_oplog(&agent.worker_id.agent_id).await);

    assert_eq!(restored, live);
    assert_eq!(replayed, live);
    let settled = || InvocationShape {
        applied: 6,
        ..InvocationShape::settled()
    };
    assert_eq!(restored_shape, settled());
    assert_eq!(replayed_shape, settled());
    Ok(())
}
