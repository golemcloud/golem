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

use async_trait::async_trait;
use golem_common::model::{ShardAssignment, ShardId};
use golem_service_base::clients::shard_manager::ShardManagerError;
use std::collections::HashSet;
use std::sync::Arc;

#[async_trait]
pub trait ShardManagerService: Send + Sync {
    async fn register(
        &self,
        port: u16,
        pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError>;
}

pub struct GrpcShardManagerService {
    client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
}

impl GrpcShardManagerService {
    pub fn new(client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl ShardManagerService for GrpcShardManagerService {
    async fn register(
        &self,
        port: u16,
        pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError> {
        let number_of_shards = self.client.register(port, pod_name).await?;
        Ok(ShardAssignment {
            number_of_shards: number_of_shards
                .try_into()
                .expect("Failed to convert number of shards to usize"),
            shard_ids: HashSet::new(),
        })
    }
}

/// Single-shard implementation for local development and the debugging
/// service.  Returns a single shard assignment without contacting a real
/// shard manager.
pub struct ShardManagerServiceSingleShard;

#[async_trait]
impl ShardManagerService for ShardManagerServiceSingleShard {
    async fn register(
        &self,
        _port: u16,
        _pod_name: Option<String>,
    ) -> Result<ShardAssignment, ShardManagerError> {
        Ok(ShardAssignment {
            number_of_shards: 1,
            shard_ids: HashSet::from_iter(vec![ShardId::new(0)]),
        })
    }
}
