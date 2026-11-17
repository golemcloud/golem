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

use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::model::AgentId;
use golem_common::model::agent::{
    AgentTypeName, RegisteredAgentType, RegisteredAgentTypeImplementer,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::type_id::TypeIdKind;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::service::type_id_resolution::{
    InvalidationGenerations, TypeIdResolutionCache,
};
use std::sync::Arc;
use std::time::Duration;

/// The agent type that an agent ID's type tag names.
#[derive(Clone, Debug)]
pub struct TaggedAgentType {
    pub name: AgentTypeName,
    /// The component that implements the type in the current deployment. `None` when the
    /// type is no longer deployed.
    pub implemented_by: Option<RegisteredAgentTypeImplementer>,
}

/// Resolves the agent types of an environment by name and by an agent ID's type tag. The
/// registry invalidation events drop what a deployment change makes stale; a lookup that is in
/// flight during an invalidation is repeated, so it never keeps the old deployment's answer.
pub struct AgentTypeResolver {
    registry_service: Arc<dyn RegistryService>,
    type_ids: TypeIdResolutionCache,
    deployed: Cache<(EnvironmentId, AgentTypeName), (), FetchedDeployedType, RegistryServiceError>,
    generations: Arc<InvalidationGenerations>,
}

#[derive(Clone)]
struct FetchedDeployedType {
    generation: u64,
    agent_type: Option<Arc<RegisteredAgentType>>,
}

impl AgentTypeResolver {
    pub fn new(
        registry_service: Arc<dyn RegistryService>,
        capacity: usize,
        ttl: Duration,
        eviction_period: Duration,
    ) -> Self {
        Self {
            type_ids: TypeIdResolutionCache::new(registry_service.clone(), capacity),
            deployed: Cache::new(
                Some(capacity),
                FullCacheEvictionMode::LeastRecentlyUsed(1),
                BackgroundEvictionMode::OlderThan {
                    ttl,
                    period: eviction_period,
                },
                "deployed_agent_types",
            ),
            generations: Arc::default(),
            registry_service,
        }
    }

    /// The agent type of the environment's current deployment that has the name.
    pub async fn deployed(
        &self,
        environment_id: EnvironmentId,
        name: &AgentTypeName,
    ) -> Result<Option<Arc<RegisteredAgentType>>, RegistryServiceError> {
        let key = (environment_id, name.clone());
        loop {
            let registry_service = self.registry_service.clone();
            let generations = self.generations.clone();
            let lookup_name = name.clone();
            let fetched = self
                .deployed
                .get_or_insert_simple_spawned(&key, move || async move {
                    let generation = generations.current();
                    let agent_type = registry_service
                        .get_deployed_agent_type(environment_id, &lookup_name)
                        .await?
                        .map(Arc::new);
                    Ok(FetchedDeployedType {
                        generation,
                        agent_type,
                    })
                })
                .await?;
            if self
                .generations
                .is_current(environment_id, fetched.generation)
            {
                return Ok(fetched.agent_type);
            }
            self.deployed
                .remove_if_cached(&key, |cached| cached.generation == fetched.generation)
                .await;
        }
    }

    /// The agent type that the agent ID's type tag names in the environment. `None` for the
    /// reserved tag, a tag that the environment never assigned, and a tool's tag.
    pub async fn tagged(
        &self,
        environment_id: EnvironmentId,
        agent_id: AgentId,
    ) -> Result<Option<TaggedAgentType>, RegistryServiceError> {
        let Some(type_id) = agent_id.type_id() else {
            return Ok(None);
        };
        let Some(resolved) = self.type_ids.resolve(environment_id, type_id).await? else {
            return Ok(None);
        };
        let (kind, name) = resolved.as_ref();
        if *kind != TypeIdKind::AgentType {
            return Ok(None);
        }
        let implemented_by = self
            .type_ids
            .deployed_implementer(environment_id, type_id)
            .await?;
        Ok(Some(TaggedAgentType {
            name: AgentTypeName(name.clone()),
            implemented_by,
        }))
    }

    /// Drops what a deployment of the environment makes stale. Type names stay, because a
    /// type ID never changes its name.
    pub async fn invalidate_environment(&self, environment_id: EnvironmentId) {
        self.type_ids.invalidate_environment(environment_id).await;
        let generation = self.generations.invalidate(environment_id);
        for (key, _) in self.deployed.iter().await {
            if key.0 == environment_id {
                self.deployed
                    .remove_if_cached(&key, |cached| cached.generation < generation)
                    .await;
            }
        }
    }

    /// Drops everything that a deployment can make stale, after missed invalidation events.
    pub async fn clear(&self) {
        self.type_ids.invalidate_all().await;
        let generation = self.generations.invalidate_all();
        for (key, _) in self.deployed.iter().await {
            self.deployed
                .remove_if_cached(&key, |cached| cached.generation < generation)
                .await;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::service::component::tests::{LookupGate, TestRegistry, component};
    use golem_common::model::agent::{AgentMode, Snapshotting};
    use golem_common::model::component::ComponentRevision;
    use golem_common::model::type_id::TypeId;
    use golem_common::schema::agent::AgentTypeKind;
    use golem_common::schema::{AgentConstructorSchema, AgentTypeSchema, InputSchema, SchemaGraph};
    use golem_schema::Empty;
    use test_r::test;

    pub(crate) fn registered(name: &AgentTypeName, revision: u64) -> RegisteredAgentType {
        let component = component();
        RegisteredAgentType {
            type_id: TypeId::try_from(42u32).unwrap(),
            agent_type: AgentTypeSchema {
                kind: AgentTypeKind::Regular,
                type_name: name.clone(),
                description: String::new(),
                source_language: String::new(),
                schema: SchemaGraph::empty(),
                constructor: AgentConstructorSchema {
                    name: None,
                    description: String::new(),
                    prompt_hint: None,
                    input_schema: InputSchema::Parameters(vec![]),
                },
                methods: vec![],
                dependencies: vec![],
                mode: AgentMode::Durable,
                http_mount: None,
                snapshotting: Snapshotting::Disabled(Empty {}),
                config: vec![],
            },
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: component.id,
                component_revision: ComponentRevision::new(revision).unwrap(),
                component_name: component.component_name.0,
                account_id: component.account_id,
                account_email: component.account_email,
            },
        }
    }

    #[test]
    async fn a_lookup_in_flight_during_a_deployment_does_not_keep_the_old_answer() {
        let registry = Arc::new(TestRegistry::new(vec![], None));
        let resolver = Arc::new(AgentTypeResolver::new(
            registry.clone(),
            16,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
        ));
        let environment_id = EnvironmentId::new();
        let name = AgentTypeName("Cart".to_string());
        let gate = LookupGate::new();
        *registry.next_lookup.lock().unwrap() = Some(gate.clone());

        // The lookup reads the registry before the type is deployed.
        let in_flight = tokio::spawn({
            let resolver = resolver.clone();
            let name = name.clone();
            async move { resolver.deployed(environment_id, &name).await }
        });
        gate.started.acquire().await.unwrap().forget();

        *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 2));
        resolver.invalidate_environment(environment_id).await;
        gate.release.add_permits(1);

        let revision = |answer: Option<Arc<RegisteredAgentType>>| {
            answer.map(|registered| registered.implemented_by.component_revision)
        };
        let deployed = Some(ComponentRevision::new(2).unwrap());
        assert_eq!(revision(in_flight.await.unwrap().unwrap()), deployed);
        assert_eq!(
            revision(resolver.deployed(environment_id, &name).await.unwrap()),
            deployed
        );
    }

    #[test]
    async fn a_deployment_replaces_a_cached_answer() {
        let registry = Arc::new(TestRegistry::new(vec![], None));
        let resolver = AgentTypeResolver::new(
            registry.clone(),
            16,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
        );
        let environment_id = EnvironmentId::new();
        let name = AgentTypeName("Cart".to_string());

        assert!(
            resolver
                .deployed(environment_id, &name)
                .await
                .unwrap()
                .is_none()
        );
        *registry.deployed_agent_type.lock().unwrap() = Some(registered(&name, 1));
        assert!(
            resolver
                .deployed(environment_id, &name)
                .await
                .unwrap()
                .is_none(),
            "the answer stays cached until a deployment invalidates it"
        );

        resolver.invalidate_environment(environment_id).await;
        assert!(
            resolver
                .deployed(environment_id, &name)
                .await
                .unwrap()
                .is_some()
        );
    }
}
