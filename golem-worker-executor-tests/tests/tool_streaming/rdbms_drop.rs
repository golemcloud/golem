use super::snapshot_attachments::DisposalScheduleComponentService;
use super::*;
use golem_common::model::oplog::{OplogEntry, OplogPayload};
use golem_service_base::storage::blob::{
    BlobMetadata, BlobMissingError, BlobRangeStream, BlobStorage, BlobStorageBackend,
    BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath, PutIfAbsent,
};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use std::path::PathBuf;
use test_r::{test, test_dep};

#[test_dep(scope = Shared)]
async fn postgres_drop() -> DockerPostgresRdb {
    DockerPostgresRdb::new(&uuid::Uuid::new_v4().to_string(), false).await
}

#[test]
#[timeout("3m")]
async fn trapped_tool_transaction_drop_survives_replay_store_disposal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres_drop: &DockerPostgresRdb,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    transaction_drop_disposal(last_unique_id, deps, postgres_drop, provider, caller, false).await
}

#[test]
#[timeout("3m")]
async fn trapped_tool_transaction_drop_retains_terminal_read_failure_after_disposal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres_drop: &DockerPostgresRdb,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    transaction_drop_disposal(last_unique_id, deps, postgres_drop, provider, caller, true).await
}

async fn transaction_drop_disposal(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    postgres_drop: &DockerPostgresRdb,
    provider: &PrecompiledComponent,
    caller: &PrecompiledComponent,
    fail_terminal_read: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let schedule = Arc::new(AtomicUsize::new(0));
    let selected_schedule = schedule.clone();
    let terminal_read = Arc::new(TerminalReadGate::default());
    let storage_gate = terminal_read.clone();
    let bytes = Arc::new(
        tokio::fs::read(
            deps.component_directory
                .join(format!("{}.wasm", provider.wasm_name)),
        )
        .await?,
    );
    let overrides = TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        wrap_component_service: Some(Arc::new(move |inner| {
            Arc::new(DisposalScheduleComponentService {
                inner,
                bytes: bytes.clone(),
                schedule: selected_schedule.clone(),
            })
        })),
        wrap_blob_storage: Some(Arc::new(move |inner| {
            Arc::new(TerminalReadStorage {
                inner,
                gate: storage_gate.clone(),
            })
        })),
        configure: Some(Arc::new(|config| {
            config.oplog.max_payload_size = 0;
            config.retry.max_attempts = 0;
            config.oplog.default_snapshotting =
                golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled;
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .name("gc-disposal-provider")
        .store()
        .await?;
    let component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", provider.wasm_name)),
        false,
        true,
    )
    .await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        component.id,
        component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );
    let address = postgres_drop.public_connection_string();
    let pool = sqlx::PgPool::connect(&address).await?;
    let table = format!("trapped_drop_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SEQUENCE {table}_effects"))
        .execute(&pool)
        .await?;
    sqlx::query(&format!("CREATE TABLE {table} (id BIGINT)"))
        .execute(&pool)
        .await?;
    let agent = agent_id!("ToolStreamingCaller", "rdbms-drop-trap");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let mut disposed = executor.probe_entity_store_disposal(&worker);
    let evidence: StreamEvidence = executor
        .invoke_and_await_agent(
            &component,
            &agent,
            "collect",
            data_value!(
                format!("rdbms-drop-trap:{address}|{table}"),
                Vec::<u8>::new(),
                1u32
            ),
        )
        .await?
        .into_typed()?;
    assert!(
        evidence.completion.contains("Failed") || evidence.completion.contains("RemoteInternal"),
        "{evidence:?}"
    );
    let entity = tokio::time::timeout(Duration::from_secs(30), disposed.recv())
        .await?
        .expect("live trapped Store disposed");
    let public = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let trapped = public
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::End(end) if end.start_index == entity => end.response.as_ref(),
            _ => None,
        })
        .expect("isolated tool terminal");
    let trapped = SerializableToolOperationTerminal::from_value(trapped.value())?;
    assert_eq!(
        trapped.body_execution,
        SerializableEntityBodyExecution::Trapped
    );
    let transaction = public
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start) if start.function_name == "<scope:transaction>" => {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("transaction scope");
    let before = executor.stored_oplog(&worker).await;
    let terminal_path = before
        .iter()
        .find_map(|entry| match entry {
            OplogEntry::End {
                start_index,
                response:
                    Some(OplogPayload::External {
                        payload_id,
                        md5_hash,
                        ..
                    }),
                ..
            } if *start_index == transaction => Some(PathBuf::from(format!(
                "{}/{}",
                hex::encode(md5_hash),
                payload_id.0
            ))),
            _ => None,
        })
        .expect("transaction terminal uses external storage");
    terminal_read.path.set(terminal_path).unwrap();
    let terminals = |history: &[OplogEntry]| {
        history.iter().filter(|entry| matches!(entry, OplogEntry::End { start_index, .. } if *start_index == transaction || *start_index == entity)).cloned().collect::<Vec<_>>()
    };
    assert_eq!(
        terminals(&before).len(),
        2,
        "transaction and isolated trap each settle once"
    );
    let rows: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(&pool)
        .await?;
    assert_eq!(rows.0, 0);
    let effects: (i64, bool) = sqlx::query_as(&format!(
        "SELECT last_value, is_called FROM {table}_effects"
    ))
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        effects,
        (1, true),
        "Postgres sequences count even rolled-back inserts"
    );
    drop(executor);
    schedule.store(1, Ordering::SeqCst);
    terminal_read
        .armed
        .store(fail_terminal_read, Ordering::SeqCst);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let mut disposed = executor.probe_entity_store_disposal(&worker);
    let invocation =
        executor.invoke_and_await_agent(&component, &agent, "replay_probe", data_value!());
    if fail_terminal_read {
        let release = async {
            tokio::time::timeout(Duration::from_secs(30), terminal_read.entered.notified()).await?;
            let settled = executor.probe_entity_settlement_access(&worker, entity);
            terminal_read.reconstruct.notify_one();
            tokio::time::timeout(Duration::from_secs(30), settled).await??;
            terminal_read.release.notify_one();
            Ok::<_, anyhow::Error>(())
        };
        let (result, released) = tokio::join!(invocation, release);
        released?;
        assert!(
            terminal_read.failed.load(Ordering::SeqCst),
            "retained cleanup must finish the storage read after Store disposal"
        );
        let error = result.expect_err("retained cleanup must propagate the terminal read failure");
        assert!(
            format!("{error:#}").contains("payload backend failure"),
            "{error:#}"
        );
        terminal_read.armed.store(false, Ordering::SeqCst);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(30), disposed.recv()).await?,
            Some(entity)
        );
    } else {
        let result: String = invocation.await?.into_typed()?;
        assert_eq!(result, "replayed");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(30), disposed.recv()).await?,
            Some(entity)
        );
    }
    let after = executor.stored_oplog(&worker).await;
    assert_eq!(&after[..before.len()], before.as_slice());
    if fail_terminal_read {
        assert!(
            after[before.len()..].iter().any(|entry| matches!(
                entry,
                OplogEntry::Error { retry_from, .. } if *retry_from == transaction
            )),
            "retained cleanup failure belongs to the original transaction Start"
        );
    }
    assert_eq!(
        terminals(&after),
        terminals(&before),
        "retain the exact original transaction and trap terminals"
    );
    let effects: (i64, bool) = sqlx::query_as(&format!(
        "SELECT last_value, is_called FROM {table}_effects"
    ))
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        effects,
        (1, true),
        "completed replay must not execute the insert again"
    );
    let rows: (i64,) = sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(&pool)
        .await?;
    assert_eq!(rows.0, 0);
    executor.delete_worker(&worker).await?;
    Ok(())
}

