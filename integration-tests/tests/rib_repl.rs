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

use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::{Value, ValueAndType};
use std::path::Path;
use std::sync::Arc;
use tracing::Instrument;

use crate::Tracing;
use async_trait::async_trait;
use golem_common::model::{ComponentId, TargetWorkerId};
use golem_rib_repl::dependency_manager::{
    ReplDependencies, RibComponentMetadata, RibDependencyManager,
};
use golem_rib_repl::invoke::WorkerFunctionInvoke;
use golem_rib_repl::repl_printer::DefaultResultPrinter;
use golem_rib_repl::rib_repl::{ComponentDetails, RibRepl};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_wasm_ast::analysis::analysed_type::{f32, field, list, record, str, u32};
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibResult};
use test_r::inherit_test_dep;
use uuid::Uuid;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn test_rib_repl(deps: &EnvBasedTestDependencies) {
    let mut rib_repl = RibRepl::bootstrap(
        None,
        Arc::new(TestRibReplDependencyManager::new(deps.clone())),
        Arc::new(TestRibReplWorkerFunctionInvoke::new(deps.clone())),
        Box::new(DefaultResultPrinter),
        Some(ComponentDetails {
            component_name: "shopping-cart".to_string(),
            source_path: deps.component_directory().join("shopping-cart.wasm"),
        }),
    )
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
        .process_command(rib1)
        .await
        .expect("Failed to process command");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .process_command(rib2)
        .await
        .map_err(|err| err.to_string());

    assert!(result.unwrap_err().contains("function 'add' not found"));

    let result = rib_repl
        .process_command(rib3)
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
        .process_command(rib4)
        .await
        .map_err(|err| err.to_string())
        .expect("Failed to process rib");

    assert_eq!(result, Some(RibResult::Unit));

    let result = rib_repl
        .process_command(rib5)
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
    async fn get_dependencies(&self) -> Result<ReplDependencies, String> {
        Err("test will need to run with a single component".to_string())
    }

    async fn add_component(
        &self,
        source_path: &Path,
        component_name: String,
    ) -> Result<RibComponentMetadata, String> {
        let component_id = self.dependencies.component("shopping-cart").store().await;
        let metadata = self
            .dependencies
            .get_latest_component_metadata(&component_id)
            .await;
        Ok(RibComponentMetadata {
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
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: Some(w.0),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_id),
                worker_name: None,
            });

        let function_name = function_name.0;

        self.embedded_worker_executor
            .invoke_and_await_typed(target_worker_id, function_name.as_str(), args.0)
            .await
            .map_err(|e| format!("Failed to invoke function: {:?}", e))
    }
}
