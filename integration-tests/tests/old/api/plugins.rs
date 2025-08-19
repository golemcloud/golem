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
use assert2::let_assert;
use axum::body::Bytes;
use axum::extract::Multipart;
use axum::routing::post;
use axum::Router;
use base64::Engine;
use golem_api_grpc::proto::golem::worker::{log_event, Log};
use golem_client::api::{ComponentClient, PluginClient};
use golem_client::model::BatchPluginInstallationUpdates;
use golem_common::model::plugin::{
    AppPluginDefinition, ComponentTransformerDefinition, LibraryPluginDefinition,
    OplogProcessorDefinition, PluginInstallationAction, PluginInstallationCreation,
    PluginInstallationUpdateWithId, PluginTypeSpecificDefinition, PluginUninstallation,
};
use golem_common::model::plugin::{PluginScope, ProjectPluginScope};
use golem_common::model::{ComponentFilePermissions, Empty, ScanCursor};
use golem_test_framework::config::{
    EnvBasedTestDependencies, TestDependencies, TestDependenciesDsl,
};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_test_framework::model::PluginDefinitionCreation;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedInstance};
use golem_wasm_rpc::{IntoValueAndType, Record, Value};
use reqwest::StatusCode;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use test_r::{inherit_test_dep, tag, test};
use tracing::{debug, info};
use wac_graph::types::Package;
use wac_graph::{plug, CompositionGraph, EncodeOptions, Processor};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
async fn component_transformer1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    fn transform_component(component: Bytes) -> anyhow::Result<Vec<u8>> {
        let mut graph = CompositionGraph::new();
        let component = Package::from_bytes("component", None, component, graph.types_mut())?;
        let component = graph.register_package(component)?;

        let adapter_bytes =
            include_bytes!("../../../test-components/component-transformer1-adapter.wasm");

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

    let admin = deps.admin().await;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_id = admin.component("logging").unique().store().await;

    admin
        .create_plugin(PluginDefinitionCreation {
            name: "component-transformer-1".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::ComponentTransformer(
                ComponentTransformerDefinition {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                },
            ),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    let _installation_id = admin
        .install_plugin_to_component(
            &component_id,
            "component-transformer-1",
            "v1",
            0,
            HashMap::new(),
        )
        .await;

    let worker = admin.start_worker(&component_id, "worker1").await;
    let mut rx = admin.capture_output(&worker).await;

    let _ = admin
        .invoke_and_await(&worker, "golem:it/api.{some-random-entries}", vec![])
        .await;

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
}

#[test]
async fn component_transformer2(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    fn transform_component(plug_bytes: Bytes) -> anyhow::Result<Vec<u8>> {
        let mut graph = CompositionGraph::new();
        let plug = Package::from_bytes("component", None, plug_bytes, graph.types_mut())?;
        let plug = graph.register_package(plug)?;

        let socket_bytes = include_bytes!("../../../test-components/app_and_library_app.wasm");

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

    let admin = deps.admin().await;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_id = admin
        .component("app_and_library_library")
        .unique()
        .store()
        .await;

    admin
        .create_plugin(PluginDefinitionCreation {
            name: "component-transformer-2".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::ComponentTransformer(
                ComponentTransformerDefinition {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                },
            ),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    admin
        .install_plugin_to_component(
            &component_id,
            "component-transformer-2",
            "v1",
            0,
            HashMap::new(),
        )
        .await;

    server_handle.abort();

    let patched_component_metadata = admin.get_latest_component_metadata(&component_id).await;

    let exports = patched_component_metadata.exports();

    assert_eq!(exports.len(), 1);
    assert!(matches!(
        &exports[0],
        AnalysedExport::Instance(AnalysedInstance {
            name,
            ..
        }) if name == "it:app-and-library-app/app-api"
    ));

    let worker = admin.start_worker(&component_id, "worker1").await;

    let response = admin
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]));
}

#[test]
async fn component_transformer_env_var(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
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

    let admin = deps.admin().await;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_id = admin
        .component("environment-service")
        .unique()
        .with_env(vec![("TEST_ENV_VAR_1".to_string(), "value_1".to_string())])
        .store()
        .await;

    admin
        .create_plugin(PluginDefinitionCreation {
            name: "component-transformer-env".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::ComponentTransformer(
                ComponentTransformerDefinition {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                },
            ),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    admin
        .install_plugin_to_component(
            &component_id,
            "component-transformer-env",
            "v1",
            0,
            HashMap::new(),
        )
        .await;

    server_handle.abort();

    let worker = admin
        .start_worker_with(
            &component_id,
            "worker1",
            vec![],
            HashMap::from_iter(vec![("TEST_ENV_VAR_3".to_string(), "value_3".to_string())]),
            vec![],
        )
        .await;

    let response = admin
        .invoke_and_await(&worker, "golem:it/api.{get-environment}", vec![])
        .await
        .unwrap();

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
}

#[test]
async fn component_transformer_ifs(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
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

    let admin = deps.admin().await;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_files = admin
        .add_initial_component_files(&[(
            "ifs-update/files/bar.txt",
            "/files/bar.txt",
            ComponentFilePermissions::ReadOnly,
        )])
        .await;

    let component_id = admin
        .component("file-service")
        .unique()
        .with_files(&component_files)
        .store()
        .await;

    admin
        .create_plugin(PluginDefinitionCreation {
            name: "component-transformer-ifs".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::ComponentTransformer(
                ComponentTransformerDefinition {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                },
            ),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    admin
        .install_plugin_to_component(
            &component_id,
            "component-transformer-ifs",
            "v1",
            0,
            HashMap::new(),
        )
        .await;

    server_handle.abort();

    let worker_id = admin.start_worker(&component_id, "worker1").await;

    let result_foo = admin
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec!["/files/foo.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    assert_eq!(
        result_foo,
        vec![Value::Result(Ok(Some(Box::new(Value::String(
            "foobar".to_string()
        )))))]
    );

    let result_bar = admin
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{read-file}",
            vec!["/files/bar.txt".into_value_and_type()],
        )
        .await
        .unwrap();

    assert_eq!(
        result_bar,
        vec![Value::Result(Ok(Some(Box::new(Value::String(
            "bar\n".to_string()
        )))))]
    );
}

#[test]
async fn component_transformer_failed(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    async fn transform() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    let admin = deps.admin().await;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();

    let port = listener.local_addr().unwrap().port();

    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let component_id = admin.component("logging").unique().store().await;

    admin
        .create_plugin(PluginDefinitionCreation {
            name: "component-transformer-failed".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::ComponentTransformer(
                ComponentTransformerDefinition {
                    provided_wit_package: None,
                    json_schema: None,
                    validate_url: "not-used".to_string(),
                    transform_url: format!("http://localhost:{port}/transform"),
                },
            ),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    let result = <TestDependenciesDsl<_> as golem_test_framework::dsl::TestDsl>::
        install_plugin_to_component(
            &admin,
            &component_id,
            "component-transformer-failed",
            "v1",
            0,
            HashMap::new(),
        )
        .await;

    server_handle.abort();

    assert!(matches!(
        result,
        Err(inner) if inner.to_string().contains("Component transformation failed: HTTP status: 500")
    ));
}

#[test]
async fn oplog_processor_global_scope(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let user = deps.user().await;

    let plugin_component_id = user.component("oplog-processor").unique().store().await;
    let component_id = user.component("shopping-cart").unique().store().await;

    user.create_plugin(PluginDefinitionCreation {
        name: "oplog-processor-1".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: plugin_component_id.clone(),
            component_version: 0,
        }),
        scope: PluginScope::Global(Empty {}),
    })
    .await;

    let _installation_id = user
        .install_plugin_to_component(&component_id, "oplog-processor-1", "v1", 0, HashMap::new())
        .await;

    let worker_id = user.start_worker(&component_id, "worker1").await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = user
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
        .await;

    let _ = user
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
        .await;

    let _ = user
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
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{force-commit}",
            vec![10u8.into_value_and_type()],
        )
        .await;

    let mut plugin_worker_id = None;
    let mut cursor = ScanCursor::default();

    loop {
        let (maybe_cursor, items) = user
            .get_workers_metadata(&plugin_component_id, None, cursor, 1, true)
            .await;

        for (item, _) in items {
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
            .await
            .unwrap();

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

    let expected = vec![
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{initialize-cart}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{update-item-quantity}}"),
    ];
    assert_eq!(invocations, expected);
}

#[test]
async fn oplog_processor_project_scope(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let user = deps.user().await;
    let project = user.create_project().await;

    let plugin_component_id = user.component("oplog-processor").unique().store().await;
    let component_id = user
        .component("shopping-cart")
        .unique()
        .with_project(project.clone())
        .store()
        .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "oplog-processor-2".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: plugin_component_id.clone(),
            component_version: 0,
        }),
        scope: PluginScope::Project(ProjectPluginScope {
            project_id: project.clone(),
        }),
    })
    .await;

    let _installation_id = user
        .install_plugin_to_component(&component_id, "oplog-processor-2", "v1", 0, HashMap::new())
        .await;

    let worker_id = user.start_worker(&component_id, "worker1").await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec!["test-user-1".into_value_and_type()],
        )
        .await;

    let _ = user
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
        .await;

    let _ = user
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
        .await;

    let _ = user
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
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec!["G1002".into_value_and_type(), 20u32.into_value_and_type()],
        )
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{force-commit}",
            vec![10u8.into_value_and_type()],
        )
        .await;

    let mut plugin_worker_id = None;
    let mut cursor = ScanCursor::default();

    loop {
        let (maybe_cursor, items) = user
            .get_workers_metadata(&plugin_component_id, None, cursor, 1, true)
            .await;

        for (item, _) in items {
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
            .await
            .unwrap();

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

    let expected = vec![
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{initialize-cart}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("{account_id}/{component_id}/worker1/golem:it/api.{{update-item-quantity}}"),
    ];
    assert_eq!(invocations, expected);
}

#[test]
async fn library_plugin(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;
    let component_id = admin
        .component("app_and_library_app")
        .unique()
        .store()
        .await;

    let plugin_wasm_key = admin.add_plugin_wasm("app_and_library_library").await;

    admin
        .create_plugin(PluginDefinitionCreation {
            name: "library-plugin-1".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
                blob_storage_key: plugin_wasm_key,
            }),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    let _installation_id = admin
        .install_plugin_to_component(&component_id, "library-plugin-1", "v1", 0, HashMap::new())
        .await;

    let worker = admin.start_worker(&component_id, "worker1").await;

    let response = admin
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]))
}

