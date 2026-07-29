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
//! `End`/`Cancelled` back to the awaiting [`CallHandle`] via a [`ReplayableOneshot`], so the two
//! halves of a call no longer have to be adjacent in the oplog — which is what lets us track
//! async, parallel host functions.
//!
//! Every durable host call runs through this path via [`CallHandle`]. Calls made through the
//! p3 `Accessor` entry points ([`CallHandle::start_access_with`] and friends) run concurrently;
//! host methods still taking `&mut self` remain serialized by the store borrow.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Error;
use async_trait::async_trait;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::UpdateDescription;
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse, OplogEntry, OplogIndex,
    OplogPayload, PersistenceLevel, ScopeScanState, host_functions::HostFunctionName,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{RetryProperties, Timestamp};
use golem_service_base::error::worker_executor::{
    GolemSpecificWasmTrap, InterruptKind, WorkerExecutorError,
};
use golem_service_base::model::component::Component;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use wasmtime::component::{Accessor, HasData, TerminalConsumption};

use crate::durable_host::durability::{
    ClassifiedHostError, DurabilityHost, DurableCallTrapContext, DurableCallTrapError,
    DurableExecutionState, HostFailureKind, InFunctionRetryController, InFunctionRetryHost,
    InternalRetryResult, TaskRetryContext, TerminalCallError, mark_durable_call_trap_context,
    try_trigger_host_trap_retry,
};
use crate::durable_host::replay_state::{OplogEntryLookupResult, ReplayState};
use crate::durable_host::{
    AtomicRegionLease, DurableScopeKind, DurableWorkerCtx, IFSWorkerFile, PublicDurableWorkerState,
};
use crate::services::HasWorker;
use crate::services::component::ComponentService;
use crate::services::file_loader::FileLoader;
use crate::services::oplog::{CommitLevel, Oplog, OplogOps, PendingUpload};
use crate::worker::agent_config::{effective_agent_config, validate_agent_config};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use std::fmt::Display;

mod access;
mod call;
mod delivery;
mod drop_events;
mod replay;

use access::*;
pub use call::*;
#[cfg(test)]
use call::{
    BegunCallExecutionScope, CallExecutionScope, ScopedRetryHost, unregistered_atomic_lease,
};
pub use delivery::*;
pub use drop_events::*;
pub use replay::*;

#[cfg(test)]
mod tests;
