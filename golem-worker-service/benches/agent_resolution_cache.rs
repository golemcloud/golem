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

//! Hot-path micro-benchmarks for the worker-service agent resolution cache.
//!
//! The cache returns its cached value by clone on every hit (`Cache::try_get`
//! and `get_or_insert_simple` both hand out an owned `V`). The cached value is
//! a [`ResolvedAgentType`], which owns the agent's full `SchemaGraph` — so an
//! owned cache value forces a deep clone of the whole schema on every REST
//! invocation that hits the cache.
//!
//! Option 4 changes the cache value type to `Arc<ResolvedAgentType>`, turning
//! each hit into a refcount bump. These benches measure exactly that per-hit
//! cost: the deep `ResolvedAgentType` clone (the BEFORE cost) versus an
//! `Arc<ResolvedAgentType>` clone (the AFTER cost).
//!
//! Run before/after a change with:
//!
//! ```text
//! cargo bench -p golem-worker-service --bench agent_resolution_cache
//! ```

use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use golem_common::base_model::Empty;
use golem_common::base_model::account::AccountId;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent::{
    AgentMode, AgentTypeName, RegisteredAgentType, RegisteredAgentTypeImplementer,
    ResolvedAgentType, Snapshotting,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::schema::{
    AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, MetadataEnvelope,
    NamedField, NamedFieldType, OutputSchema, SchemaGraph, SchemaType, SchemaTypeDef, TypeId,
};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
use golem_worker_service::service::agent_resolution_cache::AgentResolutionCache;
use uuid::Uuid;

fn def(id: &str, body: SchemaType) -> SchemaTypeDef {
    SchemaTypeDef {
        id: TypeId::new(id),
        name: None,
        body,
    }
}

/// A graph with `n` named record defs, representative of an agent type whose
/// methods reference a non-trivial registry of named types.
fn shared_graph(n: usize) -> SchemaGraph {
    let mut defs = Vec::with_capacity(n);
    for i in 0..n {
        defs.push(def(
            &format!("t{i:03}"),
            SchemaType::record(vec![
                NamedFieldType {
                    name: "a".to_string(),
                    body: SchemaType::u32(),
                    metadata: MetadataEnvelope::default(),
                },
                NamedFieldType {
                    name: "b".to_string(),
                    body: SchemaType::string(),
                    metadata: MetadataEnvelope::default(),
                },
                NamedFieldType {
                    name: "c".to_string(),
                    body: SchemaType::option(SchemaType::ref_to(TypeId::new("t000"))),
                    metadata: MetadataEnvelope::default(),
                },
            ]),
        ));
    }
    SchemaGraph {
        defs,
        root: SchemaType::record(Vec::new()),
    }
}

fn representative_agent_type() -> AgentTypeSchema {
    let method_fields = vec![
        NamedField::user_supplied("count", SchemaType::u32()),
        NamedField::user_supplied("label", SchemaType::string()),
        NamedField::user_supplied("items", SchemaType::list(SchemaType::u8())),
        NamedField::user_supplied("first", SchemaType::ref_to(TypeId::new("t000"))),
        NamedField::user_supplied("last", SchemaType::ref_to(TypeId::new("t031"))),
    ];

    let methods = (0..8)
        .map(|i| AgentMethodSchema {
            name: format!("do-work-{i}"),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(method_fields.clone()),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        })
        .collect();

    AgentTypeSchema {
        type_name: AgentTypeName("bench-agent".to_string()),
        description: "benchmark agent".to_string(),
        source_language: "rust".to_string(),
        schema: shared_graph(64),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "seed",
                SchemaType::u64(),
            )]),
        },
        methods,
        dependencies: Vec::new(),
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: Vec::new(),
    }
}

fn representative_resolved_agent_type() -> ResolvedAgentType {
    ResolvedAgentType {
        registered_agent_type: RegisteredAgentType {
            agent_type: representative_agent_type(),
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: ComponentId(Uuid::nil()),
                component_revision: ComponentRevision::INITIAL,
                component_name: "bench-component".to_string(),
                account_id: AccountId(Uuid::nil()),
                account_email: AccountEmail::new("bench@golem"),
            },
        },
        environment_id: EnvironmentId(Uuid::nil()),
        deployment_revision: DeploymentRevision::INITIAL,
        current_deployment_revision: Some(CurrentDeploymentRevision::INITIAL),
    }
}

/// Minimal registry double that always resolves to a heavy
/// [`ResolvedAgentType`]. Only `resolve_agent_type_by_names` is exercised by the
/// cache; every other method is unreachable in this benchmark.
struct BenchRegistryService {
    resolved: ResolvedAgentType,
}

