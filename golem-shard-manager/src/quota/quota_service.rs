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

use super::resource_definition_fetcher::{FetchError, ResourceDefinitionFetcher};
use crate::shard_manager_config::QuotaServiceConfig;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::resource_definition::{
    ResourceDefinition, ResourceDefinitionId, ResourceName,
};
use golem_common::SafeDisplay;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::{debug, warn};

#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    #[error("Resource definition not found for id {0}")]
    ResourceDefinitionNotFoundById(ResourceDefinitionId),
    #[error("Resource definition '{name}' not found in environment {environment_id}")]
    ResourceDefinitionNotFoundByName {
        environment_id: EnvironmentId,
        name: ResourceName,
    },
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for QuotaError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ResourceDefinitionNotFoundById(_) => self.to_string(),
            Self::ResourceDefinitionNotFoundByName { .. } => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

golem_common::error_forwarding!(QuotaError, FetchError);

enum QuotaEntry {
    Live(LiveQuotaState),
    Tombstoned,
}

struct LiveQuotaState {
    definition: ResourceDefinition,
    last_refreshed: Instant,
    // TODO: leases, allocations, remaining capacity, epoch
}

impl LiveQuotaState {
    fn new(definition: ResourceDefinition) -> Self {
        Self {
            definition,
            last_refreshed: Instant::now(),
        }
    }

    fn update_definition(&mut self, definition: ResourceDefinition) {
        debug_assert_eq!(self.definition.id, definition.id);
        debug_assert_eq!(self.definition.environment_id, definition.environment_id);
        // TODO: reconcile lease state against new limits
        self.definition = definition;
        self.last_refreshed = Instant::now();
    }

    fn is_stale(&self, ttl: Duration) -> bool {
        self.last_refreshed.elapsed() > ttl
    }
}

type DefinitionCache = Cache<(EnvironmentId, ResourceName), (), ResourceDefinition, FetchError>;
type EntryHandle = Arc<RwLock<QuotaEntry>>;

pub struct QuotaService {
    entries: scc::HashMap<ResourceDefinitionId, EntryHandle>,
    definition_cache: DefinitionCache,
    fetcher: Arc<dyn ResourceDefinitionFetcher>,
    ttl: Duration,
}

impl QuotaService {
    pub fn new(
        config: QuotaServiceConfig,
        fetcher: Arc<dyn ResourceDefinitionFetcher>,
    ) -> Arc<Self> {
        let definition_cache = Cache::new(
            Some(config.definition_cache_max_capacity),
            FullCacheEvictionMode::LeastRecentlyUsed(1),
            BackgroundEvictionMode::OlderThan {
                ttl: config.definition_cache_ttl,
                period: config.definition_cache_eviction_period,
            },
            "quota_resource_definitions",
        );
        Arc::new(Self {
            entries: scc::HashMap::new(),
            definition_cache,
            fetcher,
            ttl: config.definition_cache_ttl,
        })
    }

    /// Returns the definition for a known id, refreshing if stale.
    /// Returns NotFound for tombstoned or unknown ids.
    #[allow(unused)]
    pub async fn get_by_id(
        &self,
        id: ResourceDefinitionId,
    ) -> Result<ResourceDefinition, QuotaError> {
        let handle = self
            .get_entry_handle(id)
            .await
            .ok_or(QuotaError::ResourceDefinitionNotFoundById(id))?;

        {
            let entry = handle.read().await;
            match &*entry {
                QuotaEntry::Live(live) if !live.is_stale(self.ttl) => {
                    return Ok(live.definition.clone());
                }
                QuotaEntry::Live(_) => {
                    // Stale — drop lock and refresh below.
                }
                QuotaEntry::Tombstoned => {
                    return Err(QuotaError::ResourceDefinitionNotFoundById(id));
                }
            }
        }

        self.refresh_entry(id).await;

        let entry = handle.read().await;
        match &*entry {
            QuotaEntry::Live(live) => Ok(live.definition.clone()),
            QuotaEntry::Tombstoned => Err(QuotaError::ResourceDefinitionNotFoundById(id)),
        }
    }

