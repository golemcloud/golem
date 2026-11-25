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

//! E13: the oplog processors of a never-live fork.

use super::{settled_update_outcomes_after, tip};
use crate::Tracing;
use axum::routing::post;
use axum::{Json, Router};
use golem_common::model::OwnedAgentId;
use golem_common::model::component::{ComponentId, InstalledPlugin, PluginPriority};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantId;
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::{agent_key, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::component::ComponentService;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides, TestWorkerExecutor,
    WorkerExecutorTestDependencies, test_component,
};
use pretty_assertions::assert_eq;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);
test_component!(
    never_live_processor,
    "never_live_processor",
    "oplog_processor_release",
    "oplog:processor"
);

/// The oplog processor plugin that [`WithProcessorPlugin`] installs, and the component it is
/// installed on.
type Installation = Arc<Mutex<Option<(ComponentId, InstalledPlugin)>>>;

/// A component service that installs one oplog processor plugin on every agent type of one
/// component, as a deployment of that plugin does.
struct WithProcessorPlugin {
    inner: Arc<dyn ComponentService>,
    installation: Installation,
}

impl WithProcessorPlugin {
    fn install(
        &self,
        mut component: golem_service_base::model::component::Component,
    ) -> golem_service_base::model::component::Component {
        let Some((target, plugin)) = self.installation.lock().unwrap().clone() else {
            return component;
        };
        if component.id != target {
            return component;
        }
        let mut configs = component.metadata.agent_type_provision_configs().clone();
        for config in configs.values_mut() {
            if !config.plugins.iter().any(|installed| {
                installed.environment_plugin_grant_id == plugin.environment_plugin_grant_id
            }) {
                config.plugins.push(plugin.clone());
            }
        }
        component.metadata = component.metadata.with_provision_configs(configs);
        component
    }
}

#[async_trait::async_trait]
impl ComponentService for WithProcessorPlugin {
    async fn get(
        &self,
        engine: &wasmtime::Engine,
        component_id: ComponentId,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<
        (
            wasmtime::component::Component,
            golem_service_base::model::component::Component,
        ),
        WorkerExecutorError,
    > {
        let (wasm, component) = self
            .inner
            .get(engine, component_id, component_revision)
            .await?;
        Ok((wasm, self.install(component)))
    }

    async fn get_metadata(
        &self,
        component_id: ComponentId,
        forced_revision: Option<golem_common::model::component::ComponentRevision>,
    ) -> Result<golem_service_base::model::component::Component, WorkerExecutorError> {
        Ok(self.install(
            self.inner
                .get_metadata(component_id, forced_revision)
                .await?,
        ))
    }

    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_environment: golem_common::model::environment::EnvironmentId,
        resolving_application: golem_common::model::application::ApplicationId,
        resolving_account: golem_common::model::account::AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        self.inner
            .resolve_component(
                component_reference,
                resolving_environment,
                resolving_application,
                resolving_account,
            )
            .await
    }

    async fn all_cached_metadata(&self) -> Vec<golem_service_base::model::component::Component> {
        self.inner
            .all_cached_metadata()
            .await
            .into_iter()
            .map(|component| self.install(component))
            .collect()
    }

    async fn invalidate_current_deployed_metadata(&self) {
        self.inner.invalidate_current_deployed_metadata().await;
    }

    async fn invalidate_current_deployed_metadata_for_environment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_current_deployed_metadata_for_environment(environment_id)
            .await;
    }

    async fn invalidate_all_metadata_for_environment(
        &self,
        environment_id: golem_common::model::environment::EnvironmentId,
    ) {
        self.inner
            .invalidate_all_metadata_for_environment(environment_id)
            .await;
    }

    async fn invalidate_all(&self) {
        self.inner.invalidate_all().await;
    }
}

/// An executor whose component service installs the plugin of `installation`, and whose
/// forwarding oplog sends every commit to the processors at once.
async fn executor(
    deps: &WorkerExecutorTestDependencies,
    context: &TestContext,
    installation: &Installation,
) -> anyhow::Result<TestWorkerExecutor> {
    crate::fork::start_with_local_resume_and(
        deps,
        context,
        TestExecutorOverrides {
            configure: Some(Arc::new(|config| {
                config.oplog.plugin_max_commit_count = 1;
                config.oplog.plugin_max_elapsed_time = Duration::from_millis(100);
            })),
            wrap_component_service: Some(Arc::new({
                let installation = installation.clone();
                move |inner| {
                    Arc::new(WithProcessorPlugin {
                        inner,
                        installation: installation.clone(),
                    })
                }
            })),
            ..Default::default()
        },
    )
    .await
}

