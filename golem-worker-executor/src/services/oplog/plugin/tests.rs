// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use super::*;
use crate::services::oplog::{DurableStreamOplogRecord, OplogFence};
use golem_common::base_model::component_metadata::KnownExports;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{
    ComponentId, ComponentName, ComponentRevision, InstalledPlugin,
};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::{AgentFingerprint, OplogProcessorCheckpointState, Timestamp};
use golem_common::read_only_lock;
use golem_service_base::model::component::Component;
use test_r::test;

// --------------------------------------------------------------------------
// U1: Deterministic idempotency key stability
// --------------------------------------------------------------------------

#[test]
fn deterministic_idempotency_key_same_inputs_produce_same_key() {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "TestPlugin(test-worker)".to_string(),
    };
    let plugin_id = EnvironmentPluginGrantId::new();
    let first = OplogIndex::from_u64(10);
    let last = OplogIndex::from_u64(20);

    let key1 = oplog_processor_idempotency_key(&agent_id, &plugin_id, first, last);
    let key2 = oplog_processor_idempotency_key(&agent_id, &plugin_id, first, last);

    assert_eq!(
        key1, key2,
        "Same inputs must produce the same idempotency key"
    );
}

#[test]
fn deterministic_idempotency_key_different_batch_range_produces_different_key() {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "TestPlugin(test-worker)".to_string(),
    };
    let plugin_id = EnvironmentPluginGrantId::new();

    let key1 = oplog_processor_idempotency_key(
        &agent_id,
        &plugin_id,
        OplogIndex::from_u64(10),
        OplogIndex::from_u64(20),
    );
    let key2 = oplog_processor_idempotency_key(
        &agent_id,
        &plugin_id,
        OplogIndex::from_u64(21),
        OplogIndex::from_u64(30),
    );

    assert_ne!(
        key1, key2,
        "Different batch ranges must produce different keys"
    );
}

#[test]
fn deterministic_idempotency_key_different_worker_produces_different_key() {
    let component_id = ComponentId::new();
    let agent_id1 = AgentId {
        component_id,
        agent_id: "worker-a".to_string(),
    };
    let agent_id2 = AgentId {
        component_id,
        agent_id: "worker-b".to_string(),
    };
    let plugin_id = EnvironmentPluginGrantId::new();
    let first = OplogIndex::from_u64(1);
    let last = OplogIndex::from_u64(5);

    let key1 = oplog_processor_idempotency_key(&agent_id1, &plugin_id, first, last);
    let key2 = oplog_processor_idempotency_key(&agent_id2, &plugin_id, first, last);

    assert_ne!(key1, key2, "Different workers must produce different keys");
}

#[test]
fn deterministic_idempotency_key_different_plugin_produces_different_key() {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "TestPlugin(test-worker)".to_string(),
    };
    let plugin_id1 = EnvironmentPluginGrantId::new();
    let plugin_id2 = EnvironmentPluginGrantId::new();
    let first = OplogIndex::from_u64(1);
    let last = OplogIndex::from_u64(5);

    let key1 = oplog_processor_idempotency_key(&agent_id, &plugin_id1, first, last);
    let key2 = oplog_processor_idempotency_key(&agent_id, &plugin_id2, first, last);

    assert_ne!(key1, key2, "Different plugins must produce different keys");
}

// --------------------------------------------------------------------------
// Helpers: recording mock for OplogProcessorPlugin and fake ComponentService
// --------------------------------------------------------------------------

/// Records all `send()` calls for verification
struct RecordingOplogProcessorPlugin {
    sends: async_lock::Mutex<Vec<RecordedSend>>,
    lookups: async_lock::Mutex<Vec<RecordedLookup>>,
    send_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    target_agent_id: AgentId,
    resolve_count: std::sync::atomic::AtomicUsize,
    failed_sends_remaining: std::sync::atomic::AtomicUsize,
    target_is_local: std::sync::atomic::AtomicBool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct RecordedSend {
    target_agent_id: AgentId,
    initial_oplog_index: OplogIndex,
    entry_count: usize,
    entries: Vec<OplogEntry>,
}

#[derive(Debug, Clone)]
struct RecordedLookup {
    caller_account_id: AccountId,
}

impl RecordingOplogProcessorPlugin {
    fn new() -> Self {
        Self {
            sends: async_lock::Mutex::new(Vec::new()),
            lookups: async_lock::Mutex::new(Vec::new()),
            send_gate: None,
            target_agent_id: AgentId {
                component_id: ComponentId::new(),
                agent_id: "mock-target".to_string(),
            },
            resolve_count: std::sync::atomic::AtomicUsize::new(0),
            failed_sends_remaining: std::sync::atomic::AtomicUsize::new(0),
            target_is_local: std::sync::atomic::AtomicBool::new(true),
        }
    }

    fn fail_next_send_from_remote_target(&self) {
        self.failed_sends_remaining
            .store(1, std::sync::atomic::Ordering::Relaxed);
        self.target_is_local
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    async fn send_count(&self) -> usize {
        self.sends.lock().await.len()
    }

    async fn sends(&self) -> Vec<RecordedSend> {
        self.sends.lock().await.clone()
    }

    async fn lookups(&self) -> Vec<RecordedLookup> {
        self.lookups.lock().await.clone()
    }
}

#[async_trait]
impl OplogProcessorPlugin for RecordingOplogProcessorPlugin {
    async fn resolve_target(
        &self,
        _environment_id: EnvironmentId,
        _plugin: &InstalledPlugin,
    ) -> Result<AgentId, WorkerExecutorError> {
        self.resolve_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(self.target_agent_id.clone())
    }

    async fn send(
        &self,
        _worker_metadata: AgentMetadata,
        _plugin: &InstalledPlugin,
        target_agent_id: &AgentId,
        initial_oplog_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), WorkerExecutorError> {
        self.sends.lock().await.push(RecordedSend {
            target_agent_id: target_agent_id.clone(),
            initial_oplog_index,
            entry_count: entries.len(),
            entries,
        });
        if let Some((entered, release)) = &self.send_gate {
            entered.notify_one();
            release.notified().await;
        }
        if self
            .failed_sends_remaining
            .try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            return Err(WorkerExecutorError::unknown("injected send failure"));
        }
        Ok(())
    }

    async fn invalidate_target(&self, _environment_id: EnvironmentId, _plugin: &InstalledPlugin) {
        // no-op in tests
    }

    async fn is_local(&self, _agent_id: &AgentId) -> Result<bool, WorkerExecutorError> {
        Ok(self
            .target_is_local
            .load(std::sync::atomic::Ordering::Relaxed))
    }

    async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError> {
        Ok(())
    }