#[test]
async fn app_plugin(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;

    let component_id = admin
        .component("app_and_library_library")
        .unique()
        .store()
        .await;

    let plugin_wasm_key = admin.add_plugin_wasm("app_and_library_app").await;

    admin
        .create_plugin(PluginDefinitionCreation {
            name: "app-plugin-1".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::App(AppPluginDefinition {
                blob_storage_key: plugin_wasm_key,
            }),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    let _installation_id = admin
        .install_plugin_to_component(&component_id, "app-plugin-1", "v1", 0, HashMap::new())
        .await;

    let worker = admin.start_worker(&component_id, "worker1").await;

    let response = admin
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]))
}

/// Test that a plugin can be recreated after deleting it
#[test]
async fn recreate_plugin(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let admin = deps.admin().await;

    let component_id = admin
        .component("app_and_library_app")
        .unique()
        .store()
        .await;

    let plugin_wasm_key = admin.add_plugin_wasm("app_and_library_library").await;

    let plugin_definition = PluginDefinitionCreation {
        name: "library-plugin-2".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
            blob_storage_key: plugin_wasm_key,
        }),
        scope: PluginScope::Global(Empty {}),
    };

    admin.create_plugin(plugin_definition.clone()).await;

    admin
        .delete_plugin(admin.account_id.clone(), "library-plugin-2", "v1")
        .await;

    admin.create_plugin(plugin_definition.clone()).await;

    let _installation_id = admin
        .install_plugin_to_component(&component_id, "library-plugin-2", "v1", 0, HashMap::new())
        .await;

    let worker = admin.start_worker(&component_id, "worker1").await;

    let response = admin
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]))
}

