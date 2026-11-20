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

use crate::clients::registry::{RegistryService, RegistryServiceError};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::agent::RegisteredAgentTypeImplementer;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::type_id::{TypeId, TypeIdKind};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

type Key = (EnvironmentId, TypeId);

#[derive(Debug, Clone)]
enum NameLookupError {
    Unassigned,
    Registry(RegistryServiceError),
}

/// Invalidation generations of cached values that a deployment makes stale.
///
/// A fetch records [`Self::current`] before its registry call. A fetched value is valid only
/// while [`Self::is_current`] holds for it, so a fetch that is in flight during an invalidation
/// of its environment is dropped and repeated instead of keeping the old value.
#[derive(Default)]
pub struct InvalidationGenerations {
    generation: AtomicU64,
    invalidated_at: Mutex<HashMap<EnvironmentId, u64>>,
    all_invalidated_at: AtomicU64,
}

impl InvalidationGenerations {
    /// The generation to record before a registry call.
    pub fn current(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Invalidates the environment's values and returns the new generation. Cached values of
    /// the environment with an older generation are stale.
    pub fn invalidate(&self, environment_id: EnvironmentId) -> u64 {
        let mut invalidated_at = self.invalidated_at.lock().unwrap();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        invalidated_at.insert(environment_id, generation);
        generation
    }

    /// Invalidates the values of every environment and returns the new generation.
    pub fn invalidate_all(&self) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.all_invalidated_at
            .fetch_max(generation, Ordering::SeqCst);
        generation
    }

    /// Whether a value of the environment fetched at `generation` is still valid.
    pub fn is_current(&self, environment_id: EnvironmentId, generation: u64) -> bool {
        let environment = self
            .invalidated_at
            .lock()
            .unwrap()
            .get(&environment_id)
            .copied()
            .unwrap_or(0);
        generation >= environment.max(self.all_invalidated_at.load(Ordering::SeqCst))
    }
}

/// Resolves the type ID in an agent ID's tag through the registry.
///
/// The kind and name of a type ID never change, so they are cached without invalidation. A
/// type ID that the environment has not assigned yet is never cached, because a later
/// deployment can assign it. The implementing component changes with each deployment, so it is
/// cached until [`Self::invalidate_environment`] is called for its environment.
///
/// Each cached implementer records its [`InvalidationGenerations`] generation, so a lookup that
/// is in flight during an invalidation never keeps the old implementer. Registry calls run in
/// their own task, so a cancelled caller never leaves a pending entry behind.
pub struct TypeIdResolutionCache {
    registry_service: Arc<dyn RegistryService>,
    names: Cache<Key, (), Arc<(TypeIdKind, String)>, NameLookupError>,
    implementers: Cache<Key, (), FetchedImplementer, RegistryServiceError>,
    generations: Arc<InvalidationGenerations>,
}

#[derive(Clone)]
struct FetchedImplementer {
    generation: u64,
    implementer: Option<RegisteredAgentTypeImplementer>,
}

impl TypeIdResolutionCache {
    pub fn new(registry_service: Arc<dyn RegistryService>, capacity: usize) -> Self {
        Self {
            registry_service,
            names: Cache::new(
                Some(capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "type_id_names",
            ),
            implementers: Cache::new(
                Some(capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::None,
                "type_id_implementers",
            ),
            generations: Arc::default(),
        }
    }

    /// The kind and name of the type ID, or `None` when the environment never assigned it.
    pub async fn resolve(
        &self,
        environment_id: EnvironmentId,
        type_id: TypeId,
    ) -> Result<Option<Arc<(TypeIdKind, String)>>, RegistryServiceError> {
        let registry_service = self.registry_service.clone();
        let result = self
            .names
            .get_or_insert_simple_spawned(&(environment_id, type_id), move || async move {
                match registry_service
                    .resolve_type_id(environment_id, type_id)
                    .await
                {
                    Ok(Some(resolved)) => Ok(Arc::new((resolved.kind, resolved.name))),
                    Ok(None) => Err(NameLookupError::Unassigned),
                    Err(error) => Err(NameLookupError::Registry(error)),
                }
            })
            .await;
        match result {
            Ok(names) => Ok(Some(names)),
            Err(NameLookupError::Unassigned) => Ok(None),
            Err(NameLookupError::Registry(error)) => Err(error),
        }
    }

    /// The component that implements the agent type of the type ID in the current deployment,
    /// or `None` when it is not deployed or is no agent type.
    pub async fn deployed_implementer(
        &self,
        environment_id: EnvironmentId,
        type_id: TypeId,
    ) -> Result<Option<RegisteredAgentTypeImplementer>, RegistryServiceError> {
        let key = (environment_id, type_id);
        loop {
            let registry_service = self.registry_service.clone();
            let generations = self.generations.clone();
            let fetched = self
                .implementers
                .get_or_insert_simple_spawned(&key, move || async move {
                    let generation = generations.current();
                    let implementer = registry_service
                        .resolve_type_id(environment_id, type_id)
                        .await?
                        .and_then(|resolved| resolved.implemented_by);
                    Ok(FetchedImplementer {
                        generation,
                        implementer,
                    })
                })
                .await?;
            if self
                .generations
                .is_current(environment_id, fetched.generation)
            {
                return Ok(fetched.implementer);
            }
            self.implementers
                .remove_if_cached(&key, |cached| cached.generation == fetched.generation)
                .await;
        }
    }

    /// Drops the cached implementers of the environment's types, for example after a
    /// `DeploymentChanged` event. The cached names stay.
    pub async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        let generation = self.generations.invalidate(environment_id);
        for (key, _) in self.implementers.iter().await {
            if key.0 == environment_id {
                self.implementers
                    .remove_if_cached(&key, |cached| cached.generation < generation)
                    .await;
            }
        }
    }

    /// Drops the cached implementers of every environment, for example after missed
    /// invalidation events. The cached names stay.
    pub async fn invalidate_all(&self) {
        let generation = self.generations.invalidate_all();
        for (key, _) in self.implementers.iter().await {
            self.implementers
                .remove_if_cached(&key, |cached| cached.generation < generation)
                .await;
        }
    }
}

#[cfg(test)]
mod tests;