#[async_trait::async_trait]
impl RegistryService for BenchRegistryService {
    async fn authenticate_token(
        &self,
        _: &golem_common::model::auth::TokenSecret,
    ) -> Result<AuthCtx, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_limits(
        &self,
        _: AccountId,
    ) -> Result<golem_service_base::model::ResourceLimits, RegistryServiceError> {
        unimplemented!()
    }
    async fn update_worker_connection_limit(
        &self,
        _: AccountId,
        _: &golem_common::base_model::AgentId,
        _: bool,
    ) -> Result<(), RegistryServiceError> {
        unimplemented!()
    }
    async fn batch_update_resource_usage(
        &self,
        _: std::collections::HashMap<
            AccountId,
            golem_service_base::clients::registry::ResourceUsageUpdate,
        >,
    ) -> Result<golem_service_base::model::AccountResourceLimits, RegistryServiceError> {
        unimplemented!()
    }
    async fn download_component(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_component_metadata(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<golem_service_base::model::component::Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_deployed_component_metadata(
        &self,
        _: ComponentId,
    ) -> Result<golem_service_base::model::component::Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_all_deployed_component_revisions(
        &self,
        _: ComponentId,
    ) -> Result<Vec<golem_service_base::model::component::Component>, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_component(
        &self,
        _: AccountId,
        _: golem_common::model::application::ApplicationId,
        _: EnvironmentId,
        _: &str,
    ) -> Result<golem_service_base::model::component::Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_all_agent_types(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_agent_type(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
        _: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_agent_type_by_names(
        &self,
        _app_name: &ApplicationName,
        _environment_name: &EnvironmentName,
        _agent_type_name: &AgentTypeName,
        _deployment_revision: Option<DeploymentRevision>,
        _owner_account_email: Option<&str>,
        _auth_ctx: &AuthCtx,
    ) -> Result<ResolvedAgentType, RegistryServiceError> {
        Ok(self.resolved.clone())
    }
    async fn get_active_routes_for_domain(
        &self,
        _: &golem_common::base_model::domain_registration::Domain,
    ) -> Result<golem_service_base::custom_api::CompiledRoutes, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_active_compiled_mcps_for_domain(
        &self,
        _: &golem_common::base_model::domain_registration::Domain,
    ) -> Result<golem_service_base::mcp::CompiledMcp, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_current_environment_state(
        &self,
        _: EnvironmentId,
    ) -> Result<golem_service_base::model::environment::EnvironmentState, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_definition_by_id(
        &self,
        _: golem_common::model::quota::ResourceDefinitionId,
    ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_definition_by_name(
        &self,
        _: EnvironmentId,
        _: golem_common::model::quota::ResourceName,
    ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
        unimplemented!()
    }
    async fn subscribe_registry_invalidations(
        &self,
        _: Option<u64>,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<
                            golem_common::model::agent::RegistryInvalidationEvent,
                            RegistryServiceError,
                        >,
                    > + Send,
            >,
        >,
        RegistryServiceError,
    > {
        unimplemented!()
    }
    async fn run_registry_invalidation_event_subscriber(
        &self,
        _: &'static str,
        _: Option<tokio_util::sync::CancellationToken>,
        _: std::sync::Arc<dyn golem_service_base::clients::registry::RegistryInvalidationHandler>,
    ) {
        unimplemented!()
    }
}

/// End-to-end cache-hit bench through the real [`AgentResolutionCache`]: this
/// runs the full hit path (`try_get`, staleness check, `get_or_insert_simple`)
/// and hands the caller the cached value the way `invoke_agent_rest` receives
/// it. BEFORE Option 4 the returned value is a freshly deep-cloned
/// `ResolvedAgentType`; AFTER it is an `Arc` clone.
fn bench_cache_hit_end_to_end(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    // `AgentResolutionCache::new` spawns a background eviction task, so it must
    // be constructed within the runtime context.
    let _guard = rt.enter();

    let registry = Arc::new(BenchRegistryService {
        resolved: representative_resolved_agent_type(),
    });
    let cache = AgentResolutionCache::new(
        registry,
        16,
        Duration::from_secs(600),
        Duration::from_secs(600),
    );

    let app = ApplicationName("app".to_string());
    let env = EnvironmentName("env".to_string());
    let agent = AgentTypeName("bench-agent".to_string());
    let auth = AuthCtx::System;

    // Prime the cache and the current-revision tracking so subsequent calls are
    // true (non-stale) hits.
    rt.block_on(async {
        cache
            .resolve(&app, &env, &agent, None, &auth)
            .await
            .expect("prime resolve");
    });

    c.bench_function("agent_resolution_cache_hit/resolve_end_to_end", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(
                    cache
                        .resolve(
                            black_box(&app),
                            black_box(&env),
                            black_box(&agent),
                            None,
                            black_box(&auth),
                        )
                        .await
                        .expect("cache hit"),
                )
            })
        })
    });
}

fn bench_resolved_agent_type_clone(c: &mut Criterion) {
    let resolved = representative_resolved_agent_type();
    let arc_resolved = Arc::new(representative_resolved_agent_type());

    let mut group = c.benchmark_group("agent_resolution_cache_hit");
    // BEFORE Option 4: every cache hit deep-clones the whole `ResolvedAgentType`
    // (including the agent's full `SchemaGraph`).
    group.bench_function("owned_clone", |b| {
        b.iter(|| black_box(black_box(&resolved).clone()))
    });
    // AFTER Option 4: every cache hit only bumps an `Arc` refcount.
    group.bench_function("arc_clone", |b| {
        b.iter(|| black_box(Arc::clone(black_box(&arc_resolved))))
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_resolved_agent_type_clone,
    bench_cache_hit_end_to_end,
);
criterion_main!(benches);