/// Test that a component can be invoked after a plugin is unregistered that it depends on
#[test]
async fn invoke_after_deleting_plugin(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let user = deps.user().await;

    let component_id = user.component("app_and_library_app").unique().store().await;

    let plugin_wasm_key = user.add_plugin_wasm("app_and_library_library").await;

    let plugin_definition = PluginDefinitionCreation {
        name: "library-plugin-3".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
            blob_storage_key: plugin_wasm_key,
        }),
        scope: PluginScope::Global(Empty {}),
    };

    user.create_plugin(plugin_definition.clone()).await;

    let _installation_id = user
        .install_plugin_to_component(&component_id, "library-plugin-3", "v1", 0, HashMap::new())
        .await;

    user.delete_plugin(user.account_id.clone(), "library-plugin-3", "v1")
        .await;

    let worker = user.start_worker(&component_id, "worker1").await;

    let response = user
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]))
}

#[test]
#[tag(http_only)]
async fn querying_plugins_return_only_plugins_valid_in_scope(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    let user = deps.user().await;

    let project_1 = user.create_project().await;
    let project_2 = user.create_project().await;

    let plugin_component_id = user.component("oplog-processor").unique().store().await;
    let component_id = user
        .component("shopping-cart")
        .unique()
        .with_project(project_1.clone())
        .store()
        .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "oplog-processor-1".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: plugin_component_id.clone(),
            component_version: 0,
        }),
        scope: PluginScope::Global(Empty {}),
    })
    .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "oplog-processor-2".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: plugin_component_id.clone(),
            component_version: 0,
        }),
        scope: PluginScope::project(project_1.clone()),
    })
    .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "oplog-processor-3".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: plugin_component_id.clone(),
            component_version: 0,
        }),
        scope: PluginScope::project(project_2.clone()),
    })
    .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "oplog-processor-4".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: plugin_component_id.clone(),
            component_version: 0,
        }),
        scope: PluginScope::component(component_id.clone()),
    })
    .await;

    // querying for project should only return plugins in the project and global scope
    {
        let mut plugins = deps
            .component_service()
            .plugin_http_client(&user.token)
            .await
            .list_plugins(&PluginScope::project(project_1.clone()))
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect::<Vec<_>>();
        plugins.sort();

        assert_eq!(
            plugins,
            vec![
                "oplog-processor-1".to_string(),
                "oplog-processor-2".to_string()
            ]
        );
    }

    // querying for component should only return plugins in the component, the owning project and global scope
    {
        let mut plugins = deps
            .component_service()
            .plugin_http_client(&user.token)
            .await
            .list_plugins(&PluginScope::component(component_id.clone()))
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect::<Vec<_>>();
        plugins.sort();

        assert_eq!(
            plugins,
            vec![
                "oplog-processor-1".to_string(),
                "oplog-processor-2".to_string(),
                "oplog-processor-4".to_string()
            ]
        );
    }

    // a user the project was shared with can also query the plugins
    {
        let user_2 = deps.user().await;
        user.grant_full_project_access(&project_1, &user_2.account_id)
            .await;

        // project scope
        {
            let mut plugins = deps
                .component_service()
                .plugin_http_client(&user_2.token)
                .await
                .list_plugins(&PluginScope::project(project_1))
                .await
                .unwrap()
                .into_iter()
                .map(|p| p.name)
                .collect::<Vec<_>>();
            plugins.sort();

            assert_eq!(
                plugins,
                vec![
                    "oplog-processor-1".to_string(),
                    "oplog-processor-2".to_string()
                ]
            );
        }

        // component scope
        {
            let mut plugins = deps
                .component_service()
                .plugin_http_client(&user_2.token)
                .await
                .list_plugins(&PluginScope::component(component_id))
                .await
                .unwrap()
                .into_iter()
                .map(|p| p.name)
                .collect::<Vec<_>>();
            plugins.sort();

            assert_eq!(
                plugins,
                vec![
                    "oplog-processor-1".to_string(),
                    "oplog-processor-2".to_string(),
                    "oplog-processor-4".to_string()
                ]
            );
        }
    }
}

