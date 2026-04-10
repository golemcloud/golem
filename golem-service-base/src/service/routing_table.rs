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

use crate::clients::shard_manager::{ShardManager, ShardManagerError};
use golem_common::SafeDisplay;
use golem_common::cache::*;
use golem_common::model::RoutingTable;
use serde::Deserialize;
use serde::Serialize;
use std::fmt::Write;
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoutingTableConfig {
    #[serde(with = "humantime_serde")]
    pub invalidation_min_delay: Duration,
}

impl SafeDisplay for RoutingTableConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "invalidation minimum delay: {:?}",
            self.invalidation_min_delay
        );
        result
    }
}

impl Default for RoutingTableConfig {
    fn default() -> Self {
        Self {
            invalidation_min_delay: Duration::from_millis(500),
        }
    }
}

pub trait HasRoutingTableService {
    fn routing_table_service(&self) -> &Arc<RoutingTableService>;
}

pub struct RoutingTableService {
    config: RoutingTableConfig,
    cache: Cache<(), (), RoutingTable, ShardManagerError>,
    last_invalidated_at: RwLock<Option<Instant>>,
    shard_manager: Arc<dyn ShardManager>,
}

impl RoutingTableService {
    pub fn new(config: RoutingTableConfig, shard_manager: Arc<dyn ShardManager>) -> Self {
        Self {
            config,
            cache: Cache::new(
                Some(1),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "routing_table",
            ),
            last_invalidated_at: RwLock::new(None),
            shard_manager,
        }
    }

    pub async fn get_routing_table(&self) -> Result<RoutingTable, ShardManagerError> {
        let shard_manager = self.shard_manager.clone();
        self.cache
            .get_or_insert_simple(&(), || {
                Box::pin(async move { shard_manager.get_routing_table().await })
            })
            .await
    }

    pub async fn try_invalidate_routing_table(&self) -> bool {
        let now = Instant::now();

        let skip_invalidate = |last_invalidated_at: &Option<Instant>| {
            matches!(
                last_invalidated_at,
                Some(last_invalidated_at)
                    if now.saturating_duration_since(last_invalidated_at.to_owned()) < self.config.invalidation_min_delay
            )
        };

        if skip_invalidate(self.last_invalidated_at.read().await.deref()) {
            return false;
        }

        let mut last_invalidated_at = self.last_invalidated_at.write().await;
        if skip_invalidate(last_invalidated_at.deref()) {
            return false;
        }
        self.cache.remove(&()).await;
        *last_invalidated_at = Some(Instant::now());
        true
    }
}