    async fn lookup_invocation_status(
        &self,
        _environment_id: EnvironmentId,
        _target_agent_id: &AgentId,
        caller_account_id: AccountId,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationStatus, WorkerExecutorError> {
        self.lookups
            .lock()
            .await
            .push(RecordedLookup { caller_account_id });
        Ok(InvocationStatus::Unknown)
    }
}

#[test]
#[test_r::timeout("10s")]
async fn forwarding_retirement_cancels_send_without_confirming_or_restarting_it() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status) = test_worker_metadata(HashSet::from([grant_id]));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let mut plugin = RecordingOplogProcessorPlugin::new();
    plugin.send_gate = Some((entered.clone(), release));
    let plugin = Arc::new(plugin);
    let oplog = Arc::new(
        ForwardingOplog::new(
            Arc::new(InMemoryOplog::new()),
            plugin.clone(),
            Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
                grant_id,
            )),
            metadata,
            status,
            OplogIndex::NONE,
            Box::new(|| {}),
            1,
            Duration::from_secs(3600),
        )
        .await,
    );
    oplog.add(OplogEntry::recovery_succeeded()).await.unwrap();
    let commit = tokio::spawn({
        let oplog = oplog.clone();
        async move { oplog.commit(CommitLevel::Always).await }
    });
    entered.notified().await;
    oplog.executor_shutdown_handle().fence();
    oplog.drain_forwarding().await;
    let committed = commit.await.unwrap().unwrap();
    assert_eq!(committed.len(), 3);
    assert!(matches!(committed.get(&OplogIndex::from_u64(3)),
        Some(OplogEntry::OplogProcessorCheckpoint { confirmed_up_to, sending_up_to, .. })
            if *confirmed_up_to == OplogIndex::NONE && *sending_up_to == OplogIndex::INITIAL));
    oplog.jobs.send(ForwardingJob::Tick).unwrap();
    assert_eq!(
        oplog.add(OplogEntry::recovery_succeeded()).await.unwrap(),
        OplogIndex::from_u64(4)
    );
    let cleanup = oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(cleanup.len(), 1);
    assert_eq!(plugin.send_count().await, 1);
    assert_eq!(oplog.current_oplog_index().await, OplogIndex::from_u64(4));
}

#[test]
#[test_r::timeout("10s")]
async fn stale_forwarding_shutdown_fence_does_not_stop_replacement() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (old_metadata, old_status) = test_worker_metadata(HashSet::new());
    let old = Arc::new(
        ForwardingOplog::new(
            Arc::new(InMemoryOplog::new()),
            Arc::new(RecordingOplogProcessorPlugin::new()),
            Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
                grant_id,
            )),
            old_metadata,
            old_status,
            OplogIndex::NONE,
            Box::new(|| {}),
            usize::MAX,
            Duration::from_secs(3600),
        )
        .await,
    );
    let stale_shutdown = old.executor_shutdown_handle();
    let weak = Arc::downgrade(&old);
    let (new_metadata, new_status) = test_worker_metadata(HashSet::new());
    let replacement = ForwardingOplog::new(
        Arc::new(InMemoryOplog::new()),
        Arc::new(RecordingOplogProcessorPlugin::new()),
        Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
            grant_id,
        )),
        new_metadata,
        new_status,
        OplogIndex::NONE,
        Box::new(|| {}),
        usize::MAX,
        Duration::from_secs(3600),
    )
    .await;

    drop(old);
    assert!(weak.upgrade().is_none());
    stale_shutdown.fence();
    stale_shutdown.fence();
    stale_shutdown.close_and_wait().await.unwrap();

    assert!(!replacement.forwarding_retired.is_cancelled());
    assert_eq!(
        replacement
            .add(OplogEntry::recovery_succeeded())
            .await
            .unwrap(),
        OplogIndex::INITIAL
    );
    replacement.commit(CommitLevel::Always).await.unwrap();
}

#[test]
#[test_r::timeout("10s")]
async fn forwarding_drain_preserves_checkpoint_add_and_commit_tails() {
    for gate_add in [true, false] {
        let grant_id = EnvironmentPluginGrantId::new();
        let (metadata, status) = test_worker_metadata(HashSet::from([grant_id]));
        let inner = Arc::new(InMemoryOplog::new());
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let gate = if gate_add {
            &inner.checkpoint_add_gate
        } else {
            &inner.checkpoint_commit_gate
        };
        *gate.lock().unwrap() = Some((entered.clone(), release.clone()));
        let plugin = Arc::new(RecordingOplogProcessorPlugin::new());
        let oplog = ForwardingOplog::new(
            inner.clone(),
            plugin.clone(),
            Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
                grant_id,
            )),
            metadata,
            status,
            OplogIndex::NONE,
            Box::new(|| {}),
            usize::MAX,
            Duration::from_secs(3600),
        )
        .await;
        oplog.add(OplogEntry::recovery_succeeded()).await.unwrap();
        oplog.commit(CommitLevel::Always).await.unwrap();
        oplog.jobs.send(ForwardingJob::Tick).unwrap();
        entered.notified().await;
        oplog.fence_forwarding();
        let mut drain = Box::pin(oplog.drain_forwarding());
        assert!(futures::poll!(drain.as_mut()).is_pending());
        release.notify_one();
        drain.await;
        let committed = oplog.commit(CommitLevel::Always).await.unwrap();
        assert_eq!(committed.len(), 1);
        assert!(matches!(
            committed.get(&OplogIndex::from_u64(2)),
            Some(OplogEntry::OplogProcessorCheckpoint { .. })
        ));
        assert_eq!(
            *inner.committed_idx.lock().unwrap(),
            OplogIndex::from_u64(2)
        );
        assert_eq!(plugin.send_count().await, 0);
    }
}

#[test]
#[test_r::timeout("10s")]
async fn executor_shutdown_rejects_post_fence_ticks_and_closes_the_forwarding_generation() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status) = test_worker_metadata(HashSet::from([grant_id]));
    let plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let inner = Arc::new(InMemoryOplog::new());
    let oplog = ForwardingOplog::new(
        inner.clone(),
        plugin.clone(),
        Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
            grant_id,
        )),
        metadata,
        status,
        OplogIndex::NONE,
        Box::new(|| {}),
        usize::MAX,
        Duration::from_secs(3600),
    )
    .await;
    oplog.add(OplogEntry::recovery_succeeded()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let shutdown = oplog.executor_shutdown_handle();
    shutdown.fence();
    oplog.tick().await;
    assert_eq!(plugin.send_count().await, 0);
    assert_eq!(*inner.current_idx.lock().unwrap(), OplogIndex::INITIAL);

    shutdown.close_and_wait().await.unwrap();

    assert!(oplog.is_retired());
    assert_eq!(plugin.send_count().await, 0);
    assert_eq!(*inner.current_idx.lock().unwrap(), OplogIndex::INITIAL);
}

