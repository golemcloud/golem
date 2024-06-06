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

use std::cmp::max;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::WorkerId;

use crate::error::GolemError;
use crate::services::invocation_queue::InvocationQueue;
use crate::workerctx::WorkerCtx;

/// Holds the metadata and wasmtime structures of the active Golem workers
pub struct ActiveWorkers<Ctx: WorkerCtx> {
    cache: Cache<WorkerId, (), Arc<InvocationQueue<Ctx>>, GolemError>, // TODO: SimpleCache?
}

impl<Ctx: WorkerCtx> ActiveWorkers<Ctx> {
    pub fn bounded(max_active_workers: usize, drop_when_full: f64, ttl: Duration) -> Self {
        let drop_count = max(1, (max_active_workers as f64 * drop_when_full) as usize);
        ActiveWorkers {
            cache: Cache::new(
                Some(max_active_workers),
                FullCacheEvictionMode::LeastRecentlyUsed(drop_count),
                BackgroundEvictionMode::OlderThan {
                    ttl,
                    period: Duration::from_secs(60),
                },
                "active_workers",
            ),
        }
    }

    pub fn unbounded() -> Self {
        ActiveWorkers {
            cache: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "active_workers",
            ),
        }
    }

    pub async fn get_with<F>(
        &self,
        worker_id: WorkerId,
        f: F,
    ) -> Result<Arc<InvocationQueue<Ctx>>, GolemError>
    where
        F: FnOnce() -> Pin<
                Box<dyn Future<Output = Result<Arc<InvocationQueue<Ctx>>, GolemError>> + Send>,
            > + Send,
    {
        self.cache.get_or_insert_simple(&worker_id, f).await
    }

    pub fn remove(&self, worker_id: &WorkerId) {
        self.cache.remove(worker_id)
    }

    pub fn enum_workers(&self) -> Vec<(WorkerId, Arc<InvocationQueue<Ctx>>)> {
        self.cache.iter().collect()
    }
}
