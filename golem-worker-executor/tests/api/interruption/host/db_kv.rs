use super::*;
use anyhow::{Context as _, ensure};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::{AgentError, OplogEntry};
use golem_common::model::{RdbmsPoolKey, TransactionId};
use golem_service_base::error::worker_executor::InterruptKind;
use golem_worker_executor::services::key_value::KeyValueService;
use golem_worker_executor::services::rdbms::ignite::IgniteType;
use golem_worker_executor::services::rdbms::mysql::MysqlType;
use golem_worker_executor::services::rdbms::postgres::{PostgresType, types::DbValue};
use golem_worker_executor::services::rdbms::{
    DbResult, DbResultStream, DbTransaction, Rdbms, RdbmsError, RdbmsService, RdbmsStatus,
    RdbmsTransactionStatus,
};
use golem_worker_executor::services::{HasOplog, UsesAllDeps};
use golem_worker_executor::worker::MonthlyClockForTest;
use golem_worker_executor_test_utils::start_with_resource_limits_and_overrides;
use test_r::test;
use tokio::sync::Notify;

const ADDRESS: &str = "postgres://completion:completion@localhost/completion";
const IDLE: usize = 0;
const PENDING: usize = 1;
const RETURNED: usize = 2;
const DROPPED: usize = 3;

struct ProviderGate {
    started: AtomicBool,
    state: AtomicUsize,
    pending: Notify,
    release: Semaphore,
}

impl Default for ProviderGate {
    fn default() -> Self {
        Self {
            started: AtomicBool::new(false),
            state: AtomicUsize::new(IDLE),
            pending: Notify::new(),
            release: Semaphore::new(0),
        }
    }
}

struct PendingProvider<'a> {
    gate: &'a ProviderGate,
    returned: bool,
}

impl Drop for PendingProvider<'_> {
    fn drop(&mut self) {
        if !self.returned {
            self.gate.state.store(DROPPED, Ordering::SeqCst);
        }
    }
}

impl ProviderGate {
    async fn run<F: Future>(&self, work: F) -> F::Output {
        if self.started.swap(true, Ordering::SeqCst) {
            return work.await;
        }
        let mut guard = PendingProvider {
            gate: self,
            returned: false,
        };
        let mut acquire = std::pin::pin!(self.release.acquire());
        futures::future::poll_fn(|cx| {
            let result = acquire.as_mut().poll(cx);
            if result.is_pending()
                && self
                    .state
                    .compare_exchange(IDLE, PENDING, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                self.pending.notify_one();
            }
            result
        })
        .await
        .unwrap()
        .forget();
        let result = work.await;
        guard.returned = true;
        self.state.store(RETURNED, Ordering::SeqCst);
        result
    }

    async fn wait_pending(&self) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(10), self.pending.notified())
            .await
            .context("provider did not report its first Poll::Pending")?;
        ensure!(self.state.load(Ordering::SeqCst) == PENDING);
        Ok(())
    }
}

#[derive(Default)]
struct ProviderState {
    call: ProviderGate,
    cleanup: ProviderGate,
    effects: Mutex<Vec<String>>,
    starts: Mutex<Vec<String>>,
    transaction_status: Mutex<Option<RdbmsTransactionStatus>>,
}

impl ProviderState {
    fn effect(&self, name: &str) {
        self.effects.lock().unwrap().push(name.to_string());
    }
    fn start(&self, name: &str) {
        self.starts.lock().unwrap().push(name.to_string());
    }
    fn effects(&self) -> Vec<String> {
        self.effects.lock().unwrap().clone()
    }
    fn starts(&self) -> Vec<String> {
        self.starts.lock().unwrap().clone()
    }
    fn release_all(&self) {
        self.call.release.add_permits(10);
        self.cleanup.release.add_permits(10);
    }
}

struct ReleaseProviders(Arc<ProviderState>);

impl Drop for ReleaseProviders {
    fn drop(&mut self) {
        self.0.release_all();
    }
}

struct GatedKv {
    inner: Arc<dyn KeyValueService>,
    state: Arc<ProviderState>,
}