#[test]
async fn ordinary_drain_keeps_forwarding_available_for_the_next_worker() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status) = test_worker_metadata(HashSet::from([grant_id]));
    let plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let oplog = ForwardingOplog::new(
        Arc::new(InMemoryOplog::new()),
        plugin.clone(),
        Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
            grant_id,
        )),
        metadata,
        status,
        OplogIndex::NONE,
        Box::new(|| {}),
        1,
        Duration::from_secs(3600),
    )
    .await;
    oplog.drain_forwarding().await;
    oplog.add(OplogEntry::recovery_succeeded()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(plugin.send_count().await, 1);
}

/// A fake ComponentService that returns a component with one installed oplog processor plugin
struct FakeComponentService {
    installed_plugins: Vec<InstalledPlugin>,
}

impl FakeComponentService {
    fn with_one_oplog_processor_plugin(grant_id: EnvironmentPluginGrantId) -> Self {
        Self {
            installed_plugins: vec![InstalledPlugin {
                environment_plugin_grant_id: grant_id,
                priority: golem_common::model::component::PluginPriority(0),
                parameters: BTreeMap::new(),
                plugin_registration_id: PluginRegistrationId::new(),
                plugin_name: "test-oplog-plugin".to_string(),
                plugin_version: "1.0.0".to_string(),
                oplog_processor_component_id: Some(ComponentId::new()),
                oplog_processor_component_revision: Some(ComponentRevision::INITIAL),
            }],
        }
    }

    fn empty() -> Self {
        Self {
            installed_plugins: Vec::new(),
        }
    }
}

#[async_trait]
impl ComponentService for FakeComponentService {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        unimplemented!("not needed for forwarding oplog tests")
    }

    async fn get_metadata(
        &self,
        component_id: ComponentId,
        _forced_revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        use golem_common::base_model::component_metadata::AgentTypeProvisionConfig;
        use golem_common::model::agent::AgentTypeName;
        use golem_common::model::card::recipient::RecipientPattern;
        use golem_common::model::component::AgentTypeInitialPermissions;
        use std::collections::BTreeMap;

        let provision_configs = if self.installed_plugins.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([(
                AgentTypeName("TestPlugin".to_string()),
                AgentTypeProvisionConfig {
                    initial_permissions: AgentTypeInitialPermissions::default_for_recipient(
                        RecipientPattern::Account {
                            account: golem_common::model::account::AccountEmail::new("test@golem"),
                        },
                    )
                    .to_polymorphic_card(),
                    plugins: self.installed_plugins.clone(),
                    env: BTreeMap::new(),
                    config: Vec::new(),
                    files: Vec::new(),
                },
            )])
        };

        Ok(Component {
            id: component_id,
            revision: ComponentRevision::INITIAL,
            environment_id: EnvironmentId::new(),
            component_name: ComponentName("test-component".to_string()),
            hash: diff::Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: AccountId::new(),
            account_email: golem_common::model::account::AccountEmail::new("test@golem"),
            application_name: golem_common::model::application::ApplicationName::try_from(
                "test-app".to_string(),
            )
            .unwrap(),
            environment_name: golem_common::model::environment::EnvironmentName::try_from(
                "test-env",
            )
            .unwrap(),
            component_size: 100,
            metadata: ComponentMetadata::from_parts(
                KnownExports::default(),
                vec![],
                vec![],
                provision_configs,
            ),
            created_at: chrono::Utc::now(),
            wasm_hash: diff::Hash::empty(),
            object_store_key: String::new(),
        })
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: ApplicationId,
        _resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        Ok(None)
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        Vec::new()
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
}

/// A minimal in-memory oplog for testing ForwardingOplog behavior.
///
/// Entry `i` (1-based [`OplogIndex`]) is stored at `entries[i - 1]`, so the indices returned
/// by `add`/`add_pair` agree with `read`/`read_exact` — the forwarding wrapper's mirrored
/// buffer relies on this. `commit` returns the newly committed suffix, like the production
/// leaf oplogs, so the forwarding actor's `last_committed_idx` tracking is exercised.
#[allow(dead_code)]
struct InMemoryOplog {
    entries: std::sync::Mutex<Vec<OplogEntry>>,
    current_idx: std::sync::Mutex<OplogIndex>,
    committed_idx: std::sync::Mutex<OplogIndex>,
    checkpoint_add_gate:
        std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    checkpoint_commit_gate:
        std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    read_exact_count: std::sync::atomic::AtomicUsize,
    read_exact_requests: std::sync::Mutex<Vec<(OplogIndex, u64)>>,
    /// Refuses the next `commit` as fenced once set. The refusal latches, as it does on the
    /// primary oplog, so every commit after it is refused and `fence` reports it.
    armed_fence: std::sync::Mutex<Option<OplogFence>>,
    latched_fence: std::sync::OnceLock<OplogFence>,
}

#[allow(dead_code)]
impl InMemoryOplog {
    fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
            current_idx: std::sync::Mutex::new(OplogIndex::NONE),
            committed_idx: std::sync::Mutex::new(OplogIndex::NONE),
            checkpoint_add_gate: std::sync::Mutex::new(None),
            checkpoint_commit_gate: std::sync::Mutex::new(None),
            read_exact_count: std::sync::atomic::AtomicUsize::new(0),
            read_exact_requests: std::sync::Mutex::new(Vec::new()),
            armed_fence: std::sync::Mutex::new(None),
            latched_fence: std::sync::OnceLock::new(),
        }
    }

    fn arm_fence(&self, fence: OplogFence) {
        *self.armed_fence.lock().unwrap() = Some(fence);
    }

    fn read_exact_count(&self) -> usize {
        self.read_exact_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn read_exact_requests(&self) -> Vec<(OplogIndex, u64)> {
        self.read_exact_requests.lock().unwrap().clone()
    }
}

impl Debug for InMemoryOplog {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryOplog").finish()
    }
}