#[test]
#[tag(http_only)]
async fn install_global_plugin_in_shared_project(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    // user 1 defines a project and a global plugin, user 2 installs the plugin to a component in the project

    let user_1 = deps.user().await;
    let user_2 = deps.user().await;

    let project = user_1.create_project().await;
    user_1
        .grant_full_project_access(&project, &user_2.account_id)
        .await;

    let plugin_wasm_key = user_1.add_plugin_wasm("app_and_library_library").await;

    user_1
        .create_plugin(PluginDefinitionCreation {
            name: "library-plugin-1".to_string(),
            version: "v1".to_string(),
            description: "A test".to_string(),
            icon: vec![],
            homepage: "none".to_string(),
            specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
                blob_storage_key: plugin_wasm_key,
            }),
            scope: PluginScope::Global(Empty {}),
        })
        .await;

    let component_id = user_2
        .component("app_and_library_app")
        .unique()
        .with_project(project.clone())
        .store()
        .await;

    let _installation_id = user_2
        .install_plugin_to_component(&component_id, "library-plugin-1", "v1", 0, HashMap::new())
        .await;

    let worker = user_2.start_worker(&component_id, "worker1").await;

    let response = user_2
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]))
}

#[test]
#[tag(http_only)]
async fn install_project_plugin_in_shared_project(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    // user 1 defines a project, user 2 defines and installs the plugin to a component in the project

    let user_1 = deps.user().await;
    let user_2 = deps.user().await;

    let project = user_1.create_project().await;
    user_1
        .grant_full_project_access(&project, &user_2.account_id)
        .await;

    // make sure the plugin is stored in the blobstorage of the user that will eventually end up owning it
    let plugin_wasm_key = user_1.add_plugin_wasm("app_and_library_library").await;

    deps.component_service()
        .create_plugin(
            &user_2.token,
            &user_1.account_id,
            PluginDefinitionCreation {
                name: "library-plugin-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: vec![],
                homepage: "none".to_string(),
                specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
                    blob_storage_key: plugin_wasm_key,
                }),
                scope: PluginScope::project(project.clone()),
            },
        )
        .await
        .unwrap();

    let component_id = user_2
        .component("app_and_library_app")
        .unique()
        .with_project(project.clone())
        .store()
        .await;

    let _installation_id = user_2
        .install_plugin_to_component(&component_id, "library-plugin-1", "v1", 0, HashMap::new())
        .await;

    let worker = user_2.start_worker(&component_id, "worker1").await;

    let response = user_2
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]))
}

