use super::*;
use golem_service_base::storage::blob::agent_path_segment;
use golem_worker_executor_test_utils::{
    start_with_agent_storage_quota_and_pressure_without_metering_on_managed_xfs,
    start_with_agent_storage_quota_on_managed_xfs,
    start_with_agent_storage_quota_without_metering_on_managed_xfs,
    start_with_mutable_agent_storage_quota_on_managed_xfs,
    start_with_mutable_agent_storage_quota_without_metering_on_managed_xfs,
};
use pretty_assertions::assert_eq;
use test_r::{test, timeout};

fn schema_u64_list(result: SchemaValue) -> Vec<u64> {
    let SchemaValue::List { elements } = result else {
        panic!("expected list, got {result:?}")
    };
    elements
        .into_iter()
        .map(|element| match element {
            SchemaValue::U64(entry) => entry,
            other => panic!("expected u64, got {other:?}"),
        })
        .collect()
}

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

fn filesystem_available_bytes(path: &std::path::Path) -> anyhow::Result<u64> {
    let capacity = rustix::fs::statvfs(path)?;
    capacity
        .f_bavail
        .checked_mul(capacity.f_frsize)
        .ok_or_else(|| anyhow!("filesystem available capacity exceeds u64"))
}

fn filesystem_fragment_size(path: &std::path::Path) -> anyhow::Result<u64> {
    let fragment_size = rustix::fs::statvfs(path)?.f_frsize;
    if fragment_size == 0 {
        Err(anyhow!("filesystem reported a zero fragment size"))
    } else {
        Ok(fragment_size)
    }
}