#[async_trait]
impl Oplog for InMemoryOplog {
    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        let gate = if matches!(entry, OplogEntry::OplogProcessorCheckpoint { .. }) {
            self.checkpoint_add_gate.lock().unwrap().take()
        } else {
            None
        };
        let mut entries = self.entries.lock().unwrap();
        let mut idx = self.current_idx.lock().unwrap();
        *idx = idx.next();
        entries.push(entry);
        let result = *idx;
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            Ok(result)
        })
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> crate::services::oplog::OplogAddPairReceipt {
        let mut entries = self.entries.lock().unwrap();
        let mut idx = self.current_idx.lock().unwrap();
        *idx = idx.next();
        let first_idx = *idx;
        entries.push(start);
        *idx = idx.next();
        let second_idx = *idx;
        entries.push(make_second(first_idx));
        Box::pin(async move { Ok((first_idx, second_idx)) })
    }

    async fn add_durable_stream_batch(
        &self,
        make_batch: DurableStreamBatchBuilder,
    ) -> Result<Vec<(OplogIndex, OplogEntry)>, OplogError> {
        let mut entries = self.entries.lock().unwrap();
        let mut idx = self.current_idx.lock().unwrap();
        let records = make_batch(idx.next());
        let mut result = Vec::with_capacity(records.len());
        for record in records {
            *idx = idx.next();
            let entry = record.into_inline_entry();
            entries.push(entry.clone());
            result.push((*idx, entry));
        }
        Ok(result)
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
        let index = self.add(entry.clone()).await?;
        Ok(OrderedOplogStart {
            index,
            entry,
            pending_upload: crate::services::oplog::PendingUpload::already_durable(),
        })
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, OplogError> {
        let mut entries = self.entries.lock().unwrap();
        let mut idx = self.current_idx.lock().unwrap();
        let index = idx.next();
        let (serialized_request, build_start) = build_request(index)?;
        let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
        *idx = index;
        entries.push(entry.clone());
        Ok(OrderedOplogStart {
            index,
            entry,
            pending_upload: crate::services::oplog::PendingUpload::already_durable(),
        })
    }

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
        0
    }

    async fn commit(
        &self,
        level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, crate::services::oplog::OplogError> {
        // This oplog never commits by itself, so there is nothing to report.
        if level == CommitLevel::ReportOnly {
            return Ok(BTreeMap::new());
        }
        if let Some(fence) = self.latched_fence.get() {
            return Err(OplogError::Fenced(fence.clone()));
        }
        if let Some(fence) = self.armed_fence.lock().unwrap().take() {
            let _ = self.latched_fence.set(fence.clone());
            return Err(OplogError::Fenced(fence));
        }
        let gate = if matches!(
            self.entries.lock().unwrap().last(),
            Some(OplogEntry::OplogProcessorCheckpoint { .. })
        ) {
            self.checkpoint_commit_gate.lock().unwrap().take()
        } else {
            None
        };
        if let Some((entered, release)) = gate {
            entered.notify_one();
            release.notified().await;
        }
        let entries = self.entries.lock().unwrap();
        let current = *self.current_idx.lock().unwrap();
        let mut committed = self.committed_idx.lock().unwrap();
        let mut result = BTreeMap::new();
        let mut idx = *committed;
        while idx < current {
            idx = idx.next();
            result.insert(idx, entries[(idx.as_u64() - 1) as usize].clone());
        }
        *committed = current;
        Ok(result)
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        *self.current_idx.lock().unwrap()
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(
        &self,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<bool, OplogError> {
        Ok(true)
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, UnreadableOplog> {
        self.read_exact_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.read_exact_requests
            .lock()
            .unwrap()
            .push((oplog_index, n));
        let entries = self.entries.lock().unwrap();
        let start: u64 = oplog_index.into();
        let mut result = BTreeMap::new();
        for i in start..(start + n) {
            let entry = entries.get((i - 1) as usize).unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry in exact range [{oplog_index}..={}]",
                    OplogIndex::from_u64(start + n - 1)
                )
            });
            result.insert(OplogIndex::from_u64(i), entry.clone());
        }
        Ok(result)
    }

    async fn length(&self) -> Result<u64, UnreadableOplog> {
        Ok(self.entries.lock().unwrap().len() as u64)
    }

    async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
        unimplemented!()
    }

    async fn download_raw_payload(
        &self,
        _payload_id: PayloadId,
        _md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        unimplemented!()
    }

    fn fence(&self) -> Option<OplogFence> {
        self.latched_fence.get().cloned()
    }
}

fn test_worker_metadata(
    active_plugins: HashSet<EnvironmentPluginGrantId>,
) -> (
    AgentMetadata,
    read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
) {
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "TestPlugin(test-worker)".to_string(),
    };
    let environment_id = EnvironmentId::new();
    let account_id = AccountId::new();
    let status = AgentStatusRecord {
        active_plugins,
        ..Default::default()
    };

    let metadata = AgentMetadata {
        agent_id,
        owner_kind: golem_common::model::agent::OwnerKind::ComponentAgent,
        env: vec![],
        environment_id,
        created_by: account_id,
        created_by_email: golem_common::model::account::AccountEmail::new("test@golem"),
        config: Vec::new(),
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: status.clone(),
        original_phantom_id: None,
        fingerprint: AgentFingerprint::new(),
        agent_mode: AgentMode::Durable,
    };

    let status_lock = read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(
        arc_swap::ArcSwap::from_pointee(status),
    ));

    (metadata, status_lock)
}

fn test_worker_metadata_with_status_writer(
    active_plugins: HashSet<EnvironmentPluginGrantId>,
) -> (
    AgentMetadata,
    read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
    Arc<arc_swap::ArcSwap<AgentStatusRecord>>,
) {
    let (metadata, _) = test_worker_metadata(active_plugins);
    let status_writer = Arc::new(arc_swap::ArcSwap::from_pointee(
        metadata.last_known_status.clone(),
    ));
    let status_lock = read_only_lock::arc_swap::ReadOnlyView::new(status_writer.clone());
    (metadata, status_lock, status_writer)
}

// --------------------------------------------------------------------------
// U6: Empty buffer → no checkpoint written, no invoke called
// --------------------------------------------------------------------------

#[test]
async fn no_committed_entries_no_send() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let components: Arc<dyn ComponentService> = Arc::new(
        FakeComponentService::with_one_oplog_processor_plugin(grant_id),
    );
    let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

    let mut state = ForwardingOplogState {
        forwarding_retired: CancellationToken::new(),
        forwarding_activity: ActivityGate::new(),
        pending_checkpoint_activity: None,
        buffer: None,
        buffer_start_idx: OplogIndex::INITIAL,
        commit_count: 0,
        last_send: Instant::now(),
        oplog_plugins: recording_plugin.clone(),
        initial_worker_metadata: metadata,
        last_known_status: status_lock,
        last_oplog_idx: OplogIndex::NONE,
        last_committed_idx: OplogIndex::NONE,
        components,
        inner,
        plugin_state: HashMap::from([(
            grant_id,
            LivePluginState {
                target_agent_id: None,
                confirmed_up_to: OplogIndex::NONE,
                sending_up_to: OplogIndex::NONE,
                send_in_progress: false,
                last_batch_start: OplogIndex::NONE,
            },
        )]),
        pending_direct_commits: BTreeMap::new(),
        worker_event_service: None,
        monitor_tasks: Vec::new(),
    };

    // No committed entries (last_committed_idx = NONE) — try_flush should be a no-op
    state.try_flush().await;

    assert_eq!(
        recording_plugin.send_count().await,
        0,
        "No committed entries should not trigger any plugin sends"
    );
}

