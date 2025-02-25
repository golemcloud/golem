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

use golem_common::tracing::init_tracing_with_default_env_filter;
use golem_service_base::migration::MigrationsDir;
use golem_worker_service::config::make_config_loader;
use golem_worker_service::WorkerService;
use golem_worker_service_base::app_config::WorkerServiceBaseConfig;
use golem_worker_service_base::metrics;
use opentelemetry::global;
use prometheus::Registry;
use tokio::task::JoinSet;
use golem_worker_service_base::gateway_rib_compiler::{DefaultWorkerServiceRibCompiler, WorkerServiceRibCompiler};
use rib::{compile, Expr};

fn main() -> Result<(), anyhow::Error> {
    let expr = r#"
                let x = request.body.user-id;
                let worker = instance(x);
                let cart1 = worker.cart("bar");
                cart1.add-item("foo");
                "success"
            "#;
    let expr = Expr::from_text(expr).unwrap();
    let component_metadata = internal::get_metadata_with_resource_with_params();

    let compiled = rib::compile(&expr, &component_metadata).unwrap();

    Ok(())
}

async fn async_main(
    config: WorkerServiceBaseConfig,
    prometheus: Registry,
) -> Result<(), anyhow::Error> {
    let server = WorkerService::new(
        config,
        prometheus,
        MigrationsDir::new("./db/migration".into()),
    )
    .await?;

    let mut join_set = JoinSet::new();

    server.run(&mut join_set).await?;

    while let Some(res) = join_set.join_next().await {
        res??;
    }

    Ok(())
}

async fn dump_openapi_yaml() -> Result<(), anyhow::Error> {
    let config = WorkerServiceBaseConfig::default();
    let service = WorkerService::new(
        config,
        Registry::default(),
        MigrationsDir::new("../../golem-worker-service/db/migration".into()),
    )
    .await?;
    let yaml = service.http_service().spec_yaml();
    println!("{yaml}");
    Ok(())
}

mod internal {

    use async_trait::async_trait;
    use golem_wasm_ast::analysis::analysed_type::{
        case, f32, field, handle, list, option, r#enum, record, result, str, tuple, u32, u64,
        unit_case, variant,
    };
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
        AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysedType,
    };
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
    use std::sync::Arc;
    pub(crate) fn get_metadata_with_resource_with_params() -> Vec<AnalysedExport> {
        get_metadata_with_resource(vec![AnalysedFunctionParameter {
            name: "user-id".to_string(),
            typ: str(),
        }])
    }

    fn get_metadata_with_resource(
        resource_constructor_params: Vec<AnalysedFunctionParameter>,
    ) -> Vec<AnalysedExport> {
        let instance = AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![
                AnalysedFunction {
                    name: "[constructor]cart".to_string(),
                    parameters: resource_constructor_params,
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                    }],
                },
                AnalysedFunction {
                    name: "[method]cart.add-item".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        },
                        AnalysedFunctionParameter {
                            name: "item".to_string(),
                            typ: str(),
                        },
                    ],
                    results: vec![],
                }
            ],
        });

        vec![instance]
    }
}