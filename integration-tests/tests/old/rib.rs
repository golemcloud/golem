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
use golem_common::model::{ComponentId, TargetWorkerId};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::analysed_type::{f32, field, list, record, str, u32};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{Value, ValueAndType};
use rib::{
    ComponentDependency, ComponentDependencyKey, EvaluatedFnArgs, EvaluatedFqFn,
    EvaluatedWorkerName, InstructionId, RibCompilerConfig, RibComponentFunctionInvoke,
    RibEvalConfig, RibEvaluator, RibFunctionInvokeResult, RibInput, RibResult,
};
use std::sync::Arc;
use test_r::inherit_test_dep;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn test_rib_simple_without_worker_name(deps: &EnvBasedTestDependencies) {
    test_simple_rib(deps, None).await;
}

#[test]
#[tracing::instrument]
async fn test_rib_simple_with_worker_name(deps: &EnvBasedTestDependencies) {
    test_simple_rib(deps, Some("rib-simple-worker")).await;
}

#[test]
#[tracing::instrument]
async fn test_rib_complex_without_worker_name(deps: &EnvBasedTestDependencies) {
    test_rib_for_loop(deps, None).await;
}

#[test]
#[tracing::instrument]
async fn test_rib_complex_with_worker_name(deps: &EnvBasedTestDependencies) {
    test_rib_for_loop(deps, Some("rib-complex-worker")).await;
}

#[test]
#[tracing::instrument]
async fn test_rib_with_resource_methods_without_worker_param(deps: &EnvBasedTestDependencies) {
    test_rib_with_resource_methods(deps, None).await;
}

#[test]
#[tracing::instrument]
async fn test_rib_with_resource_methods_with_worker_param(deps: &EnvBasedTestDependencies) {
    test_rib_with_resource_methods(deps, Some("rib-with-resource-worker")).await;
}

async fn test_simple_rib(deps: &EnvBasedTestDependencies, worker_name: Option<&str>) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let metadata = admin.get_latest_component_metadata(&component_id).await;

    let component_dependency_key = ComponentDependencyKey {
        component_name: "shopping-cart".to_string(),
        component_id: component_id.0,
        root_package_name: metadata.root_package_name().clone(),
        root_package_version: metadata.root_package_version().clone(),
    };

    let component_dependency =
        ComponentDependency::new(component_dependency_key, metadata.exports().to_vec());

    let compiler_config = RibCompilerConfig::new(vec![component_dependency], vec![]);

    let rib_function_invoke = Arc::new(TestRibFunctionInvoke::new(deps.clone()));

    let rib = match worker_name {
        Some(worker_name) => {
            format!(
                r#"
                let worker = instance("{worker_name}");
                let result = worker.get-cart-contents();
                worker.add-item({{
                    product-id: "123",
                    name: "item1",
                    price: 10.0,
                    quantity: 2
                }});
                worker.get-cart-contents()
                "#
            )
        }

        None => r#"
                let worker = instance();
                let result = worker.get-cart-contents();
                worker.add-item({
                    product-id: "123",
                    name: "item1",
                    price: 10.0,
                    quantity: 2
                });
                worker.get-cart-contents()
            "#
        .to_string(),
    };

    let eval_config = RibEvalConfig::new(
        compiler_config,
        RibInput::default(),
        rib_function_invoke,
        None,
    );

    let rib_evaluator = RibEvaluator::new(eval_config);

    let result = rib_evaluator
        .eval(&rib)
        .await
        .expect("Failed to evaluate rib");

    assert_eq!(
        result,
        RibResult::Val(ValueAndType::new(
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
        ))
    );
}

