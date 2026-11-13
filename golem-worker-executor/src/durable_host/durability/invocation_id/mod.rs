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

use golem_common::model::oplog::{DurableInvocationId, OplogIndex};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::HashMap;
use std::sync::Mutex;
use wasmtime::component::RuntimeActivityId;

/// Why a guest import call is not identified by a durable invocation id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnidentifiedReason {
    /// The import was made while no agent invocation or entity body was running.
    OutsideInvocation,
    /// The import was made while a snapshot was saved or loaded. Snapshotting records no
    /// durable calls.
    Snapshotting,
    /// The call is made by the host code that drives the Store (the root of a
    /// `run_concurrent`), not by a guest import call.
    HostDriven,
}

/// The durable identity of one guest import call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuestImportIdentity {
    Id(DurableInvocationId),
    Unidentified(UnidentifiedReason),
}

impl GuestImportIdentity {
    pub(crate) fn invocation_id(self) -> Option<DurableInvocationId> {
        match self {
            Self::Id(invocation_id) => Some(invocation_id),
            Self::Unidentified(_) => None,
        }
    }
}

/// How a guest import call is numbered when it reserves its invocation id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportNumbering {
    /// Take the next ordinal under `anchor`: the `Start` of the custom durable invocation whose
    /// body makes the call, or `root` for a call outside one. `root` is the invocation anchor
    /// (see `DurableWorkerCtx::invocation_anchor`).
    Under {
        root: OplogIndex,
        anchor: OplogIndex,
    },
    Unidentified(UnidentifiedReason),
}

/// Numbers the durable guest import calls of one Store in guest call order, and gives each
/// durable call the invocation id of the import that makes it.
///
/// Wasmtime runs the Store's call hook (`CallHook::CallingHost`) and then reports the import's
/// runtime activity (`RuntimeActivityKind::Import`) synchronously, inside the guest's call, before
/// any host code of the import runs. The call hook decides the numbering from the calling guest
/// task and the durable context ([`Self::number_next_import`]); the activity report registers the
/// import with it ([`Self::import_started`]).
///
/// Wasmtime reports an activity for every import, including imports that make no durable call
/// and imports the guest makes a scheduling-dependent number of times (polls, custom invocation
/// context exchanges). So an import takes its ordinal only when it reserves it
/// ([`Self::reserve_import_identity`]): at its first durable call, or at entry for a host
/// function that `await`s before its first durable call. The reservation must happen while the
/// import is the guest's newest import; a later reservation is refused, because its place in the
/// guest's call order is lost. The reserved ordinals then follow the guest's call order, which is
/// the same in the live run and in every replay.
///
/// Imports in the body of a custom durable invocation are numbered under that invocation's
/// `Start`, because replay of a completed custom invocation does not run its body.
#[derive(Default)]
pub(crate) struct GuestImportNumbering {
    state: Mutex<NumberingState>,
}

#[derive(Clone, Copy)]
enum ImportState {
    /// The import has not reserved an identity yet.
    Numbered(ImportNumbering),
    Identified(GuestImportIdentity),
}

#[derive(Default)]
struct NumberingState {
    /// The numbering decided by the call hook for the import that starts next.
    next: Option<ImportNumbering>,
    /// The guest's newest import: the only one that can still reserve in guest call order.
    latest: Option<RuntimeActivityId>,
    /// The root anchor the ordinals belong to. A new root starts a new agent invocation or
    /// entity body, and drops the ordinals of the previous one.
    root: Option<OplogIndex>,
    ordinals: HashMap<OplogIndex, u64>,
    imports: HashMap<RuntimeActivityId, ImportState>,
    /// The import whose host call is running inside the guest's call right now. A Store-holding
    /// host function runs to completion inside the guest's call, so this is its own import.
    in_host_call: Option<RuntimeActivityId>,
}

impl NumberingState {
    fn next_id(&mut self, root: OplogIndex, anchor: OplogIndex) -> DurableInvocationId {
        if self.root != Some(root) {
            self.root = Some(root);
            self.ordinals.clear();
        }
        let next = self.ordinals.entry(anchor).or_default();
        let ordinal = *next;
        *next += 1;
        DurableInvocationId::new(anchor, ordinal)
    }