#[async_trait]
impl KeyValueService for GatedKv {
    async fn set(
        &self,
        env: EnvironmentId,
        bucket: String,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<()> {
        self.state.start(&key);
        let work = async {
            self.inner.set(env, bucket, key.clone(), value).await?;
            self.state.effect(&key);
            Ok(())
        };
        if key == "first" {
            self.state.call.run(work).await
        } else {
            work.await
        }
    }
    async fn get(
        &self,
        env: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.inner.get(env, bucket, key).await
    }
    async fn delete(&self, env: EnvironmentId, bucket: String, key: String) -> anyhow::Result<()> {
        self.inner.delete(env, bucket, key).await
    }
    async fn delete_many(
        &self,
        env: EnvironmentId,
        bucket: String,
        keys: Arc<[String]>,
    ) -> anyhow::Result<()> {
        self.inner.delete_many(env, bucket, keys).await
    }
    async fn exists(
        &self,
        env: EnvironmentId,
        bucket: String,
        key: String,
    ) -> anyhow::Result<bool> {
        self.inner.exists(env, bucket, key).await
    }
    async fn get_keys(&self, env: EnvironmentId, bucket: String) -> anyhow::Result<Vec<String>> {
        self.inner.get_keys(env, bucket).await
    }
    async fn get_many(
        &self,
        env: EnvironmentId,
        bucket: String,
        keys: Arc<[String]>,
    ) -> anyhow::Result<Vec<Option<Vec<u8>>>> {
        self.inner.get_many(env, bucket, keys).await
    }
    async fn set_many(
        &self,
        env: EnvironmentId,
        bucket: String,
        values: Arc<[(String, Vec<u8>)]>,
    ) -> anyhow::Result<()> {
        self.inner.set_many(env, bucket, values).await
    }
}

struct ScriptedService {
    inner: Arc<dyn RdbmsService>,
    postgres: Arc<ScriptedPostgres>,
}

impl RdbmsService for ScriptedService {
    fn postgres(&self) -> Arc<dyn Rdbms<PostgresType>> {
        self.postgres.clone()
    }
    fn mysql(&self) -> Arc<dyn Rdbms<MysqlType>> {
        self.inner.mysql()
    }
    fn ignite(&self) -> Arc<dyn Rdbms<IgniteType>> {
        self.inner.ignite()
    }
}

struct ScriptedPostgres {
    state: Arc<ProviderState>,
}

fn unexpected(operation: &str) -> RdbmsError {
    RdbmsError::Other(format!(
        "unexpected scripted provider operation: {operation}"
    ))
}

fn statement_name(statement: &str) -> Result<&'static str, RdbmsError> {
    match statement {
        "INSERT INTO completion_effects (label) VALUES ('first')" => Ok("first"),
        "INSERT INTO completion_effects (label) VALUES ('second')" => Ok("second"),
        "INSERT INTO completion_effects (label) VALUES ('tx-statement')" => Ok("tx-statement"),
        _ => Err(unexpected("statement")),
    }
}