const TERMINAL_READ_FAILURE: &str = "transaction terminal storage unavailable after Store disposal";

#[derive(Debug, Default)]
struct TerminalReadGate {
    path: std::sync::OnceLock<PathBuf>,
    armed: std::sync::atomic::AtomicBool,
    failed: std::sync::atomic::AtomicBool,
    reads: AtomicUsize,
    entered: tokio::sync::Notify,
    reconstruct: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[derive(Debug)]
struct TerminalReadStorage {
    inner: Arc<dyn BlobStorage>,
    gate: Arc<TerminalReadGate>,
}

#[async_trait::async_trait]
impl BlobStorageBackend for TerminalReadStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let terminal = matches!(&namespace, BlobStorageNamespace::OplogPayload { .. })
            && self
                .gate
                .path
                .get()
                .is_some_and(|terminal| terminal.as_path() == path.as_ref());
        let bytes = self
            .inner
            .get_raw(target_label, op_label, namespace, path)
            .await?;
        if terminal && self.gate.armed.load(Ordering::SeqCst) {
            if self.gate.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                // Install the settlement observer after the Store exists, before the guest
                // receives its reconstructed transaction handle.
                self.gate.entered.notify_one();
                self.gate.reconstruct.notified().await;
            } else {
                // Guest settlement stops driving accessor tasks. The retained context must
                // finish this read; dropping the pending table would silently lose its error.
                self.gate.release.notified().await;
                self.gate.failed.store(true, Ordering::SeqCst);
                anyhow::bail!(TERMINAL_READ_FAILURE);
            }
        }
        Ok(bytes)
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> anyhow::Result<Option<BlobRangeStream>> {
        self.inner
            .get_range_stream(target_label, op_label, namespace, path, offset, length)
            .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.inner
            .get_metadata(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<()> {
        self.inner
            .put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        self.inner
            .put_raw_if_absent(target_label, op_label, namespace, path, data)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .delete(target_label, op_label, namespace, path)
            .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<()> {
        self.inner
            .create_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.inner
            .list_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.inner
            .list_blobs_below(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<u64>> {
        self.inner
            .delete_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<ExistsResult> {
        self.inner
            .exists(target_label, op_label, namespace, path)
            .await
    }

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<bool> {
        match self
            .inner
            .copy_between(
                target_label,
                op_label,
                from_namespace,
                from,
                to_namespace,
                to,
            )
            .await
        {
            Ok(()) => Ok(true),
            Err(error) if error.is::<BlobMissingError>() => Ok(false),
            Err(error) => Err(error),
        }
    }
}
