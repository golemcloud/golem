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
use axum::body::Bytes;
use axum::extract::Multipart;
use axum::routing::post;
use axum::Router;
use golem_api_grpc::proto::golem::worker::{log_event, Log};
use golem_client::api::PluginClient;
use golem_common::model::plugin::{
    AppPluginDefinition, ComponentTransformerDefinition, LibraryPluginDefinition,
    OplogProcessorDefinition, PluginTypeSpecificDefinition,
};
use golem_common::model::plugin::{PluginScope, ProjectPluginScope};
use golem_common::model::{Empty, ScanCursor};
use golem_test_framework::config::{
    EnvBasedTestDependencies, TestDependencies, TestDependenciesDsl,
};
use golem_test_framework::dsl::TestDslUnsafe;
use golem_test_framework::model::PluginDefinitionCreation;
use golem_wasm_ast::analysis::{AnalysedExport, AnalysedInstance};
use golem_wasm_rpc::{IntoValueAndType, Value};
use reqwest::StatusCode;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};
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

    async fn transform(mut multipart: Multipart) -> Vec<u8> {
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

        transform_component(component.expect("did not receive a component part"))
            .expect("Failed to transform component") // TODO: error handling and returning a proper HTTP response with failed status code
    }

    let admin = deps.admin();

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

    async fn transform(mut multipart: Multipart) -> Vec<u8> {
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

        transform_component(component.expect("did not receive a component part"))
            .expect("Failed to transform component")
    }

    let admin = deps.admin();

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

    let exports = patched_component_metadata.exports;

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
async fn component_transformer_failed(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    async fn transform() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
    let admin = deps.admin();

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
            vec![vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ]
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
            vec![vec![
                ("product-id", "G1000".into_value_and_type()),
                ("name", "Golem T-Shirt M".into_value_and_type()),
                ("price", 100.0f32.into_value_and_type()),
                ("quantity", 5u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1001".into_value_and_type()),
                ("name", "Golem Cloud Subscription 1y".into_value_and_type()),
                ("price", 999999.0f32.into_value_and_type()),
                ("quantity", 1u32.into_value_and_type()),
            ]
            .into_value_and_type()],
        )
        .await;

    let _ = user
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![vec![
                ("product-id", "G1002".into_value_and_type()),
                ("name", "Mud Golem".into_value_and_type()),
                ("price", 11.0f32.into_value_and_type()),
                ("quantity", 10u32.into_value_and_type()),
            ]
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
    let admin = deps.admin();
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
    let admin = deps.admin();

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
    let admin = deps.admin();

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

    admin.delete_plugin("library-plugin-2", "v1").await;

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
    let admin = deps.admin();

    let component_id = admin
        .component("app_and_library_app")
        .unique()
        .store()
        .await;

    let plugin_wasm_key = admin.add_plugin_wasm("app_and_library_library").await;

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

    admin.create_plugin(plugin_definition.clone()).await;

    let _installation_id = admin
        .install_plugin_to_component(&component_id, "library-plugin-3", "v1", 0, HashMap::new())
        .await;

    admin.delete_plugin("library-plugin-3", "v1").await;

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
            .list_plugins(Some(&PluginScope::project(project_1)))
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
            .list_plugins(Some(&PluginScope::component(component_id)))
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