// --------------------------------------------------------------------------
// A fenced checkpoint ends forwarding: the flushes after it write and send nothing
// --------------------------------------------------------------------------

#[test]
async fn a_fenced_checkpoint_stops_later_flushes_before_they_write() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let components: Arc<dyn ComponentService> = Arc::new(
        FakeComponentService::with_one_oplog_processor_plugin(grant_id),
    );
    let in_memory = Arc::new(InMemoryOplog::new());
    in_memory.add(grow_memory(1)).await.unwrap();
    in_memory.add(grow_memory(2)).await.unwrap();
    in_memory.commit(CommitLevel::Always).await.unwrap();
    in_memory.arm_fence(OplogFence {
        agent_id: metadata.agent_id.clone(),
        expected_epoch: ShardEpoch(1),
        actual_epoch: Some(ShardEpoch(2)),
    });
    let inner: Arc<dyn Oplog> = in_memory.clone();

    // No target yet, so the first write is the checkpoint recording the resolved target. It
    // is written before the grant is marked as sending, so nothing but the fence stops the
    // next flush from resolving and writing it again.
    let mut state = ForwardingOplogState {
        forwarding_retired: CancellationToken::new(),
        forwarding_activity: ActivityGate::new(),
        pending_checkpoint_activity: None,
        buffer: None,
        buffer_start_idx: OplogIndex::from_u64(3),
        commit_count: 0,
        last_send: Instant::now(),
        oplog_plugins: recording_plugin.clone(),
        initial_worker_metadata: metadata,
        last_known_status: status_lock,
        last_oplog_idx: OplogIndex::from_u64(2),
        last_committed_idx: OplogIndex::from_u64(2),
        components,
        inner,
        plugin_state: HashMap::from([(
            grant_id,
            LivePluginState {
                target_agent_id: None,
                confirmed_up_to: OplogIndex::NONE,
                sending_up_to: OplogIndex::NONE,
                send_in_progress: false,
                last_batch_start: OplogIndex::NONE,
            },
        )]),
        pending_direct_commits: BTreeMap::new(),
        worker_event_service: None,
        monitor_tasks: Vec::new(),
    };

    state.try_flush().await;
    assert_eq!(recording_plugin.send_count().await, 0);
    assert_eq!(
        in_memory.length().await.unwrap(),
        3,
        "the refused checkpoint is the only entry the first flush adds"
    );
    let last_oplog_idx = state.last_oplog_idx;

    state.try_flush().await;
    state.try_flush().await;

    assert_eq!(
        in_memory.length().await.unwrap(),
        3,
        "a flush after the fence must not add another checkpoint"
    );
    assert_eq!(state.last_oplog_idx, last_oplog_idx);
    assert_eq!(recording_plugin.send_count().await, 0);
    assert_eq!(
        recording_plugin
            .resolve_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "a flush after the fence must not resolve the target again"
    );
}

// --------------------------------------------------------------------------
// U5 (partial): No active plugins → no send even with entries in buffer
// --------------------------------------------------------------------------

#[test]
async fn no_active_plugins_no_send() {
    let (metadata, status_lock) = test_worker_metadata(HashSet::new());
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let components: Arc<dyn ComponentService> = Arc::new(FakeComponentService::empty());
    let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

    let mut state = ForwardingOplogState {
        forwarding_retired: CancellationToken::new(),
        forwarding_activity: ActivityGate::new(),
        pending_checkpoint_activity: None,
        buffer: Some(VecDeque::from([OplogEntry::GrowMemory {
            timestamp: Timestamp::now_utc(),
            delta: 100,
        }])),
        buffer_start_idx: OplogIndex::INITIAL,
        commit_count: 0,
        last_send: Instant::now(),
        oplog_plugins: recording_plugin.clone(),
        initial_worker_metadata: metadata,
        last_known_status: status_lock,
        last_oplog_idx: OplogIndex::from_u64(1),
        last_committed_idx: OplogIndex::from_u64(1),
        components,
        inner,
        plugin_state: HashMap::new(),
        pending_direct_commits: BTreeMap::new(),
        worker_event_service: None,
        monitor_tasks: Vec::new(),
    };

    state.try_flush().await;

    assert_eq!(
        recording_plugin.send_count().await,
        0,
        "No active plugins should not trigger any plugin sends"
    );
}

// --------------------------------------------------------------------------
// Basic send verification: active plugin + entries → plugin receives them
// --------------------------------------------------------------------------

#[test]
async fn active_plugin_receives_buffered_entries() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let components: Arc<dyn ComponentService> = Arc::new(
        FakeComponentService::with_one_oplog_processor_plugin(grant_id),
    );
    let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

    // Pre-populate the inner oplog so read_exact can return entries
    let entry1 = OplogEntry::GrowMemory {
        timestamp: Timestamp::now_utc(),
        delta: 100,
    };
    let entry2 = OplogEntry::GrowMemory {
        timestamp: Timestamp::now_utc(),
        delta: 200,
    };
    inner.add(entry1.clone()).await.unwrap();
    inner.add(entry2.clone()).await.unwrap();

    let mut state = ForwardingOplogState {
        forwarding_retired: CancellationToken::new(),
        forwarding_activity: ActivityGate::new(),
        pending_checkpoint_activity: None,
        buffer: Some(VecDeque::from([entry1, entry2])),
        buffer_start_idx: OplogIndex::INITIAL,
        commit_count: 0,
        last_send: Instant::now(),
        oplog_plugins: recording_plugin.clone(),
        initial_worker_metadata: metadata,
        last_known_status: status_lock,
        last_oplog_idx: OplogIndex::from_u64(2),
        last_committed_idx: OplogIndex::from_u64(2),
        components,
        inner,
        plugin_state: HashMap::from([(
            grant_id,
            LivePluginState {
                target_agent_id: None,
                confirmed_up_to: OplogIndex::NONE,
                sending_up_to: OplogIndex::NONE,
                send_in_progress: false,
                last_batch_start: OplogIndex::NONE,
            },
        )]),
        pending_direct_commits: BTreeMap::new(),
        worker_event_service: None,
        monitor_tasks: Vec::new(),
    };

    state.try_flush().await;

    let sends = recording_plugin.sends().await;
    assert_eq!(sends.len(), 1, "Should have sent exactly one batch");
    assert_eq!(sends[0].entry_count, 2, "Batch should contain 2 entries");
}

