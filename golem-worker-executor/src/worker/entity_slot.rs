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

use crate::services::linear_memory::LinearMemoryTracker;
use golem_common::model::entity::{
    AgentEntity, EntityActivationFingerprint, EntityInvocationId, EntityInvocationScope,
    ExecutableTarget, InvocationExecutionMode, OwnedAgentEntityId,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{HashMap, hash_map::Entry};
use std::sync::{Arc, Mutex};
use tokio::task::AbortHandle;

/// In-memory registry for one `(owner, entity)` pair.
///
/// A slot records every active invocation independently. It never grants execution and therefore
/// cannot serialize same-entity calls; scheduling belongs exclusively to the owner lane.
pub struct EntitySlot {
    entity_id: OwnedAgentEntityId,
    state: Mutex<EntitySlotState>,
}

struct EntitySlotState {
    accepting: bool,
    fence_generation: u64,
    active: HashMap<EntityInvocationId, ActiveEntityInvocation>,
}

struct ActiveEntityInvocation {
    activation_fingerprint: EntityActivationFingerprint,
    executable: ExecutableTarget,
    mode: InvocationExecutionMode,
    linear_memory: Option<LinearMemoryTracker>,
    abort: Option<AbortHandle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveEntityInvocationMetadata {
    pub invocation_id: EntityInvocationId,
    pub activation_fingerprint: EntityActivationFingerprint,
    pub executable: ExecutableTarget,
    pub mode: InvocationExecutionMode,
    pub linear_memory_bytes: u64,
}

impl EntitySlot {
    pub fn new(entity_id: OwnedAgentEntityId) -> Self {
        Self {
            entity_id,
            state: Mutex::new(EntitySlotState {
                accepting: true,
                fence_generation: 0,
                active: HashMap::new(),
            }),
        }
    }

    pub fn entity_id(&self) -> &OwnedAgentEntityId {
        &self.entity_id
    }

    pub fn entity(&self) -> &AgentEntity {
        &self.entity_id.entity
    }

    pub fn active_invocations(&self) -> Vec<ActiveEntityInvocationMetadata> {
        let state = self.state.lock().unwrap();
        let mut active = state
            .active
            .iter()
            .map(
                |(invocation_id, invocation)| ActiveEntityInvocationMetadata {
                    invocation_id: invocation_id.clone(),
                    activation_fingerprint: invocation.activation_fingerprint,
                    executable: invocation.executable.clone(),
                    mode: invocation.mode,
                    linear_memory_bytes: invocation
                        .linear_memory
                        .as_ref()
                        .map(LinearMemoryTracker::current_bytes)
                        .unwrap_or_default(),
                },
            )
            .collect::<Vec<_>>();
        active.sort_by_key(|invocation| invocation.invocation_id.start_index());
        active
    }

    pub fn active_invocation_count(&self) -> usize {
        self.state.lock().unwrap().active.len()
    }

    pub fn charged_linear_memory_bytes(&self) -> u64 {
        self.state
            .lock()
            .unwrap()
            .active
            .values()
            .filter_map(|invocation| invocation.linear_memory.as_ref())
            .map(LinearMemoryTracker::current_bytes)
            .fold(0, u64::saturating_add)
    }

    pub fn is_accepting(&self) -> bool {
        self.state.lock().unwrap().accepting
    }

    /// Closes admission and returns the invocations a lifecycle operation must drain or interrupt.
    pub fn fence(&self) -> Vec<EntityInvocationId> {
        let mut state = self.state.lock().unwrap();
        state.accepting = false;
        state.fence_generation = state.fence_generation.wrapping_add(1);
        let mut active = state.active.keys().cloned().collect::<Vec<_>>();
        for invocation in state.active.values() {
            if let Some(abort) = &invocation.abort {
                abort.abort();
            }
        }
        active.sort_by_key(EntityInvocationId::start_index);
        active
    }

    /// Reopens admission after the owner lifecycle has established a new active generation.
    pub fn reopen(&self) {
        let mut state = self.state.lock().unwrap();
        state.fence_generation = state.fence_generation.wrapping_add(1);
        state.accepting = true;
    }

    pub(crate) fn register(
        self: &Arc<Self>,
        scope: &EntityInvocationScope,
    ) -> Result<EntitySlotRegistration, WorkerExecutorError> {
        if scope.invocation_id().entity_id() != &self.entity_id {
            return Err(WorkerExecutorError::runtime(format!(
                "Entity invocation {} does not belong to slot {}",
                scope.invocation_id(),
                self.entity_id
            )));
        }

        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return Err(WorkerExecutorError::runtime(format!(
                "Entity slot {} is fenced by owner lifecycle",
                self.entity_id
            )));
        }
        let invocation_id = scope.invocation_id().clone();
        let invocation = ActiveEntityInvocation {
            activation_fingerprint: scope.activation().fingerprint(),
            executable: scope.activation().executable().clone(),
            mode: scope.mode(),
            linear_memory: None,
            abort: None,
        };
        match state.active.entry(invocation_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(invocation);
            }
            Entry::Occupied(_) => {
                return Err(WorkerExecutorError::runtime(format!(
                    "Entity invocation {invocation_id} is already registered"
                )));
            }
        }

        Ok(EntitySlotRegistration {
            slot: self.clone(),
            invocation_id: Some(invocation_id),
        })
    }

    pub(crate) fn attach_abort(
        &self,
        invocation_id: &EntityInvocationId,
        abort: AbortHandle,
    ) -> Result<(), WorkerExecutorError> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return Err(WorkerExecutorError::runtime(format!(
                "Entity slot {} was fenced before invocation {invocation_id} started",
                self.entity_id
            )));
        }
        let invocation = state.active.get_mut(invocation_id).ok_or_else(|| {
            WorkerExecutorError::runtime(format!(
                "Entity invocation {invocation_id} is no longer registered"
            ))
        })?;
        invocation.abort = Some(abort);
        Ok(())
    }
}

pub(crate) struct EntitySlotRegistration {
    slot: Arc<EntitySlot>,
    invocation_id: Option<EntityInvocationId>,
}

impl EntitySlotRegistration {
    pub(crate) fn attach_linear_memory(
        &self,
        linear_memory: LinearMemoryTracker,
    ) -> Result<(), WorkerExecutorError> {
        let invocation_id = self.invocation_id.as_ref().ok_or_else(|| {
            WorkerExecutorError::runtime("Entity slot registration is already closed")
        })?;
        let mut state = self.slot.state.lock().unwrap();
        let invocation = state.active.get_mut(invocation_id).ok_or_else(|| {
            WorkerExecutorError::runtime(format!(
                "Entity invocation {invocation_id} is no longer registered"
            ))
        })?;
        invocation.linear_memory = Some(linear_memory);
        Ok(())
    }
}

impl Drop for EntitySlotRegistration {
    fn drop(&mut self) {
        if let Some(invocation_id) = self.invocation_id.take() {
            self.slot
                .state
                .lock()
                .unwrap()
                .active
                .remove(&invocation_id);
        }
    }
}
