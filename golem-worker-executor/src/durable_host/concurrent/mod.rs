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

//! Concurrent-replay core for durable host calls.
//!
//! A durable host call is identified by the [`OplogIndex`] of its `Start` entry. While live,
//! the call eagerly appends a `Start` (capturing its request) and later an `End` (its response)
//! or a `Cancelled`. During replay the [`ConcurrentReplayResolver`] matches each completed
//! `End`/`Cancelled` back to the awaiting [`DurableCallSession`] via a [`ReplayableOneshot`], so the two
//! halves of a call no longer have to be adjacent in the oplog — which is what lets us track
//! async, parallel host functions.
//!
//! Every durable host call runs through this path via [`DurableCallSession`]. Calls made through the
//! p3 `Accessor` entry points ([`DurableCallSession::start_access_with`] and friends) run concurrently;
//! host methods still taking `&mut self` remain serialized by the store borrow.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    OplogPayload, ScopeScanState, host_functions::HostFunctionName,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{RetryProperties, Timestamp};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use wasmtime::component::{Accessor, HasData, TerminalConsumption};

use crate::durable_host::durability::{
    ClassifiedHostError, CustomBeginLifecycle, CustomInvocationContext, CustomInvocationScope,
    DurabilityHost, DurableCallTrapContext, DurableCallTrapError, DurableExecutionState,
    HostFailureKind, InFunctionRetryController, InFunctionRetryHost, InternalRetryResult,
    TaskRetryContext, TerminalCallError, mark_durable_call_trap_context,
    try_trigger_host_trap_retry,
};
use crate::durable_host::durable_session::DroppedDurableInput;
use crate::durable_host::replay_state::{
    OplogEntryLookupResult, ReplayState, ReplayToLiveRole, ScopeStartClaimOutcome,
};
use crate::durable_host::{
    AtomicRegionLease, BeginReplayToLive, DurableScopeKind, DurableWorkerCtx, FinishReplayToLive,
    PendingReplayToLive, PublicDurableWorkerState,
};
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, PendingUpload};
use crate::services::{HasShutdownToken, HasWorker};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use std::fmt::Display;

mod access;
mod call;
mod delivery;
mod demand_stream;
mod drop_events;
mod replay;

pub(super) use super::call_coordinator::{
    DurableCallAdmission, DurableCallBoundary, DurableCallCoordinator,
    lock_synchronized_card_event_boundary_access, process_pending_replay_events_access,
};
pub(crate) use super::call_coordinator::{
    agent_auth_ctx_at_serialized_access, authorize_live_permissions_at_serialized_access,
    try_agent_auth_ctx_at_serialized_access,
};
use access::*;
pub use call::*;
#[cfg(test)]
use call::{
    BegunCallExecutionScope, CallExecutionScope, ScopedRetryHost, unregistered_atomic_lease,
};
pub use delivery::*;
pub(crate) use demand_stream::*;
pub use drop_events::*;
pub use replay::*;

#[cfg(test)]
mod tests;
