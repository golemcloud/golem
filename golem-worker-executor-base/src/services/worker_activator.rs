// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::error::GolemError;
use crate::services::HasAll;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;
use async_trait::async_trait;
use golem_common::model::{OwnedWorkerId, WorkerId};
use tracing::{error, warn};

/// Service for activating workers in the background
#[async_trait]
pub trait WorkerActivator<Ctx: WorkerCtx>: Send + Sync {
    /// Makes sure an already existing worker is active in a background task. Returns immediately
    async fn activate_worker(&self, owned_worker_id: &OwnedWorkerId);

    /// Gets or creates a worker in suspended state
    async fn get_or_create_suspended(
        &self,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Worker<Ctx>>, GolemError>;

    /// Gets or creates a worker and starts it
    async fn get_or_create_running(
        &self,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Worker<Ctx>>, GolemError>;
}

pub struct LazyWorkerActivator<Ctx: WorkerCtx> {
    worker_activator: Arc<Mutex<Option<Arc<dyn WorkerActivator<Ctx> + 'static>>>>,
}

impl<Ctx: WorkerCtx> LazyWorkerActivator<Ctx> {
    pub fn new() -> Self {
        Self {
            worker_activator: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set(&self, worker_activator: Arc<impl WorkerActivator<Ctx> + 'static>) {
        *self.worker_activator.lock().unwrap() = Some(worker_activator);
    }
}

impl<Ctx: WorkerCtx> Default for LazyWorkerActivator<Ctx> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> WorkerActivator<Ctx> for LazyWorkerActivator<Ctx> {
    async fn activate_worker(&self, owned_worker_id: &OwnedWorkerId) {
        let maybe_worker_activator = self.worker_activator.lock().unwrap().clone();
        match maybe_worker_activator {
            Some(worker_activator) => worker_activator.activate_worker(owned_worker_id).await,
            None => warn!("WorkerActivator is disabled, not activating instance"),
        }
    }

    async fn get_or_create_suspended(
        &self,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        let maybe_worker_activator = self.worker_activator.lock().unwrap().clone();
        match maybe_worker_activator {
            Some(worker_activator) => {
                worker_activator
                    .get_or_create_suspended(
                        owned_worker_id,
                        worker_args,
                        worker_env,
                        component_version,
                        parent,
                    )
                    .await
            }
            None => Err(GolemError::runtime(
                "WorkerActivator is disabled, not creating instance",
            )),
        }
    }

    async fn get_or_create_running(
        &self,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        let maybe_worker_activator = self.worker_activator.lock().unwrap().clone();
        match maybe_worker_activator {
            Some(worker_activator) => {
                worker_activator
                    .get_or_create_running(
                        owned_worker_id,
                        worker_args,
                        worker_env,
                        component_version,
                        parent,
                    )
                    .await
            }
            None => Err(GolemError::runtime(
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
    async fn activate_worker(&self, owned_worker_id: &OwnedWorkerId) {
        let metadata = self.all.worker_service().get(owned_worker_id).await;
        match metadata {
            Some(_) => {
                if let Err(err) = Worker::get_or_create_running(
                    &self.all,
                    owned_worker_id,
                    None,
                    None,
                    None,
                    None,
                )
                .await
                {
                    error!("Failed to activate worker: {err}")
                }
            }
            None => {
                error!("WorkerActivator::activate_worker: worker not found")
            }
        }
    }

    async fn get_or_create_suspended(
        &self,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        Worker::get_or_create_suspended(
            &self.all,
            owned_worker_id,
            worker_args,
            worker_env,
            component_version,
            parent,
        )
        .await
    }

    async fn get_or_create_running(
        &self,
        owned_worker_id: &OwnedWorkerId,
        worker_args: Option<Vec<String>>,
        worker_env: Option<Vec<(String, String)>>,
        component_version: Option<u64>,
        parent: Option<WorkerId>,
    ) -> Result<Arc<Worker<Ctx>>, GolemError> {
        Worker::get_or_create_running(
            &self.all,
            owned_worker_id,
            worker_args,
            worker_env,
            component_version,
            parent,
        )
        .await
    }
}
