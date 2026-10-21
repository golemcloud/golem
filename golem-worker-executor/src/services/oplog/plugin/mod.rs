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

use crate::model::ExecutionStatus;
use crate::model::event::InternalWorkerEvent;
use crate::services::activity::{ActivityGate, ActivityGuard};
use crate::services::component::ComponentService;
use crate::services::oplog::{
    CommitLevel, DurableStreamBatchBuilder, IndexedReservedStartBuilder, OpenOplogs, Oplog,
    OplogAddReceipt, OplogCloseCompletion, OplogConstructor, OplogError, OplogLifecycleGuard,
    OplogRefusal, OplogService, OrderedOplogStart, RawOplogPayloadDownloadError,
    ReservedRawStartBuilder, StagePublication, UnreadableOplog,
};
use crate::services::shard::ShardService;
use crate::services::worker_activator::WorkerActivator;
use crate::services::worker_event::WorkerEventService;
use crate::services::worker_proxy::WorkerProxy;
use crate::services::{
    HasComponentService, HasOplogProcessorPlugin, HasShardService, HasWorkerActivator,
};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_lock::{RwLock, RwLockUpgradableReadGuard};
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::model::ShardEpoch;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMode, ParsedAgentId, Principal};
use golem_common::model::component::{ComponentId, ComponentRevision, InstalledPlugin};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{OplogEntry, OplogIndex, PayloadId, RawOplogPayload};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::model::{
    AgentId, AgentInvocation, AgentMetadata, AgentStatusRecord, IdempotencyKey, InvocationStatus,
    OwnedAgentId, ScanCursor, ShardId,
};
use golem_common::read_only_lock;
use golem_common::related_span;
use golem_common::tracing::TraceOrigin;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::{Debug, Formatter};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use uuid::{Uuid, uuid};

mod executor;
mod forwarding;
mod service;
mod state;

pub use executor::PerExecutorOplogProcessorPlugin;
pub use service::ForwardingOplogService;

#[cfg(test)]
mod tests;

#[cfg(test)]
impl ForwardingOplog {
    async fn inspect_state(&self) -> ForwardingStateSnapshot {
        self.run_job(|done| ForwardingJob::Inspect { done }).await
    }

    async fn tick(&self) {
        if self.jobs.send(ForwardingJob::Tick).is_err() {
            panic!("Forwarding oplog actor terminated unexpectedly");
        }
        self.inspect_state().await;
    }
}

/// Per-plugin live state tracked by `ForwardingOplogState` for exactly-once delivery.
/// Seeded from `AgentStatusRecord.oplog_processor_checkpoints` on construction,
/// updated synchronously after each checkpoint write.
#[derive(Clone, Debug)]
struct LivePluginState {
    target_agent_id: Option<AgentId>,
    confirmed_up_to: OplogIndex,
    sending_up_to: OplogIndex,
    send_in_progress: bool,
    last_batch_start: OplogIndex,
}

#[async_trait]
pub trait OplogProcessorPlugin: Send + Sync {
    /// Resolves or creates a target plugin worker for the given plugin, returning its AgentId.
    async fn resolve_target(
        &self,
        environment_id: EnvironmentId,
        plugin: &InstalledPlugin,
    ) -> Result<AgentId, WorkerExecutorError>;

    /// Enqueues oplog entries to the specified target plugin worker.
    /// Any `Ok(())` means durable delivery — the batch has been accepted.
    async fn send(
        &self,
        worker_metadata: AgentMetadata,
        plugin: &InstalledPlugin,
        target_agent_id: &AgentId,
        initial_oplog_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), WorkerExecutorError>;

    /// Evicts the cached plugin worker for the given plugin, forcing a fresh
    /// instance to be created on the next `resolve_target` call.
    async fn invalidate_target(&self, environment_id: EnvironmentId, plugin: &InstalledPlugin);

    async fn on_shard_assignment_changed(&self) -> Result<(), WorkerExecutorError>;

    async fn is_local(&self, agent_id: &AgentId) -> Result<bool, WorkerExecutorError>;

