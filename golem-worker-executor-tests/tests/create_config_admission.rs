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
use super::*;
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentConfigEntryDto, AgentConfigSource};
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::SchemaType;
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_service_base::model::component::Component;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor::worker::CreateIntent;
use test_r::{test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(
    #[tagged_as("agent_counters")]
    PrecompiledComponent
);

struct RecordConfigComponentService(Arc<dyn ComponentService>);

impl RecordConfigComponentService {
    fn declare_config(mut component: Component) -> Component {
        let metadata = &component.metadata;
        let mut agents = metadata.agent_types().to_vec();
        for agent in &mut agents {
            if agent.type_name.0 == "Counter" {
                agent.config.push(AgentConfigDeclarationSchema {
                    source: AgentConfigSource::Local,
                    path: vec!["retryRecord".into()],
                    value_type: SchemaType::option(SchemaType::record_from_fields([(
                        "optionalField",
                        SchemaType::option(SchemaType::string()),
                    )])),
                });
            }
        }
        component.metadata = ComponentMetadata::from_parts(
            metadata.known_exports().clone(),
            metadata.memories().to_vec(),
            agents,
            metadata.agent_type_provision_configs().clone(),
        );
        component
    }
}

#[async_trait]
impl ComponentService for RecordConfigComponentService {
    async fn get(
        &self,
        engine: &wasmtime::Engine,
        id: ComponentId,
        revision: ComponentRevision,
    ) -> Result<(wasmtime::component::Component, Component), WorkerExecutorError> {
        let (compiled, metadata) = self.0.get(engine, id, revision).await?;
        Ok((compiled, Self::declare_config(metadata)))
    }

    async fn get_metadata(
        &self,
        id: ComponentId,
        revision: Option<ComponentRevision>,
    ) -> Result<Component, WorkerExecutorError> {
        Ok(Self::declare_config(
            self.0.get_metadata(id, revision).await?,
        ))
    }

    async fn resolve_component(
        &self,
        reference: String,
        environment: EnvironmentId,
        application: ApplicationId,
        account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        self.0
            .resolve_component(reference, environment, application, account)
            .await
    }

    async fn all_cached_metadata(&self) -> Vec<Component> {
        self.0
            .all_cached_metadata()
            .await
            .into_iter()
            .map(Self::declare_config)
            .collect()
    }

    async fn invalidate_all_metadata_for_environment(&self, environment: EnvironmentId) {
        self.0
            .invalidate_all_metadata_for_environment(environment)
            .await
    }
}

#[test]
#[timeout("2 minutes")]
async fn identical_keyed_record_config_retry_is_admitted(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_counters")] component: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            wrap_component_service: Some(Arc::new(|inner| {
                Arc::new(RecordConfigComponentService(inner))
            })),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let seed = seed_worker(&executor, &component).await?;
    let mut failures = Vec::new();
    for (case, value) in [
        (
            "explicit-null-control",
            serde_json::json!({"optionalField": null}),
        ),
        ("omitted-optional-field", serde_json::json!({})),
    ] {
        let id = target(&seed, case);
        let key = IdempotencyKey::fresh();
        let create = |value: serde_json::Value| {
            let seed = seed.clone();
            let id = id.clone();
            let key = key.clone();
            async move {
                Worker::get_or_create_suspended_with_freshness(
                    seed.all(),
                    &id,
                    None,
                    vec![AgentConfigEntryDto {
                        path: vec!["retryRecord".into()],
                        value: value.into(),
                    }],
                    None,
                    None,
                    &InvocationContextStack::fresh(),
                    Principal::anonymous(),
                    InvocationFreshnessDisposition::MayExist,
                    CreateIntent::Create {
                        exclusive: true,
                        idempotency_key: Some(key),
                    },
                )
                .await
            }
        };
        let original = create(value.clone()).await?;
        for cold in [false, true] {
            if cold {
                seed.active_agents().remove(&id).await;
            }
            match create(value.clone()).await {
                Ok(retry) => assert_eq!(
                    retry.get_initial_worker_metadata().fingerprint,
                    original.get_initial_worker_metadata().fingerprint
                ),
                Err(error) => failures.push(format!("{case}, cold={cold}: {error:?}")),
            }
            assert!(matches!(
                create(serde_json::json!({"optionalField": "changed"})).await,
                Err(WorkerExecutorError::CreateIdempotencyKeyConflict { .. })
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "identical valid keyed requests must be admitted: {failures:#?}"
    );
    Ok(())
}