    fn reserve(
        &mut self,
        activity: RuntimeActivityId,
    ) -> Result<GuestImportIdentity, WorkerExecutorError> {
        match self.imports.get(&activity).copied() {
            None => Err(WorkerExecutorError::runtime(
                "a durable call was made from a runtime activity that is not a guest import; \
                 the task that makes it must be given the invocation id of the guest import \
                 that started it",
            )),
            Some(ImportState::Identified(identity)) => Ok(identity),
            Some(ImportState::Numbered(ImportNumbering::Unidentified(reason))) => {
                let identity = GuestImportIdentity::Unidentified(reason);
                self.imports
                    .insert(activity, ImportState::Identified(identity));
                Ok(identity)
            }
            Some(ImportState::Numbered(ImportNumbering::Under { root, anchor })) => {
                if self.latest != Some(activity) {
                    return Err(WorkerExecutorError::runtime(
                        "a guest import reserved its durable invocation id after the guest made \
                         another import call; a host function that awaits before its first \
                         durable call must reserve its invocation id at entry",
                    ));
                }
                let identity = GuestImportIdentity::Id(self.next_id(root, anchor));
                self.imports
                    .insert(activity, ImportState::Identified(identity));
                Ok(identity)
            }
        }
    }
}

impl GuestImportNumbering {
    /// Called from the Store's call hook on `CallHook::CallingHost`.
    pub(crate) fn number_next_import(&self, numbering: ImportNumbering) {
        self.state.lock().unwrap().next = Some(numbering);
    }

    /// Called from the Store's call hook on `CallHook::ReturningFromHost`.
    pub(crate) fn host_call_returned(&self) {
        self.state.lock().unwrap().in_host_call = None;
    }

    /// Called from the runtime observer when a guest import activity starts.
    pub(crate) fn import_started(&self, activity: RuntimeActivityId) {
        let mut state = self.state.lock().unwrap();
        state.latest = Some(activity);
        let Some(numbering) = state.next.take() else {
            return;
        };
        state
            .imports
            .insert(activity, ImportState::Numbered(numbering));
        state.in_host_call = Some(activity);
    }

    /// Called from the runtime observer when the future that drives the Store starts. Durable
    /// calls that host code makes from it are not guest calls.
    pub(crate) fn driver_started(&self, activity: RuntimeActivityId) {
        self.state.lock().unwrap().imports.insert(
            activity,
            ImportState::Identified(GuestImportIdentity::Unidentified(
                UnidentifiedReason::HostDriven,
            )),
        );
    }

    /// Called from the runtime observer when an activity finishes.
    pub(crate) fn activity_finished(&self, activity: RuntimeActivityId) {
        let mut state = self.state.lock().unwrap();
        state.imports.remove(&activity);
        if state.in_host_call == Some(activity) {
            state.in_host_call = None;
        }
    }

    /// The identity of the guest import `activity`, reserving its ordinal on first use.
    ///
    /// Fails for an activity that is not a guest import, such as a task the import spawned (such
    /// a task must be given the identity of the import that spawned it), and for an import that
    /// reserves after the guest made another import call.
    pub(crate) fn reserve_import_identity(
        &self,
        activity: Option<RuntimeActivityId>,
    ) -> Result<GuestImportIdentity, WorkerExecutorError> {
        let mut state = self.state.lock().unwrap();
        match activity {
            Some(activity) => state.reserve(activity),
            None => Err(WorkerExecutorError::runtime(
                "a durable call was made outside any runtime activity",
            )),
        }
    }

    /// The identity of a durable call made by Store-holding host code.
    ///
    /// Inside a guest import call, this is the identity of that import. Host code that runs
    /// outside any guest import call, between guest calls, takes the next ordinal under `root`:
    /// it runs at the same point of the guest's call order in every run.
    pub(crate) fn store_holding_call_identity(
        &self,
        root: OplogIndex,
    ) -> Result<GuestImportIdentity, WorkerExecutorError> {
        let mut state = self.state.lock().unwrap();
        match state.in_host_call {
            Some(activity) if state.imports.contains_key(&activity) => state.reserve(activity),
            _ => Ok(GuestImportIdentity::Id(state.next_id(root, root))),
        }
    }
}

#[cfg(test)]
mod tests;
