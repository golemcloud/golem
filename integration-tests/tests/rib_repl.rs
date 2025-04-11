// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
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
use golem_common::model::{ComponentId, TargetWorkerId};
use golem_rib_repl::{ComponentSource, RibRepl};
use golem_rib_repl::{ReplDependencies, RibComponentMetadata, RibDependencyManager};
use golem_rib_repl::{RibReplConfig, WorkerFunctionInvoke};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::analysed_type::{f32, field, list, record, str, u32};
use golem_wasm_rpc::{Value, ValueAndType};
use rib::RibResult;
use std::path::Path;
use std::sync::Arc;
use test_r::inherit_test_dep;
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn test_rib_repl(deps: &EnvBasedTestDependencies) {
    let mut rib_repl = RibRepl::bootstrap(RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(deps.clone())),
        worker_function_invoke: Arc::new(TestRibReplWorkerFunctionInvoke::new(deps.clone())),
        printer: None,
        component_source: Some(ComponentSource {
            component_name: "shopping-cart".to_string(),
            source_path: deps.component_directory().join("shopping-cart.wasm"),
        }),
        prompt: None,
    })
    .await
    .expect("Failed to bootstrap REPL");

    let rib1 = r#"
      let worker = instance("my_worker")
    "#;

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
        .execute_rib(rib1)
        .await
        .expect("Failed to process command");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .execute_rib(rib2)
        .await
        .map_err(|err| err.to_string());

    assert!(result.unwrap_err().contains("function 'add' not found"));

    let result = rib_repl
        .execute_rib(rib3)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![]),
            list(record(vec![
                field("product-id", str()),
                field("name", str()),
                field("price", f32()),
                field("quantity", u32()),
            ],),),
        )))
    );

    let result = rib_repl
        .execute_rib(rib4)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .execute_rib(rib5)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![Value::Record(vec![
                Value::String("123".to_string()),
                Value::String("item1".to_string()),
                Value::F32(10.0),
                Value::U32(2),
            ])]),
            list(record(vec![
                field("product-id", str()),
                field("name", str()),
                field("price", f32()),
                field("quantity", u32()),
            ],)),
        )))
    );
}

#[test]
#[tracing::instrument]
async fn test_rib_repl_with_resource(deps: &EnvBasedTestDependencies) {
    let mut rib_repl = RibRepl::bootstrap(RibReplConfig {
        history_file: None,
        dependency_manager: Arc::new(TestRibReplDependencyManager::new(deps.clone())),
        worker_function_invoke: Arc::new(TestRibReplWorkerFunctionInvoke::new(deps.clone())),
        printer: None,
        component_source: Some(ComponentSource {
            component_name: "shopping-cart-resource".to_string(),
            source_path: deps
                .component_directory()
                .join("shopping-cart-resource.wasm"),
        }),
        prompt: None,
    })
    .await
    .expect("Failed to bootstrap REPL");

    let rib1 = r#"
      let worker = instance("my_worker")
    "#;

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

    let result = rib_repl
        .execute_rib(rib1)
        .await
        .expect("Failed to process command");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .execute_rib(rib2)
        .await
        .expect("Failed to process command");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .execute_rib(rib3)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![]),
            list(record(vec![
                field("product-id", str()),
                field("name", str()),
                field("price", f32()),
                field("quantity", u32()),
            ],),),
        )))
    );

    let result = rib_repl
        .execute_rib(rib4)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .execute_rib(rib5)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(
        result,
        Some(RibResult::Val(ValueAndType::new(
            Value::List(vec![Value::Record(vec![
                Value::String("123".to_string()),
                Value::String("item1".to_string()),
                Value::F32(10.0),
                Value::U32(2),
            ])]),
            list(record(vec![
                field("product-id", str()),
                field("name", str()),
                field("price", f32()),
                field("quantity", u32()),
            ],)),
        )))
    );
}

struct TestRibReplDependencyManager {
    dependencies: EnvBasedTestDependencies,
}

impl TestRibReplDependencyManager {
    fn new(dependencies: EnvBasedTestDependencies) -> Self {
        Self { dependencies }
    }
}

#[async_trait]
impl RibDependencyManager for TestRibReplDependencyManager {
    async fn get_dependencies(&self) -> anyhow::Result<ReplDependencies> {
        Err(anyhow!(
            "test will need to run with a single component".to_string()
        ))
    }

    async fn add_component(
        &self,
        _source_path: &Path,
        component_name: String,
    ) -> anyhow::Result<RibComponentMetadata> {
        let component_id = self
            .dependencies
            .component(component_name.as_str())
            .store()
            .await;

        let metadata = self
            .dependencies
            .get_latest_component_metadata(&component_id)
            .await;

        Ok(RibComponentMetadata {
            component_name,
            component_id: component_id.0,
            metadata: metadata.exports,
        })
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
        worker_name: Option<String>,
        function_name: &str,
        args: Vec<ValueAndType>,
    ) -> anyhow::Result<ValueAndType> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: Some(w),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: None,
            });

        self.embedded_worker_executor
            .invoke_and_await_typed(target_worker_id, function_name, args)
            .await
            .map_err(|e| anyhow!("Failed to invoke function: {:?}", e))
    }
}
