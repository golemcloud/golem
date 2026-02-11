// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::WorkerId;
use golem_rib_repl::WorkerFunctionInvoke;
use golem_rib_repl::{ReplComponentDependencies, RibDependencyManager};
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::ValueAndType;
use rib::{ComponentDependency, ComponentDependencyKey};
use std::path::Path;
use uuid::Uuid;

pub struct TestRibReplDependencyManager {
    admin: TestUserContext<EnvBasedTestDependencies>,
    environment_id: EnvironmentId,
}

impl TestRibReplDependencyManager {
    pub async fn new(dependencies: EnvBasedTestDependencies) -> anyhow::Result<Self> {
        let admin = dependencies.admin().await;
        let (_, env) = admin.app_and_env().await?;
        Ok(Self {
            admin,
            environment_id: env.id,
        })
    }
}

#[async_trait]
impl RibDependencyManager for TestRibReplDependencyManager {
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Err(anyhow!("test will need to run with a single component"))
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        component_name: String,
    ) -> anyhow::Result<ComponentDependency> {
        let component = self
            .admin
            .component(&self.environment_id, component_name.as_str())
            .store()
            .await?;

        let component_dependency_key = ComponentDependencyKey {
            component_name,
            component_id: component.id.0,
            component_revision: 0,
            root_package_name: component.metadata.root_package_name().clone(),
            root_package_version: component.metadata.root_package_version().clone(),
        };

        Ok(ComponentDependency::new(
            component_dependency_key,
            component.metadata.exports().to_vec(),
        ))
    }
}

// Embedded RibFunctionInvoke implementation
pub struct TestRibReplWorkerFunctionInvoke {
    embedded_worker_executor: EnvBasedTestDependencies,
}

impl TestRibReplWorkerFunctionInvoke {
    pub fn new(embedded_worker_executor: EnvBasedTestDependencies) -> Self {
        Self {
            embedded_worker_executor,
        }
    }
}

#[async_trait]
impl WorkerFunctionInvoke for TestRibReplWorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: Uuid,
        _component_name: &str,
        worker_name: &str,
        function_name: &str,
        args: Vec<ValueAndType>,
        _return_type: Option<AnalysedType>,
    ) -> anyhow::Result<Option<ValueAndType>> {
        let worker_id = WorkerId {
            component_id: ComponentId(component_id),
            worker_name: worker_name.to_string(),
        };

        self.embedded_worker_executor
            .admin()
            .await
            .invoke_and_await_typed(&worker_id, function_name, args)
            .await
            .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))?
            .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))
    }
}
