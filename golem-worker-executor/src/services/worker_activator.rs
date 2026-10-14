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

use crate::services::HasAll;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::base_model::agent::Principal;
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{AgentFingerprint, AgentId, OwnedAgentId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, Weak};
use tracing::warn;

/// Service for activating workers in the background
#[async_trait]
pub trait WorkerActivator<Ctx: WorkerCtx>: Send + Sync {
    /// Returns the fingerprint of an active in-memory worker without touching persistent storage.
    async fn active_worker_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint>;

    /// Makes sure an already existing worker is active in a background task. Returns immediately.
    ///
    /// `Ok(())` means the worker is running, was already running, or no longer exists. `Err` means
    /// it could not be activated and still needs to be: callers driving a scheduled action must not
    /// acknowledge it, or the agent stays suspended with nothing left to wake it.
    async fn activate_worker(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError>;

    /// Gets or creates a worker in suspended state
    async fn get_or_create_suspended(
        &self,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>;

    /// Gets or creates a worker and starts it
    async fn get_or_create_running(
        &self,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError>;
}

pub struct LazyWorkerActivator<Ctx: WorkerCtx> {
    worker_activator: Arc<Mutex<Option<Weak<dyn WorkerActivator<Ctx> + 'static>>>>,
}

impl<Ctx: WorkerCtx> LazyWorkerActivator<Ctx> {
    pub fn new() -> Self {
        Self {
            worker_activator: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set(&self, worker_activator: Arc<dyn WorkerActivator<Ctx> + 'static>) {
        *self.worker_activator.lock().unwrap() = Some(Arc::downgrade(&worker_activator));
    }
}

impl<Ctx: WorkerCtx> Default for LazyWorkerActivator<Ctx> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> WorkerActivator<Ctx> for LazyWorkerActivator<Ctx> {
    async fn active_worker_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        let maybe_worker_activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade());
        match maybe_worker_activator {
            Some(worker_activator) => {
                worker_activator
                    .active_worker_fingerprint(owned_agent_id)
                    .await
            }
            None => None,
        }
    }

    async fn activate_worker(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        let maybe_worker_activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade());
        match maybe_worker_activator {
            Some(worker_activator) => worker_activator.activate_worker(owned_agent_id).await,
            None => Err(WorkerExecutorError::runtime(
                "WorkerActivator is disabled, not activating instance",
            )),
        }
    }

    async fn get_or_create_suspended(
        &self,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        let maybe_worker_activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade());
        match maybe_worker_activator {
            Some(worker_activator) => {
                worker_activator
                    .get_or_create_suspended(
                        owned_agent_id,
                        worker_env,
                        worker_agent_config,
                        component_revision,
                        parent,
                        invocation_context,
                        principal,
                    )
                    .await
            }
            None => Err(WorkerExecutorError::runtime(
                "WorkerActivator is disabled, not creating instance",
            )),
        }
    }

    async fn get_or_create_running(
        &self,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        let maybe_worker_activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade());
        match maybe_worker_activator {
            Some(worker_activator) => {
                worker_activator
                    .get_or_create_running(
                        owned_agent_id,
                        worker_env,
                        worker_agent_config,
                        component_revision,
                        parent,
                        invocation_context,
                        principal,
                    )
                    .await
            }
            None => Err(WorkerExecutorError::runtime(
                "WorkerActivator is disabled, not creating instance",
            )),
        }
    }
}

#[derive(Clone)]
pub struct DefaultWorkerActivator<Ctx: WorkerCtx, Svcs: HasAll<Ctx>> {
    all: Svcs,
    ctx: PhantomData<Ctx>,
}

impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx>> DefaultWorkerActivator<Ctx, Svcs> {
    pub fn new(all: Svcs) -> Self {
        Self {
            all,
            ctx: PhantomData,
        }
    }
}

#[async_trait]
impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + Send + Sync + 'static> WorkerActivator<Ctx>
    for DefaultWorkerActivator<Ctx, Svcs>
{
    async fn active_worker_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        self.all
            .active_workers()
            .try_get(owned_agent_id)
            .await
            .map(|worker| worker.get_initial_worker_metadata().fingerprint)
    }

    async fn activate_worker(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError> {
        if self
            .active_worker_fingerprint(owned_agent_id)
            .await
            .is_some()
        {
            return Ok(());
        }

        // A metadata read that *failed* is not evidence that the worker is gone, so it propagates:
        // only the two outcomes below are conclusive.
        match self.all.worker_service().get(owned_agent_id).await? {
            Some(_) => {
                Worker::get_or_create_running(
                    &self.all,
                    owned_agent_id,
                    None,
                    Vec::new(),
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                    Principal::anonymous(),
                )
                .await?;
                Ok(())
            }
            // No oplog: the worker was deleted. There is nothing to activate and no retry that
            // could bring it back, so this is a success rather than a failure to report upwards.
            None => {
                warn!("WorkerActivator::activate_worker: worker not found");
                Ok(())
            }
        }
    }

    async fn get_or_create_suspended(
        &self,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        Worker::get_or_create_suspended(
            &self.all,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context,
            principal,
        )
        .await
    }

    async fn get_or_create_running(
        &self,
        owned_agent_id: &OwnedAgentId,
        worker_env: Option<Vec<(String, String)>>,
        worker_agent_config: Vec<AgentConfigEntryDto>,
        component_revision: Option<ComponentRevision>,
        parent: Option<AgentId>,
        invocation_context: &InvocationContextStack,
        principal: Principal,
    ) -> Result<Arc<Worker<Ctx>>, WorkerExecutorError> {
        Worker::get_or_create_running(
            &self.all,
            owned_agent_id,
            worker_env,
            worker_agent_config,
            component_revision,
            parent,
            invocation_context,
            principal,
        )
        .await
    }
}