#[test]
#[tag(http_only)]
async fn install_component_plugin_in_shared_project(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) {
    // user 1 defines a project, user 2 defines and installs the plugin to a component in the project

    let user_1 = deps.user().await;
    let user_2 = deps.user().await;

    let project = user_1.create_project().await;
    user_1
        .grant_full_project_access(&project, &user_2.account_id)
        .await;

    let plugin_wasm_key = user_1.add_plugin_wasm("app_and_library_library").await;

    deps.component_service()
        .create_plugin(
            &user_2.token,
            &user_1.account_id,
            PluginDefinitionCreation {
                name: "library-plugin-1".to_string(),
                version: "v1".to_string(),
                description: "A test".to_string(),
                icon: vec![],
                homepage: "none".to_string(),
                specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
                    blob_storage_key: plugin_wasm_key,
                }),
                scope: PluginScope::project(project.clone()),
            },
        )
        .await
        .unwrap();

    let component_id = user_2
        .component("app_and_library_app")
        .unique()
        .with_project(project.clone())
        .store()
        .await;

    let _installation_id = user_2
        .install_plugin_to_component(&component_id, "library-plugin-1", "v1", 0, HashMap::new())
        .await;

    let worker = user_2.start_worker(&component_id, "worker1").await;

    let response = user_2
        .invoke_and_await(
            &worker,
            "it:app-and-library-app/app-api.{app-function}",
            vec![],
        )
        .await;

    assert_eq!(response, Ok(vec![Value::U64(2)]))
}

