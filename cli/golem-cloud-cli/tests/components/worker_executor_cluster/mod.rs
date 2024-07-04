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

use crate::components::worker_executor::WorkerExecutor;
use async_trait::async_trait;
use std::sync::Arc;

pub mod spawned;

#[async_trait]
pub trait WorkerExecutorCluster {
    fn size(&self) -> usize;
    fn kill_all(&self);
    async fn restart_all(&self);

    fn stop(&self, index: usize);
    async fn start(&self, index: usize);

    fn blocking_start(&self, index: usize) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move { self.start(index).await });
    }

    fn to_vec(&self) -> Vec<Arc<dyn WorkerExecutor + Send + Sync + 'static>>;

    fn stopped_indices(&self) -> Vec<usize>;
    fn started_indices(&self) -> Vec<usize>;
}
