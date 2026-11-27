use super::snapshot_attachments::DisposalScheduleComponentService;
use super::*;
use golem_common::model::card::parse_polymorphic_permission;
use golem_common::model::oplog::{OplogEntry, OplogPayload};
use golem_service_base::storage::blob::{BlobStorageNamespace, NormalizedBlobPath};
use golem_test_framework::components::rdb::docker_postgres::DockerPostgresRdb;
use golem_worker_executor_test_utils::{
    BlobStorageDecorator, BlobStorageInner, WrappedBlobStorage,
};
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
        wrap_blob_storage: Some(terminal_read.clone().wrap_blob_storage()),
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
        .update_agent_provision_config("ToolStreamingCaller", |config| {
            // The tool uses its caller's wallet for the trusted nextval sequence probe.
            config.initial_permissions.lower_bound.positive.push(
                parse_polymorphic_permission("rdbms(?env) @ * : unanalyzed : database(postgres)")
                    .unwrap(),
            );
        })
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
        Some(
            rust_provider_deployment(context.account_id, &provider_component, metadata.tools)
                .provider_name("golem-it:tool-streaming-rust-provider")
                .bound_to("ToolStreamingCaller")
                .build(),
        ),
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
    let agent = agent_key!("ToolStreamingCaller", "rdbms-drop-trap");
    let worker = executor.start_agent(&component.id, agent.clone()).await?;
    let mut disposed = executor.probe_entity_store_disposal(&worker.agent_id);
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
    let before = executor.stored_oplog(&worker.agent_id).await;
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
    let mut disposed = executor.probe_entity_store_disposal(&worker.agent_id);
    let invocation =
        executor.invoke_and_await_agent(&component, &agent, "replay_probe", data_value!());
    if fail_terminal_read {
        let release = async {
            tokio::time::timeout(Duration::from_secs(30), terminal_read.entered.notified()).await?;
            let settled = executor.probe_entity_settlement_access(&worker.agent_id, entity);
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
    let after = executor.stored_oplog(&worker.agent_id).await;
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

#[async_trait::async_trait]
impl BlobStorageDecorator for TerminalReadGate {
    async fn get_raw_at(
        &self,
        inner: &WrappedBlobStorage,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let terminal = matches!(&namespace, BlobStorageNamespace::OplogPayload { .. })
            && self
                .path
                .get()
                .is_some_and(|terminal| terminal.as_path() == path.as_ref());
        let bytes = inner
            .get_raw_at(target_label, op_label, namespace, path)
            .await?;
        if terminal && self.armed.load(Ordering::SeqCst) {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                // Install the settlement observer after the Store exists, before the guest
                // receives its reconstructed transaction handle.
                self.entered.notify_one();
                self.reconstruct.notified().await;
            } else {
                // Guest settlement stops driving accessor tasks. The retained context must
                // finish this read; dropping the pending table would silently lose its error.
                self.release.notified().await;
                self.failed.store(true, Ordering::SeqCst);
                anyhow::bail!(TERMINAL_READ_FAILURE);
            }
        }
        Ok(bytes)
    }
}