    async fn lookup_invocation_status(
        &self,
        environment_id: EnvironmentId,
        target_agent_id: &AgentId,
        caller_account_id: AccountId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationStatus, WorkerExecutorError>;
}

/// A wrapper for `Oplog` that periodically sends buffered oplog entries to oplog processor plugins
///
/// Structured as an actor for the same reason as `PrimaryOplog` (see its doc comment): this
/// wrapper is called both from futures polled by wasmtime's store event loop and from host code
/// running on store-keeping wasm fibers, and while such a fiber is suspended the event loop
/// cannot poll store-polled futures (wasmtime#11869/#11870). A shared async mutex held across the
/// inner-oplog delegation — or even fairly handed to a queued store-polled future — deadlocks the
/// store, so instead the state is exclusively owned by an independent actor task and callers only
/// await oneshot replies.
///
/// ORDERING: `add`, `add_pair` and `add_start_with_reserved_raw_payload` rely on the mirrored
/// `buffer`/`last_oplog_idx` staying in lockstep with the order the inner (leaf) oplog assigns
/// indices. The single-consumer job channel processes jobs in enqueue order, which is the same
/// FIFO guarantee the previous FIFO-fair mutex provided via `lock()` acquisition order.
pub struct ForwardingOplog {
    inner: Arc<dyn Oplog>,
    jobs: tokio::sync::mpsc::UnboundedSender<ForwardingJob>,
    /// Completion of the actor task; also used to wait for a
    /// cooperative shutdown without needing to take the `JoinHandle` out of a shared reference.
    closed: OplogCloseCompletion,
    retired: AtomicBool,
    timer: Option<JoinHandle<()>>,
    forwarding_retired: CancellationToken,
    close_fn: Mutex<Option<Box<dyn FnOnce() + Send + Sync>>>,
}

/// A request processed by the [`ForwardingOplog`] actor task, which exclusively owns the
/// [`ForwardingOplogState`].
enum ForwardingJob {
    /// Requests a graceful shutdown: sent by `Drop` and `retire`.
    /// Drains no further jobs after this one, so the actor exits once every job already queued
    /// ahead of it - including a stray `Tick` the timer enqueued in the instant before it was
    /// stopped - has been processed.
    Close,
    Add {
        entry: OplogEntry,
        done: tokio::sync::oneshot::Sender<Result<OplogIndex, OplogError>>,
    },
    AddDurableStreamBatch {
        make_batch: DurableStreamBatchBuilder,
        done: tokio::sync::oneshot::Sender<Result<Vec<(OplogIndex, OplogEntry)>, OplogError>>,
    },
    AddPair {
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
        done: tokio::sync::oneshot::Sender<Result<(OplogIndex, OplogIndex), OplogError>>,
    },
    AddStart {
        serialized_request: Vec<u8>,
        build_start: ReservedRawStartBuilder,
        done: tokio::sync::oneshot::Sender<Result<OrderedOplogStart, OplogError>>,
    },
    AddIndexedStart {
        build_request: IndexedReservedStartBuilder,
        done: tokio::sync::oneshot::Sender<Result<OrderedOplogStart, OplogError>>,
    },
    Commit {
        level: CommitLevel,
        done: tokio::sync::oneshot::Sender<Result<BTreeMap<OplogIndex, OplogEntry>, OplogError>>,
    },
    SetWorkerEventService {
        service: Arc<dyn WorkerEventService>,
        done: tokio::sync::oneshot::Sender<()>,
    },
    /// Periodic tick from the timer task: runs locality recovery and a time-based flush.
    Tick,
    Drain {
        done: tokio::sync::oneshot::Sender<()>,
    },
    #[cfg(test)]
    Inspect {
        done: tokio::sync::oneshot::Sender<ForwardingStateSnapshot>,
    },
}

#[cfg(test)]
#[derive(Debug)]
struct ForwardingStateSnapshot {
    buffer_len: Option<usize>,
    buffer_start_idx: OplogIndex,
    last_oplog_idx: OplogIndex,
}

struct ForwardingOplogState {
    forwarding_retired: CancellationToken,
    forwarding_activity: Arc<ActivityGate>,
    pending_checkpoint_activity: Option<ActivityGuard>,
    buffer: Option<VecDeque<OplogEntry>>,
    /// Oplog index corresponding to the first entry in `buffer`.
    buffer_start_idx: OplogIndex,
    commit_count: usize,
    last_send: Instant,
    oplog_plugins: Arc<dyn OplogProcessorPlugin>,
    initial_worker_metadata: AgentMetadata,
    last_known_status: read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord>,
    last_oplog_idx: OplogIndex,
    last_committed_idx: OplogIndex,
    components: Arc<dyn ComponentService>,
    inner: Arc<dyn Oplog>,
    plugin_state: HashMap<EnvironmentPluginGrantId, LivePluginState>,
    /// Entries committed directly to inner (bypassing `ForwardingOplog::commit()`),
    /// to be surfaced in the next `ForwardingOplog::commit()` return value
    /// so the Worker folds them into `AgentStatusRecord`.
    pending_direct_commits: BTreeMap<OplogIndex, OplogEntry>,
    worker_event_service: Option<Arc<dyn WorkerEventService>>,
    /// Abort-on-drop so that dropping this state (when the owning actor task is aborted) also
    /// stops all in-flight send-monitor tasks.
    monitor_tasks: Vec<wasmtime_wasi::runtime::AbortOnDropJoinHandle<()>>,
}

const OPLOG_PROC_NS: Uuid = uuid!("A7E3F1B2-8C4D-5E6F-9A0B-1C2D3E4F5A6B");

fn oplog_processor_idempotency_key(
    source_agent_id: &AgentId,
    plugin_installation_id: &EnvironmentPluginGrantId,
    batch_first_index: OplogIndex,
    batch_last_index: OplogIndex,
) -> IdempotencyKey {
    let mut buf = Vec::with_capacity(128);
    buf.extend_from_slice(b"oplog-proc-v1\0");
    buf.extend_from_slice(source_agent_id.component_id.0.as_bytes());
    let worker_name = source_agent_id.agent_id.as_bytes();
    buf.extend_from_slice(&(worker_name.len() as u32).to_be_bytes());
    buf.extend_from_slice(worker_name);
    buf.extend_from_slice(plugin_installation_id.0.as_bytes());
    buf.extend_from_slice(&batch_first_index.as_u64().to_be_bytes());
    buf.extend_from_slice(&batch_last_index.as_u64().to_be_bytes());
    IdempotencyKey::from_uuid(Uuid::new_v5(&OPLOG_PROC_NS, &buf))
}