/// The agent names in the batches that the processor posted to the test server.
fn batch_sources(batches: &Mutex<Vec<serde_json::Value>>) -> Vec<String> {
    batches
        .lock()
        .unwrap()
        .iter()
        .filter_map(|batch| batch["source_worker_id"].as_str().map(str::to_string))
        .collect()
}

/// Waits until the processor posted a batch of `agent`.
async fn processed(
    batches: &Mutex<Vec<serde_json::Value>>,
    agent: &OwnedAgentId,
) -> anyhow::Result<()> {
    let name = agent.agent_id.to_string();
    tokio::time::timeout(Duration::from_secs(60), async {
        while !batch_sources(batches).contains(&name) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("no batch of {name}: {:?}", batch_sources(batches)))
}

/// E13: a source agent has an active oplog processor plugin, which a never-live fork copies with
/// its `Create`. The processor receives the oplog of the source and of an unmarked fork, but
/// nothing of the never-live fork: not at the fork, not during an automatic update, and not
/// during a second update after a cold restart of the executor.
#[test]
#[timeout("6m")]
async fn no_oplog_processor_receives_the_oplog_of_a_never_live_fork(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    #[tagged_as("never_live_processor")] processor_component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let installation: Installation = Arc::new(Mutex::new(None));
    let executor = executor(deps, &context, &installation).await?;
    let processor = executor
        .component_dep(&context.default_environment_id, processor_component)
        .store()
        .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;

    let batches = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let callback_url = format!("http://{}/batch", listener.local_addr()?);
    let server = tokio::spawn({
        let batches = batches.clone();
        async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/batch",
                    post(move |Json(body): Json<serde_json::Value>| {
                        let batches = batches.clone();
                        async move {
                            batches.lock().unwrap().push(body);
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        }
    });
    let _server = tokio_util::task::AbortOnDropHandle::new(server);
    *installation.lock().unwrap() = Some((
        component.id,
        InstalledPlugin {
            environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
            priority: PluginPriority(0),
            parameters: [("callback-url".to_string(), callback_url)].into(),
            plugin_registration_id: PluginRegistrationId::new(),
            plugin_name: "never-live-processor".to_string(),
            plugin_version: "1".to_string(),
            oplog_processor_component_id: Some(processor.id),
            oplog_processor_component_revision: Some(processor.revision),
            installed_by: golem_common::model::account::AccountId::new(),
        },
    ));

    let name = format!("e13-{}", uuid::Uuid::new_v4());
    let agent = agent_key!("Clock", name.clone());
    let source_id = executor.start_agent(&component.id, agent.clone()).await?;
    executor
        .invoke_and_await_agent(&component, &agent, "healthcheck", data_value!())
        .await?;
    processed(&batches, &source_id).await?;
    let cut = tip(&executor, &source_id).await?;

    let marked = executor.fork_worker_never_live(&source_id, cut).await?;
    executor
        .auto_update_worker(&marked, component.revision, false)
        .await?;
    let outcomes = settled_update_outcomes_after(&executor, &marked, cut).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );

    // The control: an unmarked fork of the same cut reaches the processor.
    let unmarked = executor.fork_worker(&source_id, cut).await?;
    crate::agent::invoke_and_await_agent_by_id(
        &executor,
        &component,
        &unmarked,
        None,
        "healthcheck",
        data_value!(),
    )
    .await?;
    processed(&batches, &unmarked).await?;

    // A cold restart hydrates the never-live fork again for its second update.
    drop(executor);
    let executor = self::executor(deps, &context, &installation).await?;
    let before = tip(&executor, &marked).await?;
    executor
        .auto_update_worker(&marked, component.revision, false)
        .await?;
    let outcomes = settled_update_outcomes_after(&executor, &marked, before).await?;
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        matches!(outcomes[0].entry, PublicOplogEntry::SuccessfulUpdate(_)),
        "{outcomes:?}"
    );
    executor
        .invoke_and_await_agent(&component, &agent, "healthcheck", data_value!())
        .await?;
    let processed_before = batch_sources(&batches).len();
    tokio::time::timeout(Duration::from_secs(60), async {
        while batch_sources(&batches).len() == processed_before {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("the source's processor stopped after the restart"))?;

    tokio::time::sleep(Duration::from_secs(2)).await;
    let marked_name = marked.agent_id.to_string();
    assert!(
        !batch_sources(&batches).contains(&marked_name),
        "{marked_name}: {:?}",
        batch_sources(&batches)
    );
    Ok(())
}