fn filesystem_tree_allocated_bytes(path: &std::path::Path) -> anyhow::Result<u64> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path)?;
    let mut allocated_bytes = metadata
        .blocks()
        .checked_mul(512)
        .ok_or_else(|| anyhow!("filesystem tree allocation exceeds u64"))?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            allocated_bytes = allocated_bytes
                .checked_add(filesystem_tree_allocated_bytes(&entry?.path())?)
                .ok_or_else(|| anyhow!("filesystem tree allocation exceeds u64"))?;
        }
    }
    Ok(allocated_bytes)
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn p2_p3_quota_classification_on_managed_xfs(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_key, data_value};

    let root = managed_xfs_test_root();
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota_without_metering_on_managed_xfs(
        deps,
        &context,
        1024 * 1024,
        root,
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;

    let p2_exhaustion_agent = agent_key!("P3FileSystem", "managed-quota-p2-exhaustion");
    let p2_exhaustion = schema_string_list(
        executor
            .invoke_and_await_agent(
                &component,
                &p2_exhaustion_agent,
                "exhaust_p2_quota",
                data_value!(),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected P2 exhaustion result"))?,
    );
    assert_eq!(
        p2_exhaustion.first().map(String::as_str),
        Some("completion=err:quota"),
        "P2 growth must fail specifically because the project quota is exhausted"
    );
    assert_eq!(
        p2_exhaustion.get(1).map(String::as_str),
        Some("prefix-persisted=true")
    );
    let persisted_p2_prefix = schema_string_list(
        executor
            .invoke_and_await_agent(
                &component,
                &p2_exhaustion_agent,
                "inspect_p2_exhaustion",
                data_value!(),
            )
            .await?
            .into_return_value()
            .ok_or_else(|| anyhow!("expected persisted P2 failure prefix"))?,
    );
    let p2_size = persisted_p2_prefix
        .first()
        .and_then(|size| size.strip_prefix("size="))
        .and_then(|size| size.parse::<u64>().ok())
        .expect("P2 exhaustion inspection must report a numeric size");
    assert!((4096..=1024 * 1024).contains(&p2_size));
    assert_eq!(
        &persisted_p2_prefix[1..],
        ["prefix-complete=true", "suffix-bytes=0"]
    );

    let p3_exhaustion_agent = agent_key!("P3FileSystem", "managed-quota-p3-exhaustion");
    let p3_exhaustion = executor
        .invoke_and_await_agent(
            &component,
            &p3_exhaustion_agent,
            "exhaust_p3_quota",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected P3 exhaustion result"))?;
    let p3_exhaustion = schema_string_list(p3_exhaustion);
    assert_eq!(
        p3_exhaustion.first().map(String::as_str),
        Some("completion=err:quota"),
        "P3 growth must fail specifically because the project quota is exhausted"
    );
    let unwritten_bytes = p3_exhaustion
        .get(2)
        .and_then(|value| value.strip_prefix("unwritten-bytes="))
        .ok_or_else(|| anyhow!("P3 stream input result was not reported: {p3_exhaustion:?}"))?
        .parse::<usize>()?;
    assert_eq!(
        p3_exhaustion.get(1).map(String::as_str),
        Some("prefix-persisted=true"),
        "P3 stream must acknowledge data persisted before quota denial"
    );
    assert!(
        unwritten_bytes > 0,
        "P3 quota failure must return the input suffix that was not persisted"
    );
    let persisted_prefix = executor
        .invoke_and_await_agent(
            &component,
            &p3_exhaustion_agent,
            "inspect_p3_exhaustion",
            data_value!(),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected persisted P3 failure prefix"))?;
    let persisted_prefix = schema_string_list(persisted_prefix);
    assert_eq!(
        persisted_prefix.get(1).map(String::as_str),
        Some("prefix-complete=true")
    );
    Ok(())
}

fn fill_filesystem_leaving(path: &std::path::Path, reserve_bytes: u64) -> anyhow::Result<()> {
    use std::io::{Seek, SeekFrom, Write};

    let mut filler = std::fs::File::create(path)?;
    let coarse_block = vec![0x7f; 1024 * 1024];
    let fragment_size = filesystem_fragment_size(path)?;
    let fragment_block = vec![0x7f; usize::try_from(fragment_size)?];
    let mut committed_length = 0u64;
    let mut consecutive_interrupts = 0;

    loop {
        match filler.write(&coarse_block) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into()),
            Ok(written) => {
                consecutive_interrupts = 0;
                let candidate_length = committed_length
                    .checked_add(written as u64)
                    .ok_or_else(|| anyhow!("filesystem filler length exceeds u64"))?;
                match filler.sync_all() {
                    Ok(()) => committed_length = candidate_length,
                    Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                        filler.set_len(committed_length)?;
                        filler.seek(SeekFrom::Start(committed_length))?;
                        filler.sync_all()?;
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                consecutive_interrupts += 1;
                if consecutive_interrupts >= 8 {
                    return Err(error.into());
                }
                continue;
            }
            Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => break,
            Err(error) => return Err(error.into()),
        }
    }

    filler.seek(SeekFrom::Start(committed_length))?;
    let fine_start_available = filesystem_available_bytes(path)?;
    let max_fine_attempts = fine_start_available
        .checked_div(fragment_size)
        .and_then(|fragments| fragments.checked_add(2))
        .ok_or_else(|| anyhow!("filesystem filler refinement bound exceeds u64"))?;
    for _ in 0..max_fine_attempts {
        if filesystem_available_bytes(path)? < fragment_size {
            break;
        }
        match filler.write(&fragment_block) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into()),
            Ok(written) => {
                let candidate_length = committed_length
                    .checked_add(written as u64)
                    .ok_or_else(|| anyhow!("filesystem filler length exceeds u64"))?;
                match filler.sync_all() {
                    Ok(()) => committed_length = candidate_length,
                    Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                        filler.set_len(committed_length)?;
                        filler.seek(SeekFrom::Start(committed_length))?;
                        filler.sync_all()?;
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => break,
            Err(error) => return Err(error.into()),
        }
    }
    if reserve_bytes > 0 {
        let rounded_reserve = reserve_bytes
            .checked_add(fragment_size - 1)
            .and_then(|bytes| bytes.checked_div(fragment_size))
            .and_then(|fragments| fragments.checked_mul(fragment_size))
            .ok_or_else(|| anyhow!("filesystem filler reserve exceeds u64"))?;
        filler.set_len(committed_length.saturating_sub(rounded_reserve))?;
    }
    filler.sync_all()?;
    Ok(())
}

fn create_allocated_file(path: &std::path::Path, length: u64, fill_byte: u8) -> anyhow::Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(path)?;
    let block = vec![fill_byte; 1024 * 1024];
    let mut remaining = length;
    while remaining > 0 {
        let write_size = usize::try_from(remaining.min(block.len() as u64))?;
        file.write_all(&block[..write_size])?;
        remaining -= write_size as u64;
    }
    file.sync_all()?;
    assert_eq!(file.metadata()?.len(), length);
    Ok(())
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn filesystem_downgrade_blocks_guest_until_limit_recovers(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_key, data_value};

    let root = managed_xfs_test_root();
    let context = TestContext::new(last_unique_id);
    let (executor, quota) = start_with_mutable_agent_storage_quota_without_metering_on_managed_xfs(
        deps,
        &context,
        1024 * 1024,
        root.clone(),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .with_files(
            "P3FileSystem",
            &[
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/foo.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/foo-copy.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadOnly,
                },
                IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                },
            ],
        )
        .store()
        .await?;
    let agent = agent_key!("P3FileSystem", "managed-quota-downgrade");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;
    let initial_result = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "confirm_invocation_started",
            data_value!(),
        )
        .await?;
    assert_eq!(
        initial_result.into_return_value(),
        Some(SchemaValue::String("executed".to_string()))
    );
    let runtime_path = root
        .join(context.default_environment_id.to_string())
        .join(format!("{:08x}", worker_id.agent_id.type_tag()))
        .join(agent_path_segment(&worker_id.agent_id));
    assert!(runtime_path.exists());

    quota.set_limit(4096).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Suspended, Duration::from_secs(10))
        .await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        while runtime_path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("over-limit runtime filesystem was not deleted");

    let before_blocked_invocation = executor.oplog_max_index(&worker_id).await?;
    let blocked_key = IdempotencyKey::fresh();
    executor
        .invoke_agent_with_key(
            &component,
            &agent,
            &blocked_key,
            "confirm_invocation_started",
            data_value!(),
        )
        .await?;
    let blocked = tokio::time::timeout(
        Duration::from_secs(1),
        executor.invoke_and_await_agent_with_key(
            &component,
            &agent,
            &blocked_key,
            "confirm_invocation_started",
            data_value!(),
        ),
    )
    .await;
    assert!(
        !matches!(blocked, Ok(Ok(_))),
        "pending invocation completed while reconstruction exceeded its installed quota"
    );
    let blocked_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&blocked_oplog, before_blocked_invocation),
        (0, 0),
        "over-limit reconstruction must not start the pending guest invocation"
    );

    quota.set_limit(1024 * 1024).await?;
    executor.resume(&worker_id, false).await?;
    let recovered = tokio::time::timeout(
        Duration::from_secs(20),
        executor.invoke_and_await_agent_with_key(
            &component,
            &agent,
            &blocked_key,
            "confirm_invocation_started",
            data_value!(),
        ),
    )
    .await
    .expect("pending invocation did not recover after raising the limit")?;
    assert_eq!(
        recovered.into_return_value(),
        Some(SchemaValue::String("executed".to_string()))
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    assert!(runtime_path.exists());
    let recovered_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        count_agent_invocation_pair_since(&recovered_oplog, before_blocked_invocation),
        (1, 1)
    );
    assert_eq!(
        quota.flush_durable_storage_byte_seconds(),
        0,
        "managed XFS quota enforcement must not accumulate storage usage when metering is disabled"
    );
    Ok(())
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn managed_xfs_resource_billing_survives_idle_and_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_common::{agent_key, data_value};

    let context = TestContext::new(last_unique_id);
    let (executor, billing) = start_with_mutable_agent_storage_quota_on_managed_xfs(
        deps,
        &context,
        1024 * 1024,
        managed_xfs_test_root(),
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;
    let agent = agent_key!("P3FileSystem", "managed-xfs-resource-billing");
    let worker_id = executor.start_agent(&component.id, agent.clone()).await?;

    let mutation = executor
        .invoke_and_await_agent(&component, &agent, "run_writable", data_value!())
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected run_writable return value"))?;
    assert_eq!(
        schema_string_list(mutation),
        vec![
            "p2_write_p3_read=p2-to-p3".to_string(),
            "p3_write_p2_read=p3-to-p2".to_string(),
        ]
    );
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;

    let after_mutation = billing.flush_durable_storage_byte_seconds();
    let mut after_active_window = after_mutation;
    for _ in 0..32 {
        assert_reconstructed_writable_file(&executor, &component, &agent).await?;
        executor
            .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
            .await?;
        after_active_window = billing.flush_durable_storage_byte_seconds();
        if after_active_window > after_mutation {
            break;
        }
    }
    assert!(
        after_active_window > after_mutation,
        "authoritative managed-XFS allocation remained zero-billed across active windows"
    );

    let mut idle_start = billing.flush_durable_storage_byte_seconds();
    let mut stable_samples = 0;
    while stable_samples < 3 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let current = billing.flush_durable_storage_byte_seconds();
        if current == idle_start {
            stable_samples += 1;
        } else {
            idle_start = current;
            stable_samples = 0;
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let idle_end = billing.flush_durable_storage_byte_seconds();
    assert_eq!(
        idle_end, idle_start,
        "loaded-idle managed-XFS storage continued billing"
    );

    executor.simulated_crash(&worker_id).await?;
    assert_reconstructed_writable_file(&executor, &component, &agent).await?;
    executor
        .wait_for_status(&worker_id, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    let after_replay = billing.flush_durable_storage_byte_seconds();
    assert!(
        after_replay > idle_end,
        "replay and its following invocation produced no managed-XFS storage billing"
    );

    let before_delete = billing.flush_durable_storage_byte_seconds();
    executor.delete_worker(&worker_id).await?;
    let after_delete = billing.flush_durable_storage_byte_seconds();
    assert!(after_delete >= before_delete);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        billing.flush_durable_storage_byte_seconds(),
        after_delete,
        "deleted managed-XFS storage continued billing"
    );
    Ok(())
}

const PRESSURE_MINIMUM_AVAILABLE_BYTES: u64 = 64 * 1024 * 1024;

const PRESSURE_TARGET_AVAILABLE_BYTES: u64 = 384 * 1024 * 1024;

const PRESSURE_PROJECT_QUOTA_BYTES: u64 = 256 * 1024 * 1024;

const PRESSURE_VICTIM_ALLOCATION_BYTES: u64 = 224 * 1024 * 1024;

const PRESSURE_GATE_BYTES: u64 = 224 * 1024 * 1024;

const PRESSURE_POST_GATE_TARGET_MARGIN_BYTES: u64 = 32 * 1024 * 1024;

const PRESSURE_OBSERVATION_DELAY: Duration = Duration::from_millis(25);

/// The pressure settings of the physical-pressure tests: recovery starts below 64 MiB and stops at
/// 384 MiB of free space, with fresh observations 25 ms apart.
fn physical_pressure_config()
-> golem_worker_executor::services::golem_config::FilesystemPressureConfig {
    let default_pressure =
        golem_worker_executor::services::golem_config::FilesystemPressureConfig::default();
    golem_worker_executor::services::golem_config::FilesystemPressureConfig::new(
        PRESSURE_MINIMUM_AVAILABLE_BYTES,
        PRESSURE_TARGET_AVAILABLE_BYTES,
        default_pressure.minimum_available_filesystem_objects(),
        default_pressure.target_available_filesystem_objects(),
        200,
        PRESSURE_OBSERVATION_DELAY,
    )
    .unwrap()
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn managed_xfs_physical_pressure_unloads_loaded_idle_and_retries_safe_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    const {
        assert!(
            PRESSURE_PROJECT_QUOTA_BYTES - PRESSURE_VICTIM_ALLOCATION_BYTES == 32 * 1024 * 1024,
            "the victim allocation must remain safely below its project quota"
        );
    }
    let root = managed_xfs_test_root();
    let context = TestContext::new(last_unique_id);
    let executor = start_with_agent_storage_quota_and_pressure_without_metering_on_managed_xfs(
        deps,
        &context,
        PRESSURE_PROJECT_QUOTA_BYTES,
        root.clone(),
        physical_pressure_config(),
    )
    .await?;
    physical_pressure_unloads_loaded_idle_and_retries_safe_write(
        executor,
        &context,
        host_api_tests,
        root,
        Some(PRESSURE_PROJECT_QUOTA_BYTES),
    )
    .await
}

#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("2m")]
#[tracing::instrument]
async fn reflink_xfs_physical_pressure_unloads_loaded_idle_and_retries_safe_write(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    use golem_worker_executor_test_utils::start_with_pressure_on_reflink_xfs;

    let root = reflink_xfs_test_root();
    let context = TestContext::new(last_unique_id);
    let executor = start_with_pressure_on_reflink_xfs(
        deps,
        &context,
        root.clone(),
        physical_pressure_config(),
    )
    .await?;
    physical_pressure_unloads_loaded_idle_and_retries_safe_write(
        executor,
        &context,
        host_api_tests,
        root,
        None,
    )
    .await
}

/// Fills the XFS volume at `root` until a write of an agent on `executor` finds no space, and
/// checks that pressure recovery unloads an idle loaded agent and retries the write once the
/// volume reaches its target. With `project_quota`, the agents have that project quota, and the
/// write must fail for lack of space and not for the quota.
async fn physical_pressure_unloads_loaded_idle_and_retries_safe_write(
    executor: TestWorkerExecutor,
    context: &TestContext,
    host_api_tests: &PrecompiledComponent,
    root: PathBuf,
    project_quota: Option<u64>,
) -> anyhow::Result<()> {
    use golem_common::{agent_key, data_value};
    use std::io::{Seek, SeekFrom, Write};

    const {
        assert!(PRESSURE_VICTIM_ALLOCATION_BYTES < PRESSURE_TARGET_AVAILABLE_BYTES);
        assert!(
            PRESSURE_VICTIM_ALLOCATION_BYTES + PRESSURE_GATE_BYTES
                - PRESSURE_TARGET_AVAILABLE_BYTES
                == 64 * 1024 * 1024,
            "victim and gate reclamation must retain margin above the recovery target"
        );
    }
    let allocation_unit = filesystem_fragment_size(&root)?;
    let retry_contents = "r".repeat(usize::try_from(allocation_unit)?);
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let victim = agent_key!("FileSystem", "physical-pressure-victim");
    let victim_worker = executor.start_agent(&component.id, victim.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &victim,
            "write_file_direct",
            data_value!("victim-marker", "loaded-idle"),
        )
        .await?;
    executor
        .wait_for_status(&victim_worker, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    let victim_path = root
        .join(context.default_environment_id.to_string())
        .join(format!("{:08x}", victim_worker.agent_id.type_tag()))
        .join(agent_path_segment(&victim_worker.agent_id));
    assert!(victim_path.is_dir(), "the victim did not use the XFS root");
    create_allocated_file(
        &victim_path.join("pressure-allocation"),
        PRESSURE_VICTIM_ALLOCATION_BYTES,
        0x5a,
    )?;

    let trigger = agent_key!("FileSystem", "physical-pressure-trigger");
    let trigger_worker = executor.start_agent(&component.id, trigger.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &trigger,
            "write_file_direct",
            data_value!("pressure-target", "seed"),
        )
        .await?;
    executor
        .wait_for_status(&trigger_worker, AgentStatus::Idle, Duration::from_secs(10))
        .await?;
    let trigger_path = root
        .join(context.default_environment_id.to_string())
        .join(format!("{:08x}", trigger_worker.agent_id.type_tag()))
        .join(agent_path_segment(&trigger_worker.agent_id));
    assert!(
        trigger_path.is_dir(),
        "the trigger did not use the XFS root"
    );

    let observation_gate = root.join(format!("pressure-target-gate-{}", uuid::Uuid::new_v4()));
    create_allocated_file(&observation_gate, PRESSURE_GATE_BYTES, 0x6b)?;
    let residual_probe_path = trigger_path.join("pressure-residual-probe");
    let mut residual_probe = std::fs::File::create(&residual_probe_path)?;
    assert_eq!(residual_probe.metadata()?.len(), 0);
    let volume_filler = root.join(format!("pressure-volume-filler-{}", uuid::Uuid::new_v4()));
    fill_filesystem_leaving(&volume_filler, 0)?;
    assert_eq!(
        residual_probe.metadata()?.len(),
        0,
        "pressure residual probe changed before its data-allocation proof"
    );
    let probe_start_available = filesystem_available_bytes(&root)?;
    assert!(
        probe_start_available < PRESSURE_MINIMUM_AVAILABLE_BYTES,
        "global filler did not cross the configured physical-pressure watermark: available={probe_start_available}"
    );
    let probe_fragment = vec![0x4d; usize::try_from(allocation_unit)?];
    let max_probe_attempts = probe_start_available
        .checked_div(allocation_unit)
        .and_then(|fragments| fragments.checked_add(8))
        .ok_or_else(|| anyhow!("pressure residual-probe bound exceeds u64"))?;
    let mut probe_committed_bytes = 0u64;
    let mut probe_failure = None;
    let rollback_probe = |probe: &mut std::fs::File, committed_bytes: u64| -> anyhow::Result<()> {
        probe.set_len(committed_bytes)?;
        probe.seek(SeekFrom::Start(committed_bytes))?;
        match probe.sync_all() {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                probe.sync_all()?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    };
    for _ in 0..max_probe_attempts {
        residual_probe.seek(SeekFrom::Start(probe_committed_bytes))?;
        match residual_probe.write_all(&probe_fragment) {
            Ok(()) => match residual_probe.sync_all() {
                Ok(()) => {
                    probe_committed_bytes = probe_committed_bytes
                        .checked_add(allocation_unit)
                        .ok_or_else(|| anyhow!("pressure residual-probe length exceeds u64"))?;
                }
                Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                    rollback_probe(&mut residual_probe, probe_committed_bytes)?;
                    probe_failure = Some(error);
                    break;
                }
                Err(error) => return Err(error.into()),
            },
            Err(error) if error.raw_os_error() == Some(libc::ENOSPC) => {
                rollback_probe(&mut residual_probe, probe_committed_bytes)?;
                probe_failure = Some(error);
                break;
            }
            Err(error) if error.raw_os_error() == Some(libc::EDQUOT) => {
                return Err(anyhow!(
                    "pressure residual probe hit the trigger project quota instead of physical ENOSPC"
                ));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let probe_failure = probe_failure.ok_or_else(|| {
        anyhow!("pressure residual probe never reached physical ENOSPC within its bounded attempts")
    })?;
    assert_eq!(probe_failure.raw_os_error(), Some(libc::ENOSPC));
    if let Some(project_quota) = project_quota {
        let trigger_project_allocated_bytes = filesystem_tree_allocated_bytes(&trigger_path)?;
        assert!(
            trigger_project_allocated_bytes < project_quota / 2,
            "pressure residual probe approached the trigger project quota: allocated={trigger_project_allocated_bytes}, quota={project_quota}"
        );
    }

    let before_invocation = filesystem_available_bytes(&root)?;
    assert!(
        before_invocation < PRESSURE_MINIMUM_AVAILABLE_BYTES,
        "test setup did not cross the configured physical-pressure watermark"
    );
    assert_eq!(
        std::fs::read(trigger_path.join("pressure-target"))?,
        b"seed",
        "pressure target changed before the exhausting invocation"
    );

    let invocation = executor.invoke_and_await_agent(
        &component,
        &trigger,
        "pwrite_file",
        data_value!("/pressure-target", allocation_unit, retry_contents.clone()),
    );
    let observation_barrier = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            while victim_path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| anyhow!("pressure victim was not deleted"))?;
        let before_gate_release = filesystem_available_bytes(&root)?;
        assert!(
            before_gate_release > before_invocation,
            "verified victim deletion did not increase fresh available capacity"
        );
        assert!(
            before_gate_release < PRESSURE_TARGET_AVAILABLE_BYTES,
            "victim deletion unexpectedly reached the configured target before the observation gate was released: available={before_gate_release}"
        );
        let minimum_margin_setup = PRESSURE_TARGET_AVAILABLE_BYTES
            .saturating_sub(PRESSURE_GATE_BYTES)
            .saturating_add(PRESSURE_POST_GATE_TARGET_MARGIN_BYTES);
        assert!(
            before_gate_release >= minimum_margin_setup,
            "victim deletion left insufficient gate-release margin: available={before_gate_release}, required={minimum_margin_setup}"
        );
        // Allow recovery polling while leaving budget for verified unloading and
        // a target-reaching observation within the 250 ms recovery deadline.
        tokio::time::sleep(PRESSURE_OBSERVATION_DELAY * 2).await;
        // Before Linux 6.11, XFS zeroes the EOF block and keeps the grown size when a
        // write past EOF fails with ENOSPC, so the refused write may leave the seed
        // zero-padded to the write offset, one allocation unit. A retry would add the
        // retry contents.
        let before_retry = std::fs::read(trigger_path.join("pressure-target"))?;
        assert!(
            before_retry.starts_with(b"seed")
                && (before_retry.len() == 4 || before_retry.len() as u64 == allocation_unit)
                && before_retry[4..].iter().all(|byte| *byte == 0),
            "safe write retried before a fresh observation could reach the configured target: len={}",
            before_retry.len()
        );

        std::fs::remove_file(&observation_gate)?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let available = filesystem_available_bytes(&root)?;
                if available >= PRESSURE_TARGET_AVAILABLE_BYTES {
                    return Ok::<(u64, u64), anyhow::Error>((before_gate_release, available));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| anyhow!("fresh statvfs observation did not reach the configured target"))?
    };
    let (retried, observed_target) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(invocation, observation_barrier)
    })
    .await
    .expect("physical-pressure recovery did not finish");
    let retried = retried?;
    let (before_gate_release, observed_target) = observed_target?;
    assert!(observed_target >= PRESSURE_TARGET_AVAILABLE_BYTES);
    assert!(
        observed_target > before_gate_release,
        "removing the observation gate did not increase fresh available capacity"
    );
    assert!(matches!(
        retried.into_return_value(),
        Some(SchemaValue::Result(ResultValuePayload::Ok { .. }))
    ));

    assert!(
        !victim_path.exists(),
        "pressure recovery reported success before verified victim deletion"
    );
    assert!(
        trigger_path.is_dir(),
        "pressure recovery unloaded the running trigger"
    );
    let contents = executor
        .invoke_and_await_agent(
            &component,
            &trigger,
            "read_file",
            data_value!("/pressure-target"),
        )
        .await?
        .into_return_value()
        .ok_or_else(|| anyhow!("expected pressure target contents"))?;
    let SchemaValue::Result(ResultValuePayload::Ok { value: Some(value) }) = contents else {
        panic!("expected successful pressure target read, got {contents:?}")
    };
    let SchemaValue::String(contents) = *value else {
        panic!("expected pressure target string contents")
    };
    assert_eq!(contents.len() as u64, allocation_unit * 2);
    assert_eq!(&contents.as_bytes()[..4], b"seed");
    assert!(
        contents.as_bytes()[4..allocation_unit as usize]
            .iter()
            .all(|byte| *byte == 0)
    );
    assert_eq!(
        &contents.as_bytes()[allocation_unit as usize..],
        retry_contents.as_bytes()
    );

    drop(residual_probe);
    std::fs::remove_file(residual_probe_path)?;
    std::fs::remove_file(volume_filler)?;
    executor.delete_worker(&trigger_worker).await?;
    Ok(())
}

#[test]
#[ignore = "run with integration-tests/scripts/managed-filesystem/run-lima.sh --filesystem-benchmark"]
#[timeout("30m")]
async fn filesystem_guest_latency_benchmark(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("initial_file_system")] initial_file_system: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_common::data_value;
    use std::fs;

    fn percentile(sorted: &[u64], percentile: usize) -> u64 {
        sorted[(sorted.len() - 1) * percentile / 100]
    }

    let mode = std::env::var("GOLEM_FILESYSTEM_BENCH_MODE").map_err(|_| {
        anyhow!("GOLEM_FILESYSTEM_BENCH_MODE must be managed, managed-unmetered, or unmanaged")
    })?;
    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS root"))?;
    let context = TestContext::new(last_unique_id);
    let executor = match mode.as_str() {
        "managed" => {
            start_with_agent_storage_quota_on_managed_xfs(deps, &context, 128 * 1024 * 1024, root)
                .await?
        }
        "managed-unmetered" => {
            start_with_agent_storage_quota_without_metering_on_managed_xfs(
                deps,
                &context,
                128 * 1024 * 1024,
                root,
            )
            .await?
        }
        "unmanaged" => {
            let unmanaged_root = root.join("unmanaged-benchmark");
            fs::create_dir_all(&unmanaged_root)?;
            start_with_overrides(
                deps,
                &context,
                TestExecutorOverrides {
                    configure: Some(Arc::new(move |config| {
                        config.filesystem_storage.mode = FilesystemStorageMode::Directory {
                            root: unmanaged_root.clone().into(),
                        };
                        config.resource_usage_metering = Default::default();
                        config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
                        config.oplog.oplog_processor_snapshotting = SnapshotPolicy::Disabled;
                    })),
                    ..TestExecutorOverrides::default()
                },
            )
            .await?
        }
        other => return Err(anyhow!("unsupported filesystem benchmark mode: {other}")),
    };
    let component = executor
        .component_dep(&context.default_environment_id, initial_file_system)
        .store()
        .await?;

    let quick = std::env::var("GOLEM_FILESYSTEM_BENCH_QUICK").is_ok_and(|value| value == "1");
    let concurrencies: &[usize] = if quick { &[1] } else { &[1, 4, 16, 64] };
    let payloads: &[u32] = if quick {
        &[4 * 1024]
    } else {
        &[4 * 1024, 64 * 1024, 1024 * 1024]
    };
    let max_concurrency = *concurrencies.last().unwrap();
    let agent_ids = (0..max_concurrency)
        .map(|index| {
            agent_key!(
                "P3FileSystem",
                format!("filesystem-benchmark-{mode}-{index}")
            )
        })
        .collect::<Vec<_>>();
    let starts = agent_ids
        .iter()
        .map(|agent_id| executor.start_agent(&component.id, agent_id.clone()));
    for started in futures::future::join_all(starts).await {
        started?;
    }

    let operations = [
        "p2-read",
        "p2-write",
        "p2-read-stream",
        "p2-write-stream",
        "p3-read-stream",
        "p3-write-stream",
    ];
    for &concurrency in concurrencies {
        let clock_samples: u32 = if quick { 8 } else { 128 };
        println!(
            "FILESYSTEM_BENCHMARK_PHASE {}",
            serde_json::json!({
                "state": "start",
                "mode": mode,
                "operation": "clock",
                "payload_bytes": 0,
                "concurrency": concurrency,
            })
        );
        let clock_calls = agent_ids[..concurrency].iter().map(|agent_id| {
            executor.invoke_and_await_agent(
                &component,
                agent_id,
                "benchmark_filesystem",
                data_value!("clock".to_string(), 0u32, clock_samples, 1u32),
            )
        });
        let mut clock_durations = Vec::new();
        for result in futures::future::join_all(clock_calls).await {
            let value = result?
                .into_return_value()
                .ok_or_else(|| anyhow!("clock benchmark did not return samples"))?;
            clock_durations.extend(schema_u64_list(value));
        }
        clock_durations.sort_unstable();
        println!(
            "FILESYSTEM_BENCHMARK_PHASE {}",
            serde_json::json!({
                "state": "end",
                "mode": mode,
                "operation": "clock",
                "payload_bytes": 0,
                "concurrency": concurrency,
            })
        );
        println!(
            "FILESYSTEM_BENCHMARK {}",
            serde_json::json!({
                "mode": mode,
                "operation": "clock",
                "payload_bytes": 0,
                "concurrency": concurrency,
                "batch_size": 1,
                "sample_count": clock_durations.len(),
                "p50_ns": percentile(&clock_durations, 50),
                "p95_ns": percentile(&clock_durations, 95),
                "p99_ns": percentile(&clock_durations, 99),
            })
        );

        for &payload_size in payloads {
            let samples: u32 = if quick {
                8
            } else if payload_size <= 4 * 1024 {
                128
            } else if payload_size <= 64 * 1024 {
                64
            } else {
                16
            };
            for operation in operations {
                let batch_size: u32 = match (operation, payload_size) {
                    ("p2-read" | "p2-write" | "p2-read-stream" | "p2-write-stream", size)
                        if size <= 4 * 1024 =>
                    {
                        4
                    }
                    ("p2-read" | "p2-write" | "p2-read-stream" | "p2-write-stream", size)
                        if size <= 64 * 1024 =>
                    {
                        2
                    }
                    _ => 1,
                };
                println!(
                    "FILESYSTEM_BENCHMARK_PHASE {}",
                    serde_json::json!({
                        "state": "start",
                        "mode": mode,
                        "operation": operation,
                        "payload_bytes": payload_size,
                        "concurrency": concurrency,
                    })
                );
                let calls = agent_ids[..concurrency].iter().map(|agent_id| {
                    executor.invoke_and_await_agent(
                        &component,
                        agent_id,
                        "benchmark_filesystem",
                        data_value!(operation.to_string(), payload_size, samples, batch_size),
                    )
                });
                let mut durations = Vec::new();
                for result in futures::future::join_all(calls).await {
                    let value = result?
                        .into_return_value()
                        .ok_or_else(|| anyhow!("filesystem benchmark did not return samples"))?;
                    durations.extend(schema_u64_list(value));
                }
                durations.sort_unstable();
                println!(
                    "FILESYSTEM_BENCHMARK_PHASE {}",
                    serde_json::json!({
                        "state": "end",
                        "mode": mode,
                        "operation": operation,
                        "payload_bytes": payload_size,
                        "concurrency": concurrency,
                    })
                );
                let total = durations
                    .iter()
                    .fold(0u128, |total, duration| total + u128::from(*duration));
                println!(
                    "FILESYSTEM_BENCHMARK {}",
                    serde_json::json!({
                        "mode": mode,
                        "operation": operation,
                        "payload_bytes": payload_size,
                        "concurrency": concurrency,
                        "batch_size": batch_size,
                        "sample_count": durations.len(),
                        "mean_ns": total / durations.len() as u128,
                        "p50_ns": percentile(&durations, 50),
                        "p95_ns": percentile(&durations, 95),
                        "p99_ns": percentile(&durations, 99),
                    })
                );
            }
        }
    }

    Ok(())
}
