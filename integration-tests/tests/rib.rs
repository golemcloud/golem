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

use crate::Tracing;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::component::ComponentId;
use golem_common::model::WorkerId;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended, WorkerInvocationResultOps};
use golem_wasm::analysis::analysed_type::{f32, field, list, record, str, u32};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};
use rib::{
    ComponentDependency, ComponentDependencyKey, EvaluatedFnArgs, EvaluatedFqFn,
    EvaluatedWorkerName, InstructionId, RibCompilerConfig, RibComponentFunctionInvoke,
    RibEvalConfig, RibEvaluator, RibFunctionInvokeResult, RibInput, RibResult,
};
use std::sync::Arc;
use test_r::inherit_test_dep;
use test_r::test;

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn test_rib_simple_without_worker_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    test_simple_rib(deps, None).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_simple_with_worker_name(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    test_simple_rib(deps, Some("rib-simple-worker")).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_complex_without_worker_name(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    test_rib_for_loop(deps, None).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_complex_with_worker_name(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    test_rib_for_loop(deps, Some("rib-complex-worker")).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_with_resource_methods_without_worker_param(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    test_rib_with_resource_methods(deps, None).await?;
    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_rib_with_resource_methods_with_worker_param(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    test_rib_with_resource_methods(deps, Some("rib-with-resource-worker")).await?;
    Ok(())
}

async fn test_simple_rib(
    deps: &EnvBasedTestDependencies,
    worker_name: Option<&str>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user.component(&env.id, "shopping-cart").store().await?;

    let component_dependency_key = ComponentDependencyKey {
        component_name: "shopping-cart".to_string(),
        component_id: component.id.0,
        component_revision: 0,
        root_package_name: component.metadata.root_package_name().clone(),
        root_package_version: component.metadata.root_package_version().clone(),
    };

    let component_dependency = ComponentDependency::new(
        component_dependency_key,
        component.metadata.exports().to_vec(),
    );

    let compiler_config = RibCompilerConfig::new(vec![component_dependency], vec![], vec![]);

    let rib_function_invoke = Arc::new(TestRibFunctionInvoke::new(user));

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

    Ok(())
}

async fn test_rib_for_loop(
    deps: &EnvBasedTestDependencies,
    worker_name: Option<&str>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user.component(&env.id, "shopping-cart").store().await?;

    let component_dependency_key = ComponentDependencyKey {
        component_name: "shopping-cart".to_string(),
        component_id: component.id.0,
        component_revision: 0,
        root_package_name: component.metadata.root_package_name().clone(),
        root_package_version: component.metadata.root_package_version().clone(),
    };

    let component_dependency = ComponentDependency::new(
        component_dependency_key,
        component.metadata.exports().to_vec(),
    );

    let compiler_config = RibCompilerConfig::new(vec![component_dependency], vec![], vec![]);

    let rib_function_invoke = Arc::new(TestRibFunctionInvoke::new(user));

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

    Ok(())
}

async fn test_rib_with_resource_methods(
    deps: &EnvBasedTestDependencies,
    worker_name: Option<&str>,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "shopping-cart-resource")
        .store()
        .await?;

    let component_dependency_key = ComponentDependencyKey {
        component_name: "shopping-cart".to_string(),
        component_id: component.id.0,
        component_revision: 0,
        root_package_name: component.metadata.root_package_name().clone(),
        root_package_version: component.metadata.root_package_version().clone(),
    };

    let component_dependency = ComponentDependency::new(
        component_dependency_key,
        component.metadata.exports().to_vec(),
    );

    let compiler_config = RibCompilerConfig::new(vec![component_dependency], vec![], vec![]);

    let rib_function_invoke = Arc::new(TestRibFunctionInvoke::new(user));

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

    Ok(())
}

struct TestRibFunctionInvoke {
    test_dsl: TestUserContext<EnvBasedTestDependencies>,
}

impl TestRibFunctionInvoke {
    fn new(test_dsl: TestUserContext<EnvBasedTestDependencies>) -> Self {
        Self { test_dsl }
    }
}

#[async_trait]
impl RibComponentFunctionInvoke for TestRibFunctionInvoke {
    async fn invoke(
        &self,
        component_dependency_key: ComponentDependencyKey,
        _instruction_id: &InstructionId,
        worker_name: EvaluatedWorkerName,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
        _return_type: Option<AnalysedType>,
    ) -> RibFunctionInvokeResult {
        let worker_id = WorkerId {
            component_id: ComponentId(component_dependency_key.component_id),
            worker_name: worker_name.0,
        };

        let result = self
            .test_dsl
            .invoke_and_await_typed(&worker_id, function_name.0.as_str(), args.0)
            .await
            .collapse();

        Ok(result.map_err(|err| {
            tracing::error!("Failed to invoke function: {:?}", err);
            anyhow!("Failed to invoke function: {:?}", err)
        })?)
    }
}
