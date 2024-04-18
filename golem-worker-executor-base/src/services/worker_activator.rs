// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use golem_common::model::WorkerId;
#[cfg(any(feature = "mocks", test))]
use tracing::info;
use tracing::{error, warn};

use crate::services::HasAll;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;

/// Service for activating workers in the background
#[async_trait]
pub trait WorkerActivator {
    /// Makes sure an already existing worker is active in a background task. Returns immediately
    async fn activate_worker(&self, worker_id: &WorkerId);
}

pub struct LazyWorkerActivator {
    worker_activator: Arc<Mutex<Option<Arc<dyn WorkerActivator + Send + Sync + 'static>>>>,
}

impl LazyWorkerActivator {
    pub fn new() -> Self {
        Self {
            worker_activator: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set(&self, worker_activator: Arc<impl WorkerActivator + Send + Sync + 'static>) {
        *self.worker_activator.lock().unwrap() = Some(worker_activator);
    }
}

impl Default for LazyWorkerActivator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WorkerActivator for LazyWorkerActivator {
    async fn activate_worker(&self, worker_id: &WorkerId) {
        let maybe_worker_activator = self.worker_activator.lock().unwrap().clone();
        match maybe_worker_activator {
            Some(worker_activator) => worker_activator.activate_worker(worker_id).await,
            None => warn!("WorkerActivator is disabled, not activating instance: {worker_id}"),
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
impl<Ctx: WorkerCtx, Svcs: HasAll<Ctx> + Send + Sync + 'static> WorkerActivator
    for DefaultWorkerActivator<Ctx, Svcs>
{
    async fn activate_worker(&self, worker_id: &WorkerId) {
        let metadata = self.all.worker_service().get(worker_id).await;
        match metadata {
            Some(metadata) => {
                Worker::activate(
                    &self.all,
                    worker_id,
                    metadata.args,
                    metadata.env,
                    Some(metadata.last_known_status.component_version),
                    metadata.account_id,
                )
                .await
            }
            None => {
                error!("WorkerActivator::activate_worker: worker {worker_id} not found")
            }
        }
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct WorkerActivatorMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for WorkerActivatorMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl WorkerActivatorMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl WorkerActivator for WorkerActivatorMock {
    async fn activate_worker(&self, worker_id: &WorkerId) {
        info!("WorkerActivatorMock::activate_worker {worker_id}");
    }
}