#[test]
async fn plugin_monitor_looks_up_status_as_original_worker_owner() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
    let worker_owner = metadata.created_by;
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let components: Arc<dyn ComponentService> = Arc::new(
        FakeComponentService::with_one_oplog_processor_plugin(grant_id),
    );
    let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

    let entry = OplogEntry::GrowMemory {
        timestamp: Timestamp::now_utc(),
        delta: 100,
    };
    inner.add(entry.clone()).await.unwrap();

    let mut state = ForwardingOplogState {
        forwarding_retired: CancellationToken::new(),
        forwarding_activity: ActivityGate::new(),
        pending_checkpoint_activity: None,
        buffer: Some(VecDeque::from([entry])),
        buffer_start_idx: OplogIndex::INITIAL,
        commit_count: 0,
        last_send: Instant::now(),
        oplog_plugins: recording_plugin.clone(),
        initial_worker_metadata: metadata,
        last_known_status: status_lock,
        last_oplog_idx: OplogIndex::from_u64(1),
        last_committed_idx: OplogIndex::from_u64(1),
        components,
        inner,
        plugin_state: HashMap::from([(
            grant_id,
            LivePluginState {
                target_agent_id: None,
                confirmed_up_to: OplogIndex::NONE,
                sending_up_to: OplogIndex::NONE,
                send_in_progress: false,
                last_batch_start: OplogIndex::NONE,
            },
        )]),
        pending_direct_commits: BTreeMap::new(),
        worker_event_service: None,
        monitor_tasks: Vec::new(),
    };

    state.try_flush().await;

    assert_eq!(state.monitor_tasks.len(), 1, "Expected a monitoring task");
    for task in state.monitor_tasks.drain(..) {
        let _ = task.await;
    }

    let lookups = recording_plugin.lookups().await;
    assert_eq!(lookups.len(), 1, "Expected exactly one status lookup");
    assert_eq!(lookups[0].caller_account_id, worker_owner);
}

// --------------------------------------------------------------------------
// Reserved-start through the forwarding actor
// --------------------------------------------------------------------------

#[test]
async fn synchronously_enqueued_add_keeps_position_before_later_async_add() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status_lock) = test_worker_metadata(HashSet::new());
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let components: Arc<dyn ComponentService> = Arc::new(
        FakeComponentService::with_one_oplog_processor_plugin(grant_id),
    );
    let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());
    let oplog = ForwardingOplog::new(
        inner,
        recording_plugin,
        components,
        metadata,
        status_lock,
        OplogIndex::NONE,
        Box::new(|| {}),
        usize::MAX,
        Duration::from_secs(3600),
    )
    .await;

    let first = oplog.enqueue_add(grow_memory(1));
    let second = oplog.add(grow_memory(2)).await.unwrap();

    assert_eq!(first.await.unwrap(), OplogIndex::INITIAL);
    assert_eq!(second, OplogIndex::INITIAL.next());
}

#[test]
async fn abandoned_pair_receipt_keeps_pair_before_later_add() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status_lock) = test_worker_metadata(HashSet::new());
    let inner = Arc::new(InMemoryOplog::new());
    let oplog = ForwardingOplog::new(
        inner.clone(),
        Arc::new(RecordingOplogProcessorPlugin::new()),
        Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
            grant_id,
        )),
        metadata,
        status_lock,
        OplogIndex::NONE,
        Box::new(|| {}),
        usize::MAX,
        Duration::from_secs(3600),
    )
    .await;

    drop(oplog.enqueue_add_pair(grow_memory(1), Box::new(|_| grow_memory(2))));
    let later = oplog.add(grow_memory(3)).await.unwrap();
    let entries = inner
        .read_exact(OplogIndex::INITIAL, 3)
        .await
        .unwrap()
        .into_values()
        .collect::<Vec<_>>();

    assert_eq!(later, OplogIndex::from_u64(3));
    assert_eq!(memory_deltas(&entries), vec![1, 2, 3]);
}

/// Reserved starts pass through the forwarding actor in initiation order: the indices the
/// wrapper returns are the ones the inner (leaf) oplog assigned, the mirrored buffer stays in
/// lockstep, and a flush forwards the `Start` entries to the plugin as regular oplog entries.
/// The contract verified here is structural (entry kind, order, batch range); the payload
/// representation is owned by the leaf oplog and is not part of the forwarding contract.
#[test]
async fn reserved_start_forwarded_in_initiation_order_and_reaches_plugin() {
    use crate::services::oplog::OplogOps;
    use golem_common::model::oplog::host_functions::HostFunctionName;
    use golem_common::model::oplog::{DurableFunctionType, HostRequest};
    use golem_common::schema::IntoTypedSchemaValue;

    let grant_id = EnvironmentPluginGrantId::new();
    let (metadata, status_lock) = test_worker_metadata(HashSet::from([grant_id]));
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let components: Arc<dyn ComponentService> = Arc::new(
        FakeComponentService::with_one_oplog_processor_plugin(grant_id),
    );
    let inner: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());

    let oplog = ForwardingOplog::new(
        inner.clone(),
        recording_plugin.clone(),
        components,
        metadata,
        status_lock,
        OplogIndex::NONE,
        Box::new(|| {}),
        1,                         // flush on every commit
        Duration::from_secs(3600), // keep the periodic tick out of this test
    )
    .await;

    let build_start = |function_name: HostFunctionName| {
        move |request_payload| OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name,
            invocation_id: None,
            observational_owner: None,
            request: Some(request_payload),
            durable_function_type: DurableFunctionType::ReadRemote,
            span_started: None,
            denial: None,
        }
    };

    let (first_idx, first_pending) = oplog
        .add_start_with_reserved_payload(
            HostRequest::Custom("first".into_typed_schema_value().unwrap()),
            build_start(HostFunctionName::Custom("first".to_string())),
        )
        .await
        .unwrap();
    let (second_idx, second_pending) = oplog
        .add_start_with_reserved_payload(
            HostRequest::Custom("second".into_typed_schema_value().unwrap()),
            build_start(HostFunctionName::Custom("second".to_string())),
        )
        .await
        .unwrap();

    // The wrapper returns the leaf-assigned indices, in initiation order.
    assert_eq!(first_idx, OplogIndex::INITIAL);
    assert_eq!(second_idx, first_idx.next());
    first_pending.wait().await.unwrap();
    second_pending.wait().await.unwrap();

    // With max_commit_count = 1 the first commit triggers a flush to the plugin.
    oplog.commit(CommitLevel::Always).await.unwrap();

    let sends = recording_plugin.sends().await;
    assert_eq!(sends.len(), 1, "Expected exactly one batch");
    assert_eq!(sends[0].initial_oplog_index, first_idx);
    let function_names: Vec<_> = sends[0]
        .entries
        .iter()
        .map(|entry| match entry {
            OplogEntry::Start {
                function_name: HostFunctionName::Custom(name),
                ..
            } => name.clone(),
            other => panic!("unexpected forwarded entry: {other:?}"),
        })
        .collect();
    assert_eq!(
        function_names,
        vec!["first".to_string(), "second".to_string()]
    );
}