    /// Resolves a name to a definition, fetching from the registry if not cached.
    /// The returned definition's id can be used with `get_by_id` for subsequent accesses.
    #[allow(unused)]
    pub async fn get_or_fetch(
        &self,
        environment_id: EnvironmentId,
        name: ResourceName,
    ) -> Result<ResourceDefinition, QuotaError> {
        let key = (environment_id, name.clone());
        let fetcher = self.fetcher.clone();

        let definition = self
            .definition_cache
            .get_or_insert_simple(&key, async || {
                fetcher.get_by_name(environment_id, name.clone()).await
            })
            .await
            .map_err(|err| match err {
                FetchError::NotFound => QuotaError::ResourceDefinitionNotFoundByName {
                    environment_id,
                    name,
                },
                other => other.into(),
            })?;

        self.ensure_entry(&definition).await;
        self.get_by_id(definition.id).await.map_err(|_| {
            QuotaError::ResourceDefinitionNotFoundByName {
                environment_id: definition.environment_id,
                name: definition.name,
            }
        })
    }

    pub async fn on_resource_definition_changed(
        &self,
        environment_id: EnvironmentId,
        resource_definition_id: ResourceDefinitionId,
        resource_name: ResourceName,
    ) {
        self.definition_cache
            .remove(&(environment_id, resource_name.clone()))
            .await;

        if let Some(handle) = self.get_entry_handle(resource_definition_id).await {
            let is_live = {
                let entry = handle.read().await;
                matches!(&*entry, QuotaEntry::Live(_))
            };
            if is_live {
                self.refresh_entry(resource_definition_id).await;
            }
        } else {
            debug!(
                %environment_id,
                %resource_definition_id,
                %resource_name,
                "resource definition changed but not cached or tombstoned, ignoring"
            );
        }
    }

    pub async fn on_cursor_expired(&self) {
        for key in self.definition_cache.keys().await {
            self.definition_cache.remove(&key).await;
        }

        let mut live_ids = Vec::new();
        self.entries
            .iter_async(|id, handle| {
                // iter_async closure is sync — use try_read to avoid blocking.
                if let Ok(entry) = handle.try_read() {
                    if matches!(&*entry, QuotaEntry::Live(_)) {
                        live_ids.push(*id);
                    }
                } else {
                    // Lock contended — include it to be safe.
                    live_ids.push(*id);
                }
                true
            })
            .await;

        for id in live_ids {
            self.refresh_entry(id).await;
        }
    }

    async fn get_entry_handle(&self, id: ResourceDefinitionId) -> Option<EntryHandle> {
        self.entries
            .read_async(&id, |_, handle| handle.clone())
            .await
    }

    /// Ensures an entry exists for this definition.
    /// Does not overwrite existing entries (preserving runtime state).
    async fn ensure_entry(&self, definition: &ResourceDefinition) {
        let _ = self
            .entries
            .entry_async(definition.id)
            .await
            .or_insert_with(|| {
                Arc::new(RwLock::new(QuotaEntry::Live(LiveQuotaState::new(
                    definition.clone(),
                ))))
            });
    }

    async fn refresh_entry(&self, id: ResourceDefinitionId) {
        let handle = match self.get_entry_handle(id).await {
            Some(h) => h,
            None => return,
        };

        match self.fetcher.get_by_id(id).await {
            Ok(definition) => {
                debug_assert_eq!(definition.id, id);
                let mut entry = handle.write().await;
                if let QuotaEntry::Live(live) = &mut *entry {
                    live.update_definition(definition);
                }
            }
            Err(FetchError::NotFound) => {
                debug!(%id, "resource definition no longer exists, tombstoning");
                let mut entry = handle.write().await;
                *entry = QuotaEntry::Tombstoned;
            }
            Err(err) => {
                warn!(%id, error = %err, "failed to refresh resource definition, keeping stale entry");
            }
        }
    }
}
