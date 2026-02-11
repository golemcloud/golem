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

use test_r::test;

use crate::Tracing;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::WorkerId;
use golem_rib_repl::{ComponentSource, RibRepl};
use golem_rib_repl::{ReplComponentDependencies, RibDependencyManager};
use golem_rib_repl::{RibReplConfig, WorkerFunctionInvoke};
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended, WorkerInvocationResultOps};
use golem_wasm::analysis::analysed_type::{f32, field, list, record, str, u32};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use rib::{ComponentDependency, ComponentDependencyKey, RibResult};
use std::path::Path;
use std::sync::Arc;
use test_r::inherit_test_dep;
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn test_rib_repl(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    test_repl_invoking_functions(deps, Some("worker-repl-simple-test")).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_repl_without_worker_param(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    test_repl_invoking_functions(deps, None).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_repl_with_resource(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    test_repl_invoking_resource_methods(deps, Some("worker-repl-resource-test")).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_repl_with_resource_without_param(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    test_repl_invoking_resource_methods(deps, None).await?;
    Ok(())
}

async fn test_repl_invoking_functions(
    deps: &EnvBasedTestDependencies,
    worker_name: Option<&str>,
) -> anyhow::Result<()> {
    let dsl = deps.user().await?;
    let (_, env) = dsl.app_and_env().await?;

    let mut rib_repl = RibRepl::bootstrap(RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(dsl.clone(), env.id)),
        worker_function_invoke: Arc::new(TestRibReplWorkerFunctionInvoke::new(dsl)),
        printer: None,
        component_source: Some(ComponentSource {
            component_name: "shopping-cart".to_string(),
            source_path: deps.component_directory().join("shopping-cart.wasm"),
        }),
        prompt: None,
        command_registry: None,
    })
    .await
    .expect("Failed to bootstrap REPL");

    let rib1 = match worker_name {
        Some(name) => format!(r#"let worker = instance("{name}")"#),
        None => r#"let worker = instance()"#.to_string(),
    };

    let rib2 = r#"
      let result = worker.add(1, 2)
    "#;

    let rib3 = r#"
      worker.get-cart-contents()
     "#;

    let rib4 = r#"
      worker.add-item({
        product-id: "123",
        name: "item1",
        price: 10.0,
        quantity: 2
      })
    "#;

    let rib5 = r#"
      worker.get-cart-contents()
     "#;

    let result = rib_repl
        .execute(&rib1)
        .await
        .expect("Failed to process command");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl.execute(rib2).await.map_err(|err| err.to_string());

    assert!(result.unwrap_err().contains("function 'add' not found"));

    let result = rib_repl.execute(rib3).await?;

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![]),
            list(
                record(vec![
                    field("product-id", str()),
                    field("name", str()),
                    field("price", f32()),
                    field("quantity", u32()),
                ])
                .named("product-item")
                .owned("golem:it/api")
            ),
        )))
    );

    let result = rib_repl.execute(rib4).await?;

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl.execute(rib5).await?;

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![Value::Record(vec![
                Value::String("123".to_string()),
                Value::String("item1".to_string()),
                Value::F32(10.0),
                Value::U32(2),
            ])]),
            list(
                record(vec![
                    field("product-id", str()),
                    field("name", str()),
                    field("price", f32()),
                    field("quantity", u32()),
                ])
                .named("product-item")
                .owned("golem:it/api")
            ),
        )))
    );

    Ok(())
}

async fn test_repl_invoking_resource_methods(
    deps: &EnvBasedTestDependencies,
    worker_name: Option<&str>,
) -> anyhow::Result<()> {
    let dsl = deps.user().await?;
    let (_, env) = dsl.app_and_env().await?;

    let mut rib_repl = RibRepl::bootstrap(RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(dsl.clone(), env.id)),
        worker_function_invoke: Arc::new(TestRibReplWorkerFunctionInvoke::new(dsl)),
        printer: None,
        component_source: Some(ComponentSource {
            component_name: "shopping-cart-resource".to_string(),
            source_path: deps
                .component_directory()
                .join("shopping-cart-resource.wasm"),
        }),
        prompt: None,
        command_registry: None,
    })
    .await?;

    let rib1 = match worker_name {
        Some(name) => format!(r#"let worker = instance("{name}")"#),
        None => r#"let worker = instance()"#.to_string(),
    };

    let rib2 = r#"
      let resource = worker.cart("foo")
    "#;

    let rib3 = r#"
      resource.get-cart-contents()
     "#;

    let rib4 = r#"
      resource.add-item({
        product-id: "123",
        name: "item1",
        price: 10.0,
        quantity: 2
      })
    "#;

    let rib5 = r#"
      resource.get-cart-contents()
     "#;

    let result = rib_repl.execute(&rib1).await?;

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl.execute(rib2).await?;

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .execute(rib3)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![]),
            list(
                record(vec![
                    field("product-id", str()),
                    field("name", str()),
                    field("price", f32()),
                    field("quantity", u32()),
                ])
                .named("product-item")
                .owned("golem:it/api")
            ),
        )))
    );

    let result = rib_repl.execute(rib4).await?;

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl.execute(rib5).await?;

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![Value::Record(vec![
                Value::String("123".to_string()),
                Value::String("item1".to_string()),
                Value::F32(10.0),
                Value::U32(2),
            ])]),
            list(
                record(vec![
                    field("product-id", str()),
                    field("name", str()),
                    field("price", f32()),
                    field("quantity", u32()),
                ])
                .named("product-item")
                .owned("golem:it/api")
            ),
        )))
    );

    Ok(())
}

struct TestRibReplDependencyManager {
    test_dsl: TestUserContext<EnvBasedTestDependencies>,
    environment_id: EnvironmentId,
}

impl TestRibReplDependencyManager {
    fn new(
        test_dsl: TestUserContext<EnvBasedTestDependencies>,
        environment_id: EnvironmentId,
    ) -> Self {
        Self {
            test_dsl,
            environment_id,
        }
    }
}

#[async_trait]
impl RibDependencyManager for TestRibReplDependencyManager {
    async fn get_dependencies(&self) -> anyhow::Result<ReplComponentDependencies> {
        Err(anyhow!(
            "test will need to run with a single component".to_string()
        ))
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        component_name: String,
    ) -> anyhow::Result<ComponentDependency> {
        let component = self
            .test_dsl
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

pub struct TestRibReplWorkerFunctionInvoke {
    test_dsl: TestUserContext<EnvBasedTestDependencies>,
}

impl TestRibReplWorkerFunctionInvoke {
    pub fn new(test_dsl: TestUserContext<EnvBasedTestDependencies>) -> Self {
        Self { test_dsl }
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

        let result = self
            .test_dsl
            .invoke_and_await_typed(&worker_id, function_name, args)
            .await
            .collapse();

        Ok(result.map_err(|err| {
            tracing::error!("Failed to invoke function: {:?}", err);
            anyhow!("Failed to invoke function: {:?}", err)
        })?)
    }
}