fn grow_memory(delta: u64) -> OplogEntry {
    OplogEntry::GrowMemory {
        timestamp: Timestamp::now_utc(),
        delta,
    }
}

fn memory_deltas(entries: &[OplogEntry]) -> Vec<u64> {
    entries
        .iter()
        .map(|entry| match entry {
            OplogEntry::GrowMemory { delta, .. } => *delta,
            other => panic!("unexpected entry: {other:?}"),
        })
        .collect()
}

fn publish_status(
    status_writer: &arc_swap::ArcSwap<AgentStatusRecord>,
    active_plugins: HashSet<EnvironmentPluginGrantId>,
    checkpoint: Option<(EnvironmentPluginGrantId, OplogProcessorCheckpointState)>,
) {
    let mut status = status_writer.load_full().as_ref().clone();
    status.active_plugins = active_plugins;
    status.oplog_processor_checkpoints = checkpoint.into_iter().collect();
    status_writer.store(Arc::new(status));
}

async fn test_forwarding_oplog(
    active_plugins: HashSet<EnvironmentPluginGrantId>,
    grant_id: EnvironmentPluginGrantId,
    max_commit_count: usize,
) -> (
    ForwardingOplog,
    Arc<InMemoryOplog>,
    Arc<RecordingOplogProcessorPlugin>,
    Arc<arc_swap::ArcSwap<AgentStatusRecord>>,
) {
    let (metadata, status_lock, status_writer) =
        test_worker_metadata_with_status_writer(active_plugins);
    let recording_plugin = Arc::new(RecordingOplogProcessorPlugin::new());
    let inner = Arc::new(InMemoryOplog::new());
    let oplog = ForwardingOplog::new(
        inner.clone(),
        recording_plugin.clone(),
        Arc::new(FakeComponentService::with_one_oplog_processor_plugin(
            grant_id,
        )),
        metadata,
        status_lock,
        OplogIndex::NONE,
        Box::new(|| {}),
        max_commit_count,
        Duration::from_secs(3600),
    )
    .await;
    (oplog, inner, recording_plugin, status_writer)
}

/// A report commits nothing, so it must not start the plugin flush either: that flush commits the
/// buffered entries of the worker. A commit count of zero puts every commit over the limit, as a
/// flush that skipped its reset leaves it.
#[test]
async fn report_only_commit_does_not_start_a_plugin_flush() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, inner, recording_plugin, _status_writer) =
        test_forwarding_oplog(HashSet::from([grant_id]), grant_id, 0).await;

    let index = oplog.add(grow_memory(1)).await.unwrap();
    assert!(
        oplog
            .commit(CommitLevel::ReportOnly)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(recording_plugin.sends().await.is_empty());

    let committed = inner.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(committed.keys().copied().collect::<Vec<_>>(), vec![index]);
}

#[test]
async fn no_plugin_mixed_appends_allocate_cache_only_after_activation() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, inner, recording_plugin, status_writer) =
        test_forwarding_oplog(HashSet::new(), grant_id, 1).await;

    oplog.add(grow_memory(1)).await.unwrap();
    oplog
        .add_pair(grow_memory(2), Box::new(|_| grow_memory(3)))
        .await
        .unwrap();
    oplog
        .add_start_with_reserved_raw_payload(Vec::new(), Box::new(|_| Ok(grow_memory(4))))
        .await
        .unwrap()
        .pending_upload
        .wait()
        .await
        .unwrap();
    oplog
        .add_start_with_indexed_reserved_raw_payload(Box::new(|_| {
            Ok((Vec::new(), Box::new(|_| Ok(grow_memory(5)))))
        }))
        .await
        .unwrap()
        .pending_upload
        .wait()
        .await
        .unwrap();
    oplog
        .add_durable_stream_batch(Box::new(|_| {
            vec![
                DurableStreamOplogRecord::InlineEntry(grow_memory(6)),
                DurableStreamOplogRecord::InlineEntry(grow_memory(7)),
            ]
        }))
        .await
        .unwrap();

    let uncached = oplog.inspect_state().await;
    assert_eq!(uncached.buffer_len, None);
    assert_eq!(uncached.last_oplog_idx, OplogIndex::from_u64(7));
    assert_eq!(uncached.buffer_start_idx, uncached.last_oplog_idx.next());

    publish_status(&status_writer, HashSet::from([grant_id]), None);
    oplog.add(grow_memory(8)).await.unwrap();
    let cached = oplog.inspect_state().await;
    assert_eq!(cached.buffer_len, Some(1));
    assert_eq!(cached.buffer_start_idx, OplogIndex::from_u64(8));
    assert_eq!(cached.last_oplog_idx, OplogIndex::from_u64(8));

    oplog.commit(CommitLevel::Always).await.unwrap();
    let sends = recording_plugin.sends().await;
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].initial_oplog_index, OplogIndex::INITIAL);
    assert_eq!(sends[0].entry_count, 8);
    assert_eq!(
        memory_deltas(&sends[0].entries),
        (1..=8).collect::<Vec<_>>()
    );
    assert_eq!(
        inner.read_exact_requests(),
        vec![(OplogIndex::INITIAL, 8)],
        "the whole range must fall back without splicing"
    );
}

#[test]
async fn measure_no_plugin_append_against_active_cache_control() {
    const APPENDS: u64 = 10_000;
    let grant_id = EnvironmentPluginGrantId::new();
    let (uncached, _inner, _plugin, _status) =
        test_forwarding_oplog(HashSet::new(), grant_id, usize::MAX).await;
    let started = std::time::Instant::now();
    for delta in 0..APPENDS {
        uncached.add(grow_memory(delta)).await.unwrap();
    }
    let uncached_elapsed = started.elapsed();
    assert_eq!(uncached.inspect_state().await.buffer_len, None);

    let (cached, _inner, _plugin, _status) =
        test_forwarding_oplog(HashSet::from([grant_id]), grant_id, usize::MAX).await;
    let started = std::time::Instant::now();
    for delta in 0..APPENDS {
        cached.add(grow_memory(delta)).await.unwrap();
    }
    let cached_elapsed = started.elapsed();
    assert_eq!(
        cached.inspect_state().await.buffer_len,
        Some(APPENDS as usize)
    );

    eprintln!(
        "forwarding oplog append ({APPENDS} entries): no plugin {uncached_elapsed:?} ({:.0} ns/op), active plugin {cached_elapsed:?} ({:.0} ns/op)",
        uncached_elapsed.as_nanos() as f64 / APPENDS as f64,
        cached_elapsed.as_nanos() as f64 / APPENDS as f64,
    );
}

