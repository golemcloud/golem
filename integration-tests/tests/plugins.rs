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

use assert2::assert;
use assert2::let_assert;
use axum::body::Bytes;
use axum::extract::Multipart;
use axum::routing::post;
use axum::Router;
use base64::Engine;
use golem_api_grpc::proto::golem::worker::{log_event, Log};
use golem_client::api::{RegistryServiceClient, RegistryServiceCreateComponentError};
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::base64::Base64;
use golem_common::model::component::{ComponentFilePath, ComponentFilePermissions};
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::plugin_registration::{
    ComponentTransformerPluginSpec, OplogProcessorPluginSpec, PluginRegistrationCreation,
    PluginSpecDto,
};
use golem_common::model::{Empty, ScanCursor};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use golem_test_framework::model::IFSEntry;
use golem_wasm::analysis::{AnalysedExport, AnalysedInstance};
use golem_wasm::{IntoValueAndType, Record, Value};
use reqwest::StatusCode;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use test_r::{inherit_test_dep, test};
use tracing::{debug, info};
use wac_graph::types::Package;
use wac_graph::{plug, CompositionGraph, EncodeOptions, Processor};

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn component_transformer1(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    fn transform_component(component: Bytes) -> anyhow::Result<Vec<u8>> {
        let mut graph = CompositionGraph::new();
        let component = Package::from_bytes("component", None, component, graph.types_mut())?;
        let component = graph.register_package(component)?;

        let adapter_bytes =
            include_bytes!("../../test-components/component-transformer1-adapter.wasm");

        let adapter = Package::from_bytes("adapter", None, adapter_bytes, graph.types_mut())?;
        let adapter = graph.register_package(adapter)?;

        plug(&mut graph, vec![adapter], component)?;

        let transformed_bytes = graph.encode(EncodeOptions {
            processor: Some(Processor {
                name: "component-transformer-example1",
                version: "0.1.0",
            }),
            ..Default::default()
        })?;

        Ok(transformed_bytes)
    }

    async fn transform(mut multipart: Multipart) -> axum::Json<serde_json::Value> {
        let mut component = None;

        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap().to_string();
            let data = field.bytes().await.unwrap();
            debug!("Length of `{}` is {} bytes", name, data.len());

            match name.as_str() {
                "component" => {
                    component = Some(data);
                }
                "metadata" => {
                    let json =
                        std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
                    info!("Metadata: {}", json);
                }
                _ => {
                    let value = std::str::from_utf8(&data).expect("Failed to parse field as UTF-8");
                    info!("Configuration field: {} = {}", name, value);
                }
            }
        }

        let transformed_bytes =
            transform_component(component.expect("did not receive a component part"))
                .expect("Failed to transform component");

        let data_base64 = base64::engine::general_purpose::STANDARD.encode(&transformed_bytes);

        let response = json!({
            "data": data_base64
        });

        axum::Json(response)
    }

    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_transformer_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "component-transformer-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let component_transformer_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: component_transformer_plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "logging")
        .with_plugin(&component_transformer_plugin_grant.id, 0)
        .store()
        .await?;

    let worker = user.start_worker(&component.id, "worker1").await?;
    let mut rx = user.capture_output(&worker).await?;

    user.invoke_and_await(&worker, "golem:it/api.{some-random-entries}", vec![])
        .await?;

    let mut events = vec![];
    rx.recv_many(&mut events, 100).await;

    server_handle.abort();

    let log_events: Vec<Log> = events
        .into_iter()
        .filter_map(|event| {
            if let Some(log_event::Event::Log(log)) = event.event {
                Some(log)
            } else {
                None
            }
        })
        .collect();

    assert!(log_events
        .iter()
        .all(|log| log.context.contains("custom-context")));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn component_transformer2(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    fn transform_component(plug_bytes: Bytes) -> anyhow::Result<Vec<u8>> {
        let mut graph = CompositionGraph::new();
        let plug = Package::from_bytes("component", None, plug_bytes, graph.types_mut())?;
        let plug = graph.register_package(plug)?;

        let socket_bytes = include_bytes!("../../test-components/app_and_library_app.wasm");

        let socket = Package::from_bytes("socket", None, socket_bytes, graph.types_mut())?;
        let socket = graph.register_package(socket)?;

        wac_graph::plug(&mut graph, vec![plug], socket)?;

        let transformed_bytes = graph.encode(EncodeOptions {
            processor: Some(Processor {
                name: "component-transformer-example1",
                version: "0.1.0",
            }),
            ..Default::default()
        })?;

        Ok(transformed_bytes)
    }

    async fn transform(mut multipart: Multipart) -> axum::Json<serde_json::Value> {
        let mut component = None;

        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap().to_string();
            let data = field.bytes().await.unwrap();
            debug!("Length of `{}` is {} bytes", name, data.len());

            match name.as_str() {
                "component" => {
                    component = Some(data);
                }
                "metadata" => {
                    let json =
                        std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
                    info!("Metadata: {}", json);
                }
                _ => {
                    let value = std::str::from_utf8(&data).expect("Failed to parse field as UTF-8");
                    info!("Configuration field: {} = {}", name, value);
                }
            }
        }

        let transformed_bytes =
            transform_component(component.expect("did not receive a component part"))
                .expect("Failed to transform component");

        let data_base64 = base64::engine::general_purpose::STANDARD.encode(&transformed_bytes);

        let response = json!({
            "data": data_base64
        });

        axum::Json(response)
    }

    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_transformer_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "component-transformer-2".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let component_transformer_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: component_transformer_plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "app_and_library_library")
        .with_plugin(&component_transformer_plugin_grant.id, 0)
        .store()
        .await?;

    server_handle.abort();

    {
        let exports = component.metadata.exports();

        assert_eq!(exports.len(), 1);
        assert!(matches!(
            &exports[0],
            AnalysedExport::Instance(AnalysedInstance {
                name,
                ..
            }) if name == "it:app-and-library-app/app-api"
        ));
    }

    let worker = user.start_worker(&component.id, "worker1").await?;

    let response = user
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await?;

    assert_eq!(response, vec![Value::U64(2)]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn component_transformer_env_var(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    async fn transform(mut multipart: Multipart) -> axum::Json<serde_json::Value> {
        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap().to_string();
            let data = field.bytes().await.unwrap();
            debug!("Length of `{}` is {} bytes", name, data.len());

            match name.as_str() {
                "component" => {
                    info!("Received component data");
                }
                "metadata" => {
                    let json =
                        std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
                    info!("Metadata: {}", json);
                }
                _ => {
                    let value = std::str::from_utf8(&data).expect("Failed to parse field as UTF-8");
                    info!("Configuration field: {} = {}", name, value);
                }
            }
        }

        let response = json!({
            "env": {
                "TEST_ENV_VAR_2": "value_2"
            }
        });

        axum::Json(response)
    }

    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;
    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_transformer_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "component-transformer-env".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let component_transformer_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: component_transformer_plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "environment-service")
        .with_env(vec![("TEST_ENV_VAR_1".to_string(), "value_1".to_string())])
        .with_plugin(&component_transformer_plugin_grant.id, 0)
        .store()
        .await?;

    server_handle.abort();

    let worker = user
        .start_worker_with(
            &component.id,
            "worker1",
            HashMap::from_iter(vec![("TEST_ENV_VAR_3".to_string(), "value_3".to_string())]),
            vec![],
        )
        .await?;

    let response = user
        .invoke_and_await(&worker, "golem:it/api.{get-environment}", vec![])
        .await?;

    let response_map = {
        assert_eq!(response.len(), 1);

        let_assert!(Value::Result(Ok(Some(response))) = &response[0]);
        let_assert!(Value::List(response) = response.as_ref());
        response
            .iter()
            .map(|env_var| {
                let_assert!(Value::Tuple(elems) = env_var);
                let_assert!([Value::String(key), Value::String(value)] = elems.as_slice());
                (key.to_owned(), value.to_owned())
            })
            .collect::<HashMap<_, _>>()
    };

    assert_eq!(
        response_map.get("TEST_ENV_VAR_1"),
        Some(&"value_1".to_string())
    );
    assert_eq!(
        response_map.get("TEST_ENV_VAR_2"),
        Some(&"value_2".to_string())
    );
    assert_eq!(
        response_map.get("TEST_ENV_VAR_3"),
        Some(&"value_3".to_string())
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn component_transformer_ifs(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    async fn transform(mut multipart: Multipart) -> axum::Json<serde_json::Value> {
        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap().to_string();
            let data = field.bytes().await.unwrap();
            debug!("Length of `{}` is {} bytes", name, data.len());

            match name.as_str() {
                "component" => {
                    info!("Received component data");
                }
                "metadata" => {
                    let json =
                        std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
                    info!("Metadata: {}", json);
                }
                _ => {
                    let value = std::str::from_utf8(&data).expect("Failed to parse field as UTF-8");
                    info!("Configuration field: {} = {}", name, value);
                }
            }
        }

        let file_content_base64 = base64::engine::general_purpose::STANDARD.encode("foobar");

        let response = json!({
            "additionalFiles": [
                {
                    "path": "/files/foo.txt",
                    "permissions": "read-only",
                    "content": file_content_base64
                }
            ]
        });

        axum::Json(response)
    }

    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_transformer_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "component-transformer-ifs".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let component_transformer_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: component_transformer_plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "file-service")
        .with_files(&[IFSEntry {
            source_path: PathBuf::from("ifs-update/files/bar.txt"),
            target_path: ComponentFilePath::from_abs_str("/files/bar.txt").unwrap(),
            permissions: ComponentFilePermissions::ReadOnly,
        }])
        .with_plugin(&component_transformer_plugin_grant.id, 0)
        .store()
        .await?;

    server_handle.abort();

    let worker_id = user.start_worker(&component.id, "worker1").await?;

    let result_foo = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec!["/files/foo.txt".into_value_and_type()],
        )
        .await?;

    assert_eq!(
        result_foo,
        vec![Value::Result(Ok(Some(Box::new(Value::String(
            "foobar".to_string()
        )))))]
    );

    let result_bar = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec!["/files/bar.txt".into_value_and_type()],
        )
        .await?;

    assert_eq!(
        result_bar,
        vec![Value::Result(Ok(Some(Box::new(Value::String(
            "bar\n".to_string()
        )))))]
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn component_transformer_failed(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    async fn transform() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let component_transformer_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "component-transformer-failed".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::ComponentTransformer(ComponentTransformerPluginSpec {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let component_transformer_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: component_transformer_plugin.id,
            },
        )
        .await?;

    let result = user
        .component(&env.id, "logging")
        .with_plugin(&component_transformer_plugin_grant.id, 0)
        .store()
        .await;

    server_handle.abort();

    let_assert!(Err(error) = result);
    let downcasted = error
        .downcast_ref::<golem_client::Error<RegistryServiceCreateComponentError>>()
        .unwrap();

    let_assert!(
        golem_client::Error::Item(RegistryServiceCreateComponentError::Error400(inner_error)) =
            downcasted
    );

    assert!(inner_error.errors == vec!["Component transformer plugin with priority 0 failed with: HTTP status: 500 Internal Server Error"]);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn oplog_processor(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let plugin_component = user.component(&env.id, "oplog-processor").store().await?;

    let oplog_processor_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let oplog_processor_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "shopping-cart")
        .with_plugin(&oplog_processor_plugin_grant.id, 0)
        .store()
        .await?;

    let worker_id = user.start_worker(&component.id, "worker1").await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{initialize-cart}",
        vec!["test-user-1".into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1000".into_value_and_type()),
            ("name", "Golem T-Shirt M".into_value_and_type()),
            ("price", 100.0f32.into_value_and_type()),
            ("quantity", 5u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1001".into_value_and_type()),
            ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
            ("price", 999999.0f32.into_value_and_type()),
            ("quantity", 1u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{add-item}",
        vec![Record(vec![
            ("product-id", "G1002".into_value_and_type()),
            ("name", "Mud Golem".into_value_and_type()),
            ("price", 11.0f32.into_value_and_type()),
            ("quantity", 10u32.into_value_and_type()),
        ])
        .into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{update-item-quantity}",
        vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
    )
    .await?;

    user.invoke_and_await(
        &worker_id,
        "golem:it/api.{force-commit}",
        vec![10u8.into_value_and_type()],
    )
    .await?;

    let mut plugin_worker_id = None;
    let mut cursor = ScanCursor::default();

    loop {
        let (maybe_cursor, items) = user
            .get_workers_metadata(&plugin_component.id, None, cursor, 1, true)
            .await?;

        for item in items {
            if plugin_worker_id.is_none() {
                plugin_worker_id = Some(item.worker_id.clone());
            }
        }

        if plugin_worker_id.is_some() {
            break;
        }

        if let Some(new_cursor) = maybe_cursor {
            cursor = new_cursor;
        } else {
            break;
        }
    }

    let plugin_worker_id = plugin_worker_id.expect("Plugin worker id found");

    let mut invocations = Vec::new();

    loop {
        let response = user
            .invoke_and_await(
                &plugin_worker_id,
                "golem:component/api.{get-invoked-functions}",
                vec![],
            )
            .await?;

        if let Value::List(items) = &response[0] {
            invocations.extend(items.iter().filter_map(|item| {
                if let Value::String(name) = item {
                    Some(name.clone())
                } else {
                    None
                }
            }));
        }

        if !invocations.is_empty() {
            break;
        }
    }

    let account_id = user.account_id;
    let component_id = component.id;

    let expected = vec![
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{initialize-cart}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{update-item-quantity}}"),
    ];
    assert_eq!(invocations, expected);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn library_plugin(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let library_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "library-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::Library(Empty {}),
            },
            Some(
                tokio::fs::read(
                    deps.component_directory()
                        .join("app_and_library_library.wasm"),
                )
                .await?,
            ),
        )
        .await?;

    let library_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: library_plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "app_and_library_app")
        .with_plugin(&library_plugin_grant.id, 0)
        .store()
        .await?;

    let worker = user.start_worker(&component.id, "worker1").await?;

    let response = user
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await?;

    assert!(response == vec![Value::U64(2)]);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn app_plugin(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    let library_plugin = client
        .create_plugin(
            &user.account_id.0,
            &PluginRegistrationCreation {
                name: "app-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::App(Empty {}),
            },
            Some(
                tokio::fs::read(deps.component_directory().join("app_and_library_app.wasm"))
                    .await?,
            ),
        )
        .await?;

    let library_plugin_grant = client
        .create_environment_plugin_grant(
            &env.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: library_plugin.id,
            },
        )
        .await?;

    let component = user
        .component(&env.id, "app_and_library_library")
        .with_plugin(&library_plugin_grant.id, 0)
        .store()
        .await?;

    let worker = user.start_worker(&component.id, "worker1").await?;

    let response = user
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await?;

    assert!(response == vec![Value::U64(2)]);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn oplog_processor_in_different_env_after_unregistering(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user_1 = deps.user().await?;
    let (_, env_1) = user_1.app_and_env().await?;

    let user_2 = deps.user().await?;
    let client_2 = user_2.registry_service_client().await;

    user_1
        .share_environment(&env_1.id, &user_2.account_id, &[EnvironmentRole::Admin])
        .await?;

    let plugin_component = user_2
        .component(&env_1.id, "oplog-processor")
        .store()
        .await?;

    let oplog_processor_plugin = client_2
        .create_plugin(
            &user_2.account_id.0,
            &PluginRegistrationCreation {
                name: "oplog-processor-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: Base64(Vec::new()),
                homepage: "none".to_string(),
                spec: PluginSpecDto::OplogProcessor(OplogProcessorPluginSpec {
                    component_id: plugin_component.id,
                    component_revision: plugin_component.revision,
                }),
            },
            None::<Vec<u8>>,
        )
        .await?;

    let oplog_processor_plugin_grant = client_2
        .create_environment_plugin_grant(
            &env_1.id.0,
            &EnvironmentPluginGrantCreation {
                plugin_registration_id: oplog_processor_plugin.id,
            },
        )
        .await?;

    let component = user_1
        .component(&env_1.id, "shopping-cart")
        .with_plugin(&oplog_processor_plugin_grant.id, 0)
        .store()
        .await?;

    client_2
        .delete_environment_plugin_grant(&oplog_processor_plugin_grant.id.0)
        .await?;
    client_2.delete_plugin(&oplog_processor_plugin.id.0).await?;

    let worker_id = user_1.start_worker(&component.id, "worker1").await?;

    user_1
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await?;

    user_1
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    user_1
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    user_1
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Record(vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ])
            .into_value_and_type()],
        )
        .await?;

    user_1
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await?;

    user_1
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{force-commit}",
            vec![10u8.into_value_and_type()],
        )
        .await?;

    let mut plugin_worker_id = None;
    let mut cursor = ScanCursor::default();

    loop {
        let (maybe_cursor, items) = user_1
            .get_workers_metadata(&plugin_component.id, None, cursor, 1, true)
            .await?;

        for item in items {
            if plugin_worker_id.is_none() {
                plugin_worker_id = Some(item.worker_id.clone());
            }
        }

        if plugin_worker_id.is_some() {
            break;
        }

        if let Some(new_cursor) = maybe_cursor {
            cursor = new_cursor;
        } else {
            break;
        }
    }

    let plugin_worker_id = plugin_worker_id.expect("Plugin worker id found");

    let mut invocations = Vec::new();

    loop {
        let response = user_1
            .invoke_and_await(
                &plugin_worker_id,
                "golem:component/api.{get-invoked-functions}",
                vec![],
            )
            .await?;

        if let Value::List(items) = &response[0] {
            invocations.extend(items.iter().filter_map(|item| {
                if let Value::String(name) = item {
                    Some(name.clone())
                } else {
                    None
                }
            }));
        }

        if !invocations.is_empty() {
            break;
        }
    }

    let account_id = user_1.account_id;
    let component_id = component.id;

    let expected = vec![
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{initialize-cart}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{update-item-quantity}}"),
    ];
    assert_eq!(invocations, expected);

    Ok(())
}