async fn test_rib_for_loop(deps: &EnvBasedTestDependencies, worker_name: Option<&str>) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart").store().await;

    let metadata = admin.get_latest_component_metadata(&component_id).await;

    let component_dependency_key = ComponentDependencyKey {
        component_name: "shopping-cart".to_string(),
        component_id: component_id.0,
        root_package_name: metadata.root_package_name().clone(),
        root_package_version: metadata.root_package_version().clone(),
    };

    let component_dependency =
        ComponentDependency::new(component_dependency_key, metadata.exports().to_vec());

    let compiler_config = RibCompilerConfig::new(vec![component_dependency], vec![]);

    let rib_function_invoke = Arc::new(TestRibFunctionInvoke::new(deps.clone()));

    let rib = match worker_name {
        Some(worker_name) => {
            format!(
                r#"
                let worker = instance("{worker_name}");
                let result = worker.get-cart-contents();

                for i in 1:u32..=2:u32 {{
                    yield worker.add-item({{
                        product-id: "123",
                        name: "item1",
                        price: 10.0,
                        quantity: i
                    }});
                }};

                worker.get-cart-contents()
                "#
            )
        }

        None => r#"
                let worker = instance();
                let result = worker.get-cart-contents();

                for i in 1:u32..=2:u32 {
                    yield worker.add-item({
                        product-id: "123",
                        name: "item1",
                        price: 10.0,
                        quantity: i
                    });
                };

                worker.get-cart-contents()
            "#
        .to_string(),
    };

    let eval_config = RibEvalConfig::new(
        compiler_config,
        RibInput::default(),
        rib_function_invoke,
        None,
    );

    let rib_evaluator = RibEvaluator::new(eval_config);

    let result = rib_evaluator
        .eval(&rib)
        .await
        .expect("Failed to evaluate rib");

    assert_eq!(
        result,
        RibResult::Val(ValueAndType::new(
            Value::List(vec![
                Value::Record(vec![
                    Value::String("123".to_string()),
                    Value::String("item1".to_string()),
                    Value::F32(10.0),
                    Value::U32(1),
                ]),
                Value::Record(vec![
                    Value::String("123".to_string()),
                    Value::String("item1".to_string()),
                    Value::F32(10.0),
                    Value::U32(2),
                ]),
            ]),
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
        ))
    );
}

async fn test_rib_with_resource_methods(
    deps: &EnvBasedTestDependencies,
    worker_name: Option<&str>,
) {
    let admin = deps.admin().await;
    let component_id = admin.component("shopping-cart-resource").store().await;

    let metadata = admin.get_latest_component_metadata(&component_id).await;

    let component_dependency_key = ComponentDependencyKey {
        component_name: "shopping-cart".to_string(),
        component_id: component_id.0,
        root_package_name: metadata.root_package_name().clone(),
        root_package_version: metadata.root_package_version().clone(),
    };

    let component_dependency =
        ComponentDependency::new(component_dependency_key, metadata.exports().to_vec());

    let compiler_config = RibCompilerConfig::new(vec![component_dependency], vec![]);

    let rib_function_invoke = Arc::new(TestRibFunctionInvoke::new(deps.clone()));

    let rib = match worker_name {
        Some(worker_name) => {
            format!(
                r#"
                let resource = instance("{worker_name}");
                let cart = resource.cart("foo");

                for i in 1:u32..=2:u32 {{
                    yield cart.add-item({{
                        product-id: "123",
                        name: "item1",
                        price: 10.0,
                        quantity: i
                    }});
                }};

                cart.get-cart-contents()
                "#
            )
        }

        None => r#"
                let resource = instance();
                let cart = resource.cart("foo");

                for i in 1:u32..=2:u32 {
                    yield cart.add-item({
                        product-id: "123",
                        name: "item1",
                        price: 10.0,
                        quantity: i
                    });
                };

                cart.get-cart-contents()
            "#
        .to_string(),
    };

    let eval_config = RibEvalConfig::new(
        compiler_config,
        RibInput::default(),
        rib_function_invoke,
        None,
    );

    let rib_evaluator = RibEvaluator::new(eval_config);

    let result = rib_evaluator
        .eval(&rib)
        .await
        .expect("Failed to evaluate rib");

    assert_eq!(
        result,
        RibResult::Val(ValueAndType::new(
            Value::List(vec![
                Value::Record(vec![
                    Value::String("123".to_string()),
                    Value::String("item1".to_string()),
                    Value::F32(10.0),
                    Value::U32(1),
                ]),
                Value::Record(vec![
                    Value::String("123".to_string()),
                    Value::String("item1".to_string()),
                    Value::F32(10.0),
                    Value::U32(2),
                ])
            ]),
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
        ))
    );
}

struct TestRibFunctionInvoke {
    dependencies: EnvBasedTestDependencies,
}

impl TestRibFunctionInvoke {
    fn new(dependencies: EnvBasedTestDependencies) -> Self {
        Self { dependencies }
    }
}

#[async_trait]
impl RibComponentFunctionInvoke for TestRibFunctionInvoke {
    async fn invoke(
        &self,
        component_dependency_key: ComponentDependencyKey,
        _instruction_id: &InstructionId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
        _return_type: Option<AnalysedType>,
    ) -> RibFunctionInvokeResult {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: ComponentId(component_dependency_key.component_id),
                worker_name: Some(w.0),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id: ComponentId(component_dependency_key.component_id),
                worker_name: None,
            });

        let result = self
            .dependencies
            .admin()
            .await
            .invoke_and_await_typed(target_worker_id, function_name.0.as_str(), args.0)
            .await;

        Ok(result.map_err(|err| {
            tracing::error!("Failed to invoke function: {:?}", err);
            anyhow!("Failed to invoke function: {:?}", err)
        })?)
    }
}