#[test]
async fn reactivation_checkpoint_resumes_with_cached_suffix() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, inner, recording_plugin, status_writer) =
        test_forwarding_oplog(HashSet::new(), grant_id, 1).await;
    oplog.add(grow_memory(1)).await.unwrap();
    oplog.add(grow_memory(2)).await.unwrap();

    let target = recording_plugin.target_agent_id.clone();
    publish_status(
        &status_writer,
        HashSet::from([grant_id]),
        Some((
            grant_id,
            OplogProcessorCheckpointState {
                target_agent_id: Some(target),
                confirmed_up_to: OplogIndex::from_u64(2),
                sending_up_to: OplogIndex::from_u64(2),
                last_batch_start: OplogIndex::INITIAL,
            },
        )),
    );
    oplog.add(grow_memory(3)).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    let sends = recording_plugin.sends().await;
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].initial_oplog_index, OplogIndex::from_u64(3));
    assert_eq!(sends[0].entry_count, 1);
    assert_eq!(inner.read_exact_count(), 0, "suffix should be a cache hit");
}

#[test]
async fn cached_skipped_cached_range_uses_whole_range_fallback() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, inner, recording_plugin, status_writer) =
        test_forwarding_oplog(HashSet::from([grant_id]), grant_id, 1).await;
    oplog.add(grow_memory(1)).await.unwrap();
    publish_status(&status_writer, HashSet::new(), None);
    oplog.add(grow_memory(2)).await.unwrap();
    let skipped = oplog.inspect_state().await;
    assert_eq!(skipped.buffer_len, None);
    assert_eq!(skipped.buffer_start_idx, skipped.last_oplog_idx.next());

    publish_status(&status_writer, HashSet::from([grant_id]), None);
    oplog.add(grow_memory(3)).await.unwrap();
    let resumed = oplog.inspect_state().await;
    assert_eq!(resumed.buffer_len, Some(1));
    assert_eq!(resumed.buffer_start_idx, OplogIndex::from_u64(3));
    oplog.commit(CommitLevel::Always).await.unwrap();

    let sends = recording_plugin.sends().await;
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].initial_oplog_index, OplogIndex::INITIAL);
    assert_eq!(sends[0].entry_count, 3);
    assert_eq!(memory_deltas(&sends[0].entries), vec![1, 2, 3]);
    assert_eq!(inner.read_exact_requests(), vec![(OplogIndex::INITIAL, 3)]);
}

#[test]
async fn empty_uncached_batch_releases_cache_without_advancing_index() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, _inner, _recording_plugin, status_writer) =
        test_forwarding_oplog(HashSet::from([grant_id]), grant_id, usize::MAX).await;
    oplog.add(grow_memory(1)).await.unwrap();
    assert_eq!(oplog.inspect_state().await.buffer_len, Some(1));

    publish_status(&status_writer, HashSet::new(), None);
    assert!(
        oplog
            .add_durable_stream_batch(Box::new(|_| Vec::new()))
            .await
            .unwrap()
            .is_empty()
    );
    let state = oplog.inspect_state().await;
    assert_eq!(state.last_oplog_idx, OplogIndex::INITIAL);
    assert_eq!(state.buffer_len, None);
    assert_eq!(state.buffer_start_idx, state.last_oplog_idx.next());
}

#[test]
async fn failed_reserved_starts_do_not_advance_index_or_cache() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, _inner, _recording_plugin, _status_writer) =
        test_forwarding_oplog(HashSet::from([grant_id]), grant_id, usize::MAX).await;

    assert!(
        oplog
            .add_start_with_reserved_raw_payload(
                Vec::new(),
                Box::new(|_| Err("raw start failed".to_string())),
            )
            .await
            .is_err()
    );
    assert!(
        oplog
            .add_start_with_indexed_reserved_raw_payload(Box::new(|_| {
                Err("indexed start failed".to_string())
            }))
            .await
            .is_err()
    );

    let state = oplog.inspect_state().await;
    assert_eq!(state.last_oplog_idx, OplogIndex::NONE);
    assert_eq!(state.buffer_len, None);
    assert_eq!(state.buffer_start_idx, OplogIndex::INITIAL);
}

#[test]
async fn deactivated_outstanding_batch_retries_exact_range_and_target() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, _inner, recording_plugin, status_writer) =
        test_forwarding_oplog(HashSet::from([grant_id]), grant_id, 1).await;
    recording_plugin.fail_next_send_from_remote_target();
    oplog.add(grow_memory(1)).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();

    publish_status(&status_writer, HashSet::new(), None);
    oplog.add(grow_memory(2)).await.unwrap();
    assert!(oplog.inspect_state().await.buffer_len.is_some());
    oplog.commit(CommitLevel::Always).await.unwrap();

    let sends = recording_plugin.sends().await;
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[0].initial_oplog_index, OplogIndex::INITIAL);
    assert_eq!(sends[0].entry_count, 1);
    assert_eq!(sends[1].initial_oplog_index, OplogIndex::INITIAL);
    assert_eq!(sends[1].entry_count, 1);
    assert_eq!(sends[0].target_agent_id, sends[1].target_agent_id);
    assert_eq!(memory_deltas(&sends[0].entries), vec![1]);
    assert_eq!(memory_deltas(&sends[1].entries), vec![1]);
    assert_eq!(
        recording_plugin
            .resolve_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "the recorded remote target must be reused"
    );
}

#[test]
async fn direct_checkpoint_commits_are_returned_even_after_cache_pruning() {
    let grant_id = EnvironmentPluginGrantId::new();
    let (oplog, _inner, _recording_plugin, _status_writer) =
        test_forwarding_oplog(HashSet::from([grant_id]), grant_id, usize::MAX).await;
    oplog.add(grow_memory(1)).await.unwrap();
    let initial_commit = oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(initial_commit.len(), 1);

    oplog.tick().await;
    oplog.tick().await;
    let pruned = oplog.inspect_state().await;
    assert_eq!(pruned.buffer_len, None);
    assert_eq!(pruned.buffer_start_idx, pruned.last_oplog_idx.next());

    let checkpoint_commit = oplog.commit(CommitLevel::Always).await.unwrap();
    assert_eq!(
        checkpoint_commit
            .values()
            .filter(|entry| matches!(entry, OplogEntry::OplogProcessorCheckpoint { .. }))
            .count(),
        3
    );
    assert!(oplog.commit(CommitLevel::Always).await.unwrap().is_empty());
}