#[async_trait]
impl Rdbms<PostgresType> for ScriptedPostgres {
    async fn create(&self, address: &str, _agent: &AgentId) -> Result<RdbmsPoolKey, RdbmsError> {
        RdbmsPoolKey::from(address).map_err(RdbmsError::ConnectionFailure)
    }
    async fn exists(&self, _key: &RdbmsPoolKey, _agent: &AgentId) -> bool {
        true
    }
    async fn remove(&self, _key: &RdbmsPoolKey, _agent: &AgentId) -> bool {
        true
    }
    async fn execute(
        &self,
        _key: &RdbmsPoolKey,
        _agent: &AgentId,
        statement: &str,
        _params: Vec<DbValue>,
    ) -> Result<u64, RdbmsError> {
        let name = statement_name(statement)?;
        self.state.start(name);
        let work = async {
            self.state.effect(name);
            Ok(1)
        };
        if name == "first" {
            self.state.call.run(work).await
        } else {
            work.await
        }
    }
    async fn query(
        &self,
        _key: &RdbmsPoolKey,
        _agent: &AgentId,
        _statement: &str,
        _params: Vec<DbValue>,
    ) -> Result<DbResult<PostgresType>, RdbmsError> {
        Err(unexpected("query"))
    }
    async fn query_stream(
        &self,
        _key: &RdbmsPoolKey,
        _agent: &AgentId,
        _statement: &str,
        _params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultStream<PostgresType> + Send + Sync>, RdbmsError> {
        Err(unexpected("query-stream"))
    }
    async fn begin_transaction(
        &self,
        _key: &RdbmsPoolKey,
        _agent: &AgentId,
    ) -> Result<Arc<dyn DbTransaction<PostgresType> + Send + Sync>, RdbmsError> {
        *self.state.transaction_status.lock().unwrap() = Some(RdbmsTransactionStatus::InProgress);
        Ok(Arc::new(ScriptedTransaction {
            state: self.state.clone(),
        }))
    }
    async fn get_transaction_status(
        &self,
        _key: &RdbmsPoolKey,
        _agent: &AgentId,
        _tx: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, RdbmsError> {
        Ok(self
            .state
            .transaction_status
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(RdbmsTransactionStatus::NotFound))
    }
    async fn cleanup_transaction(
        &self,
        _key: &RdbmsPoolKey,
        _agent: &AgentId,
        _tx: &TransactionId,
    ) -> Result<(), RdbmsError> {
        self.state.start("cleanup");
        self.state
            .cleanup
            .run(async {
                self.state.effect("cleanup");
                Ok(())
            })
            .await
    }
    async fn status(&self) -> RdbmsStatus {
        RdbmsStatus {
            pools: HashMap::new(),
        }
    }
}

struct ScriptedTransaction {
    state: Arc<ProviderState>,
}

#[async_trait]
impl DbTransaction<PostgresType> for ScriptedTransaction {
    fn transaction_id(&self) -> TransactionId {
        TransactionId::new("completion-transaction")
    }
    async fn execute(&self, statement: &str, _params: Vec<DbValue>) -> Result<u64, RdbmsError> {
        let name = statement_name(statement)?;
        self.state.start(name);
        self.state.effect(name);
        Ok(1)
    }
    async fn query(
        &self,
        _statement: &str,
        _params: Vec<DbValue>,
    ) -> Result<DbResult<PostgresType>, RdbmsError> {
        Err(unexpected("transaction-query"))
    }
    async fn query_stream(
        &self,
        _statement: &str,
        _params: Vec<DbValue>,
    ) -> Result<Arc<dyn DbResultStream<PostgresType> + Send + Sync>, RdbmsError> {
        Err(unexpected("transaction-query-stream"))
    }
    async fn pre_commit(&self) -> Result<(), RdbmsError> {
        self.state.effect("pre-commit");
        Ok(())
    }
    async fn pre_rollback(&self) -> Result<(), RdbmsError> {
        Err(unexpected("pre-rollback"))
    }
    async fn commit(&self) -> Result<(), RdbmsError> {
        self.state.start("commit");
        self.state
            .call
            .run(async {
                self.state.effect("commit");
                *self.state.transaction_status.lock().unwrap() =
                    Some(RdbmsTransactionStatus::Committed);
                Ok(())
            })
            .await
    }
    async fn rollback(&self) -> Result<(), RdbmsError> {
        Err(unexpected("rollback"))
    }
    async fn rollback_if_open(&self) -> Result<(), RdbmsError> {
        Err(unexpected("rollback-if-open"))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Operation {
    Keyvalue,
    Execute,
    Commit,
}

macro_rules! case {
    ($name:ident, $operation:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] fixture: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            completion_stop(
                last_unique_id,
                deps,
                fixture,
                Operation::$operation,
                AgentMode::$mode,
            )
            .await
        }
    };
}
case!(
    durable_keyvalue_completion_then_quota_stop,
    Keyvalue,
    Durable
);
case!(
    ephemeral_keyvalue_completion_then_quota_stop,
    Keyvalue,
    Ephemeral
);
case!(durable_rdbms_completion_then_quota_stop, Execute, Durable);
case!(
    ephemeral_rdbms_completion_then_quota_stop,
    Execute,
    Ephemeral
);
case!(
    durable_entered_commit_finishes_cleanup_before_quota_stop,
    Commit,
    Durable
);

fn terminals(
    entries: &[golem_common::model::oplog::PublicOplogEntryWithIndex],
    start: OplogIndex,
) -> usize {
    entries
        .iter()
        .filter(|e| match &e.entry {
            PublicOplogEntry::End(end) => end.start_index == start,
            PublicOplogEntry::Cancelled(cancelled) => cancelled.start_index == start,
            _ => false,
        })
        .count()
}

async fn completion_stop(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    fixture: &PrecompiledComponent,
    operation: Operation,
    mode: AgentMode,
) -> anyhow::Result<()> {
    let policy = support::Resource::Memory.policy();
    let metering = support::Resource::Memory.metering();
    let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
    let shutdown = CancellationToken::new();
    let _shutdown = shutdown.clone().drop_guard();
    let limits = ResourceLimitsGrpc::new(
        registry.clone(),
        Duration::from_secs(3600),
        Duration::ZERO,
        metering,
        shutdown,
    );
    let state = Arc::new(ProviderState::default());
    let release_providers = ReleaseProviders(state.clone());
    let context = TestContext::new(last_unique_id);
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(move |config| {
            config.resource_usage_metering = metering
        })),
        wrap_key_value_service: Some(Arc::new({
            let state = state.clone();
            move |inner| {
                Arc::new(GatedKv {
                    inner,
                    state: state.clone(),
                })
            }
        })),
        wrap_rdbms_service: Some(Arc::new({
            let state = state.clone();
            move |inner| {
                Arc::new(ScriptedService {
                    inner,
                    postgres: Arc::new(ScriptedPostgres {
                        state: state.clone(),
                    }),
                })
            }
        })),
        ..Default::default()
    };
    let executor =
        start_with_resource_limits_and_overrides(deps, &context, limits.clone(), overrides).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let seed_id = executor
        .start_agent(
            &component.id,
            agent_id!("DbKvCompletion", "completion-seed"),
        )
        .await?;
    let seed = executor
        .production_active_agent(&OwnedAgentId::new(context.default_environment_id, &seed_id))
        .await
        .unwrap()
        .primary();
    wait_for_invocation_pair(&executor, &seed_id, OplogIndex::INITIAL).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while seed.concurrent_agent_permit_is_held().await {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    limits.run_batch_for_test().await;

    let durable = mode == AgentMode::Durable;
    let name = if durable {
        agent_id!("DbKvCompletion", "completion-target")
    } else {
        agent_id!("EphemeralDbKvCompletion", "completion-target")
    };
    let key = IdempotencyKey::fresh();
    let physical = if durable {
        name.clone()
    } else {
        name.clone()
            .with_ephemeral_invocation_phantom(&key)
            .unwrap()
    };
    let id = AgentId::from_agent_id(component.id, &physical).unwrap();
    let worker = Worker::get_or_create_suspended(
        seed.all(),
        &OwnedAgentId::new(context.default_environment_id, &id),
        None,
        vec![],
        None,
        None,
        &InvocationContextStack::fresh(),
        Principal::anonymous(),
    )
    .await?;
    let (clock, mut polls) = MonthlyClockForTest::new();
    worker.set_monthly_clock_for_test(clock.clone());
    let mut attempts = worker.observe_monthly_acceptance_for_test();
    let method = match operation {
        Operation::Keyvalue => "set_twice",
        Operation::Execute => "execute_twice",
        Operation::Commit => "commit",
    };
    let input = if operation == Operation::Keyvalue {
        data_value!(format!("{}-completion", component.id))
    } else {
        data_value!(ADDRESS)
    };
    let mut invocation = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let executor = executor.clone();
        let component = component.clone();
        let name = name.clone();
        let key = key.clone();
        let input = input.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(&component, &name, &key, method, input)
                .await
        }
    }));
    let mut refreshes = Vec::new();
    let result = async {
        state.call.wait_pending().await?;
        executor.commit_oplog(&id).await?;
        let before = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(count_agent_invocation_pair_since(&before, OplogIndex::INITIAL) == (2, 1));
        let start = if operation == Operation::Commit {
            let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
            ensure!(!entries.values().any(|e| matches!(e, OplogEntry::CommittedRemoteTransaction { .. })));
            entries.values().find_map(|e| match e { OplogEntry::PreCommitRemoteTransaction { begin_index, .. } => Some(*begin_index), _ => None }).context("entered commit must follow committed pre-marker")?
        } else {
            let boundary = if operation == Operation::Keyvalue { "keyvalue::eventual::set" } else { "rdbms::postgres::db-connection::execute" };
            let starts: Vec<_> = before.iter().filter_map(|e| match &e.entry { PublicOplogEntry::Start(call) if call.function_name == boundary => Some(e.oplog_index), _ => None }).collect();
            ensure!(starts.len() == 1, "expected one provider Start: {before:#?}");
            starts[0]
        };
        ensure!(terminals(&before, start) == 0);
        ensure!(worker.concurrent_agent_permit_is_held().await && worker.monthly_window_active_for_test());
        tokio::time::timeout(Duration::from_secs(10), polls.recv()).await?.context("monthly timer")?;
        let account = limits.initialize_account(context.account_id).await?;
        let mut raw = worker.raw_interrupt_for_test();
        let mut exhausted = policy.clone();
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        refreshes.push(tokio_util::task::AbortOnDropHandle::new(
            applied_refresh(&limits, &registry, context.account_id).await?,
        ));
        let attempt = tokio::time::timeout(Duration::from_secs(10), attempts.recv()).await?.context("monthly stop acceptance")?;
        ensure!(tokio::time::timeout(Duration::from_secs(10), attempt.completed).await??);
        let kind = tokio::time::timeout(Duration::from_secs(10), raw.recv()).await??;
        ensure!(matches!(kind, InterruptKind::Suspend(_)) && worker.frozen_stop_for_test() == Some(kind));
        let baseline = registry.applied_updates().len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        refreshes.push(tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
            let limits = limits.clone();
            async move { limits.run_batch_for_test().await }
        })));
        tokio::time::timeout(Duration::from_secs(10), async {
            while !registry.applied_updates()[baseline..].iter().any(|u|
                u.memory_gb_seconds_delta > 0 || u.memory_byte_nanoseconds_remainder > 0)
            {
                tokio::task::yield_now().await;
            }
        }).await.context("held account interval did not submit memory byte-time")?;
        ensure!(state.call.state.load(Ordering::SeqCst) == PENDING, "accepted stop dropped or returned provider work");
        ensure!(!invocation.is_finished() && worker.is_loaded().await && worker.concurrent_agent_permit_is_held().await);
        ensure!(worker.monthly_window_active_for_test() && clock.active_sleeps() > 0 && account.monthly_observer_count_for_test() == 1);
        ensure!(registry.applied_updates()[baseline..].iter().any(|u| u.memory_gb_seconds_delta > 0 || u.memory_byte_nanoseconds_remainder > 0), "held account interval must keep accruing memory byte-time");
        let held = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(terminals(&held, start) == 0 && count_agent_invocation_pair_since(&held, OplogIndex::INITIAL) == (2, 1));
        ensure!(!held.iter().any(|e| matches!(e.entry, PublicOplogEntry::Cancelled(_))));
        state.call.release.add_permits(1);
        if operation == Operation::Commit {
            state.cleanup.wait_pending().await?;
            ensure!(state.call.state.load(Ordering::SeqCst) == RETURNED);
            let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
            ensure!(entries.values().any(|e| matches!(e, OplogEntry::CommittedRemoteTransaction { begin_index, .. } if *begin_index == start)));
            ensure!(entries.values().filter(|e| matches!(e, OplogEntry::End { start_index, .. } if *start_index == start)).count() == 1);
            ensure!(worker.concurrent_agent_permit_is_held().await && worker.monthly_window_active_for_test() && !invocation.is_finished());
            ensure!(state.effects() == ["tx-statement", "pre-commit", "commit"]);
            state.cleanup.release.add_permits(1);
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while worker.is_loaded().await || worker.concurrent_agent_permit_is_held().await { tokio::task::yield_now().await; }
        }).await.context("physical completion-only release")?;
        worker.join_accepted_stops_for_test().await?;
        worker.retained_cleanup_for_test().await?;
        ensure!(worker.unload_succeeded_for_test() && !worker.monthly_window_active_for_test());
        ensure!(clock.active_sleeps() == 0 && account.monthly_observer_count_for_test() == 0);
        ensure!(state.call.state.load(Ordering::SeqCst) == RETURNED);
        let stopped = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        ensure!(terminals(&stopped, start) == 1, "provider completion must retain its real End: {stopped:#?}");
        ensure!(!stopped.iter().any(|e| matches!(e.entry, PublicOplogEntry::Cancelled(_))));
        let expected: Vec<String> = if operation == Operation::Commit { ["tx-statement", "pre-commit", "commit", "cleanup"].into_iter().map(str::to_string).collect() } else { vec!["first".to_string()] };
        ensure!(state.effects() == expected, "post-completion successor ran before stop: effects={:?}, starts={:?}", state.effects(), state.starts());
        ensure!(state.starts() == if operation == Operation::Commit { vec!["tx-statement", "commit", "cleanup"] } else { vec!["first"] });
        let entries = worker.oplog().read_exact(OplogIndex::INITIAL, worker.oplog().current_oplog_index().await.as_u64()).await;
        let errors: Vec<_> = entries.values().filter_map(|e| match e { OplogEntry::Error { error, .. } => Some(error), _ => None }).collect();
        if durable {
            ensure!(errors.is_empty(), "{errors:?}");
            ensure!(!invocation.is_finished());
        } else {
            ensure!(matches!(errors.as_slice(), [AgentError::EphemeralCannotSuspend(reason)] if reason.reason == "monthly memory exhausted"), "{errors:?}");
            ensure!(tokio::time::timeout(Duration::from_secs(10), &mut invocation).await??.is_err());
        }
        for refresh in refreshes.drain(..) {
            tokio::time::timeout(Duration::from_secs(10), refresh).await
                .context("Registry refresh did not settle after physical release")??;
        }
        limits.run_batch_for_test().await;
        let settled = registry.applied_updates().len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        limits.run_batch_for_test().await;
        ensure!(registry.applied_updates()[settled..].iter().all(|u| u.memory_gb_seconds_delta == 0 && u.memory_byte_nanoseconds_remainder == 0));
        registry.set_policy(policy);
        applied_refresh(&limits, &registry, context.account_id).await?.await?;
        if durable {
            executor.resume(&id, false).await?;
            tokio::time::timeout(Duration::from_secs(10), &mut invocation).await.context("ordinary same-key reconstruction")???;
            let recovered = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
            ensure!(terminals(&recovered, start) == 1 && count_agent_invocation_pair_since(&recovered, OplogIndex::INITIAL) == (2, 2));
            let after = state.effects();
            ensure!(after == if operation == Operation::Commit { expected } else { vec!["first".to_string(), "second".to_string()] }, "completed history repeated a provider effect: {after:?}");
            executor.invoke_and_await_agent_with_key(&component, &name, &key, method, input.clone()).await?;
            ensure!(state.effects() == after, "same-key lookup executed provider work");
        } else {
            executor.invoke_and_await_agent(&component, &agent_id!("EphemeralDbKvCompletion", "completion-probe"), method, input.clone()).await?;
            ensure!(state.effects() == ["first", "first", "second"]);
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    drop(release_providers);
    let refresh_cleanup = async {
        for refresh in refreshes {
            tokio::time::timeout(Duration::from_secs(10), refresh)
                .await
                .context("test refresh cleanup did not settle after provider release")??;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if !invocation.is_finished() {
        invocation.abort();
        let _ = invocation.await;
    }
    match (result, refresh_cleanup) {
        (Err(error), Err(cleanup)) => {
            Err(error.context(format!("test cleanup also failed: {cleanup}")))
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), cleanup) => cleanup,
    }
}
