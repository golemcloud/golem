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
use crate::services::oplog::ArchiveWait;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::base_model::agent::{AgentMode, Principal};
use golem_common::model::component::ComponentRevision;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, OwnedAgentId,
};
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

    /// Whether `ActiveAgents` holds a constructed worker for the agent, loaded or not. One still
    /// being created does not count. Unlike [`Self::active_worker_fingerprint`] it leaves the
    /// entry's last access alone, so asking repeatedly does not keep an unloaded worker from
    /// expiring.
    async fn worker_is_cached(&self, owned_agent_id: &OwnedAgentId) -> bool;

    /// Makes sure an already existing worker is active in a background task. Returns immediately.
    ///
    /// `Ok(())` means the worker is running, was already running, or no longer exists. `Err` means
    /// it could not be activated and still needs to be: callers driving a scheduled action must not
    /// acknowledge it, or the agent stays suspended with nothing left to wake it.
    async fn activate_worker(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), WorkerExecutorError>;

    async fn archive_oplog(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: OplogIndex,
        wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError>;

    async fn expire_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: IdempotencyKey,
        expected_deadline_millis: u64,
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

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: AgentFingerprint,
        invocation: AgentInvocation,
    ) -> Result<bool, WorkerExecutorError>;

    async fn enqueue_ephemeral_external_tool(
        &self,
        owned_agent_id: &OwnedAgentId,
        component_revision: ComponentRevision,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError>;
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
    async fn archive_oplog(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: OplogIndex,
        wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        let activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade);
        match activator {
            Some(activator) => {
                activator
                    .archive_oplog(owned_agent_id, last_oplog_index, wait)
                    .await
            }
            None => Err(WorkerExecutorError::runtime(
                "WorkerActivator is disabled, not archiving oplog",
            )),
        }
    }

    async fn expire_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: IdempotencyKey,
        expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        let activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or_else(|| {
                WorkerExecutorError::runtime("WorkerActivator is disabled, not expiring session")
            })?;
        activator
            .expire_durable_stream_session(
                owned_agent_id,
                target_agent_fingerprint,
                public_session_id,
                session_key,
                expected_deadline_millis,
            )
            .await
    }

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

    async fn worker_is_cached(&self, owned_agent_id: &OwnedAgentId) -> bool {
        let maybe_worker_activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade());
        match maybe_worker_activator {
            Some(worker_activator) => worker_activator.worker_is_cached(owned_agent_id).await,
            None => false,
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

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: AgentFingerprint,
        invocation: AgentInvocation,
    ) -> Result<bool, WorkerExecutorError> {
        let activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| {
                WorkerExecutorError::runtime("WorkerActivator is disabled, not invoking instance")
            })?;
        activator
            .enqueue_exact_existing(owned_agent_id, expected_fingerprint, invocation)
            .await
    }

    async fn enqueue_ephemeral_external_tool(
        &self,
        owned_agent_id: &OwnedAgentId,
        component_revision: ComponentRevision,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        let activator = self
            .worker_activator
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| {
                WorkerExecutorError::runtime("WorkerActivator is disabled, not invoking instance")
            })?;
        activator
            .enqueue_ephemeral_external_tool(owned_agent_id, component_revision, invocation)
            .await
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
    async fn archive_oplog(
        &self,
        owned_agent_id: &OwnedAgentId,
        last_oplog_index: OplogIndex,
        wait: ArchiveWait,
    ) -> Result<Option<bool>, WorkerExecutorError> {
        match self
            .all
            .active_agents()
            .get_existing(&self.all, owned_agent_id, Principal::anonymous())
            .await
        {
            Ok(worker) => worker.archive_oplog(last_oplog_index, wait).await,
            Err(WorkerExecutorError::AgentNotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn expire_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: IdempotencyKey,
        expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        let worker = match self
            .all
            .active_agents()
            .get_existing(&self.all, owned_agent_id, Principal::anonymous())
            .await
        {
            Ok(worker) => worker,
            Err(WorkerExecutorError::AgentNotFound { .. }) => {
                let publication_pending = self
                    .all
                    .oplog_service()
                    .staged_exists(
                        owned_agent_id,
                        AgentMode::Durable,
                        target_agent_fingerprint.0,
                    )
                    .await
                    .map_err(WorkerExecutorError::runtime)?;
                if publication_pending {
                    return Err(WorkerExecutorError::runtime(
                        "durable stream target publication is still in progress",
                    ));
                }
                match self
                    .all
                    .active_agents()
                    .get_existing(&self.all, owned_agent_id, Principal::anonymous())
                    .await
                {
                    Ok(worker) => worker,
                    Err(WorkerExecutorError::AgentNotFound { .. }) => {
                        if self
                            .all
                            .oplog_service()
                            .exists(owned_agent_id, AgentMode::Durable)
                            .await
                        {
                            return Err(WorkerExecutorError::runtime(
                                "durable stream target was published during activation",
                            ));
                        }
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        if worker.get_initial_worker_metadata().fingerprint != target_agent_fingerprint {
            return Ok(());
        }
        worker
            .deliver_stream_session_expiry(public_session_id, session_key, expected_deadline_millis)
            .await
    }

    async fn active_worker_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Option<AgentFingerprint> {
        self.all
            .active_agents()
            .try_get(owned_agent_id)
            .await
            .map(|worker| worker.get_initial_worker_metadata().fingerprint)
    }

    async fn worker_is_cached(&self, owned_agent_id: &OwnedAgentId) -> bool {
        self.all
            .active_agents()
            .contains_cached_agent(owned_agent_id)
            .await
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

    async fn enqueue_exact_existing(
        &self,
        owned_agent_id: &OwnedAgentId,
        expected_fingerprint: AgentFingerprint,
        invocation: AgentInvocation,
    ) -> Result<bool, WorkerExecutorError> {
        let principal = invocation.principal().cloned().ok_or_else(|| {
            WorkerExecutorError::invalid_request("external tool invocation has no principal")
        })?;
        let worker = match Worker::get_exact_existing_suspended(
            &self.all,
            owned_agent_id,
            principal,
        )
        .await
        {
            Ok(worker) => worker,
            Err(WorkerExecutorError::AgentNotFound { .. }) => return Ok(false),
            Err(error) => return Err(error),
        };
        if worker.get_initial_worker_metadata().fingerprint != expected_fingerprint {
            return Ok(false);
        }
        worker.clone().invoke(invocation).await?;
        Worker::start_if_needed(worker).await?;
        Ok(true)
    }

    async fn enqueue_ephemeral_external_tool(
        &self,
        owned_agent_id: &OwnedAgentId,
        component_revision: ComponentRevision,
        invocation: AgentInvocation,
    ) -> Result<(), WorkerExecutorError> {
        let idempotency_key: IdempotencyKey =
            invocation.idempotency_key().cloned().ok_or_else(|| {
                WorkerExecutorError::invalid_request(
                    "external tool invocation has no idempotency key",
                )
            })?;
        let context = invocation.invocation_context();
        let principal = invocation.principal().cloned().ok_or_else(|| {
            WorkerExecutorError::invalid_request("external tool invocation has no principal")
        })?;
        let worker = self
            .all
            .active_agents()
            .get_or_add_ephemeral_external_tool_pinned(
                &self.all,
                owned_agent_id.agent_id.component_id,
                owned_agent_id.environment_id,
                &idempotency_key,
                component_revision,
                &context,
                principal,
            )
            .await?;
        if worker.owned_agent_id() != owned_agent_id {
            return Err(WorkerExecutorError::invalid_request(
                "scheduled external tool owner does not match its reserved identity",
            ));
        }
        worker.clone().invoke(invocation).await?;
        Worker::start_if_needed(worker).await?;
        Ok(())
    }
}