#[test]
async fn batch_update_plugin_installations(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let user = deps.user().await;
    let component_id = user.component("app_and_library_app").unique().store().await;

    let plugin_wasm_key = user.add_plugin_wasm("app_and_library_library").await;

    user.create_plugin(PluginDefinitionCreation {
        name: "library-plugin-1".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
            blob_storage_key: plugin_wasm_key.clone(),
        }),
        scope: PluginScope::Global(Empty {}),
    })
    .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "library-plugin-2".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
            blob_storage_key: plugin_wasm_key.clone(),
        }),
        scope: PluginScope::Global(Empty {}),
    })
    .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "library-plugin-3".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
            blob_storage_key: plugin_wasm_key.clone(),
        }),
        scope: PluginScope::Global(Empty {}),
    })
    .await;

    user.create_plugin(PluginDefinitionCreation {
        name: "library-plugin-4".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::Library(LibraryPluginDefinition {
            blob_storage_key: plugin_wasm_key,
        }),
        scope: PluginScope::Global(Empty {}),
    })
    .await;

    let installation_id_1 = user
        .install_plugin_to_component(&component_id, "library-plugin-1", "v1", 0, HashMap::new())
        .await;

    let installation_id_2 = user
        .install_plugin_to_component(&component_id, "library-plugin-2", "v1", 1, HashMap::new())
        .await;

    let installation_id_3 = user
        .install_plugin_to_component(&component_id, "library-plugin-3", "v1", 2, HashMap::new())
        .await;

    deps.component_service()
        .component_http_client(&user.token)
        .await
        .batch_update_installed_plugins(
            &component_id.0,
            &BatchPluginInstallationUpdates {
                actions: vec![
                    PluginInstallationAction::Uninstall(PluginUninstallation {
                        installation_id: installation_id_2.clone(),
                    }),
                    PluginInstallationAction::Update(PluginInstallationUpdateWithId {
                        installation_id: installation_id_3.clone(),
                        priority: 3,
                        parameters: HashMap::from_iter(vec![(
                            "foo".to_string(),
                            "bar".to_string(),
                        )]),
                    }),
                    PluginInstallationAction::Install(PluginInstallationCreation {
                        name: "library-plugin-4".to_string(),
                        version: "v1".to_string(),
                        priority: 4,
                        parameters: HashMap::new(),
                    }),
                ],
            },
        )
        .await
        .unwrap();

    let latest_version = deps
        .component_service()
        .component_http_client(&user.token)
        .await
        .get_latest_component_metadata(&component_id.0)
        .await
        .unwrap()
        .versioned_component_id
        .version;

    let installed_plugins = deps
        .component_service()
        .component_http_client(&user.token)
        .await
        .get_installed_plugins(&component_id.0, &latest_version.to_string())
        .await
        .unwrap();

    assert_eq!(installed_plugins.len(), 3);
    {
        let mut priorities = installed_plugins
            .iter()
            .map(|ip| ip.priority)
            .collect::<Vec<_>>();
        priorities.sort();
        assert_eq!(priorities, vec![0, 3, 4]);
    }
    {
        let installation_ids = installed_plugins
            .iter()
            .map(|ip| ip.id)
            .collect::<HashSet<_>>();
        assert!(installation_ids.contains(&installation_id_1.0)); // untouched
        assert!(!installation_ids.contains(&installation_id_2.0)); // uninstalled
        assert!(installation_ids.contains(&installation_id_3.0)); // updated
    }
}
