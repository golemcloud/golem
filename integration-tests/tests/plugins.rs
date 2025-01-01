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

use crate::Tracing;
use axum::body::Bytes;
use axum::extract::Multipart;
use axum::routing::post;
use axum::Router;
use golem_api_grpc::proto::golem::worker::{log_event, Log};
use golem_common::model::plugin::{
    ComponentTransformerDefinition, DefaultPluginOwner, DefaultPluginScope,
    OplogProcessorDefinition, PluginDefinition, PluginTypeSpecificDefinition,
};
use golem_common::model::{Empty, ScanCursor};
use golem_test_framework::config::EnvBasedTestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::Value;
use std::collections::HashMap;
use test_r::{inherit_test_dep, test};
use tracing::{debug, info};
use wac_graph::types::Package;
use wac_graph::{plug, CompositionGraph, EncodeOptions, Processor};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
async fn component_transformer1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let component_id = deps.store_unique_component("logging").await;
    let port = 8999;

    deps.create_plugin(PluginDefinition {
        name: "component-transformer-1".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::ComponentTransformer(ComponentTransformerDefinition {
            provided_wit_package: None,
            json_schema: None,
            validate_url: "not-used".to_string(),
            transform_url: format!("http://localhost:{port}/transform"),
        }),
        scope: DefaultPluginScope::Global(Empty {}),
        owner: DefaultPluginOwner,
    })
    .await;

    let app = Router::new().route("/transform", post(transform));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .unwrap();
    let server_handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let _installation_id = deps
        .install_plugin_to_component(
            &component_id,
            "component-transformer-1",
            "v1",
            0,
            HashMap::new(),
        )
        .await;

    let worker = deps.start_worker(&component_id, "worker1").await;
    let mut rx = deps.capture_output(&worker).await;

    let _ = deps
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
                let json = std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
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

fn transform_component(component: Bytes) -> anyhow::Result<Vec<u8>> {
    let mut graph = CompositionGraph::new();
    let component = Package::from_bytes("component", None, component, graph.types_mut())?;
    let component = graph.register_package(component)?;

    let adapter_bytes = include_bytes!("../../test-components/component-transformer1-adapter.wasm");

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

#[test]
async fn oplog_processor1(deps: &EnvBasedTestDependencies, _tracing: &Tracing) {
    let plugin_component_id = deps.store_unique_component("oplog-processor").await;
    let component_id = deps.store_unique_component("shopping-cart").await;

    deps.create_plugin(PluginDefinition {
        name: "oplog-processor-1".to_string(),
        version: "v1".to_string(),
        description: "A test".to_string(),
        icon: vec![],
        homepage: "none".to_string(),
        specs: PluginTypeSpecificDefinition::OplogProcessor(OplogProcessorDefinition {
            component_id: plugin_component_id.clone(),
            component_version: 0,
        }),
        scope: DefaultPluginScope::Global(Empty {}),
        owner: DefaultPluginOwner,
    })
    .await;

    let _installation_id = deps
        .install_plugin_to_component(&component_id, "oplog-processor-1", "v1", 0, HashMap::new())
        .await;

    let worker_id = deps.start_worker(&component_id, "worker1").await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{initialize-cart}",
            vec![Value::String("test-user-1".to_string())],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1000".to_string()),
                Value::String("Golem T-Shirt M".to_string()),
                Value::F32(100.0),
                Value::U32(5),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1001".to_string()),
                Value::String("Golem Cloud Subscription 1y".to_string()),
                Value::F32(999999.0),
                Value::U32(1),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{add-item}",
            vec![Value::Record(vec![
                Value::String("G1002".to_string()),
                Value::String("Mud Golem".to_string()),
                Value::F32(11.0),
                Value::U32(10),
            ])],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{update-item-quantity}",
            vec![Value::String("G1002".to_string()), Value::U32(20)],
        )
        .await;

    let _ = deps
        .invoke_and_await(
            &worker_id,
            "golem:it/api.{force-commit}",
            vec![Value::U8(10)],
        )
        .await;

    let mut plugin_worker_id = None;
    let mut cursor = ScanCursor::default();

    loop {
        let (maybe_cursor, items) = deps
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
        let response = deps
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

    //   left: ["-1/ff34cdec-65e3-4960-aa86-6316f93ffe40/worker1/golem:it/api.{initialize-cart}", "-1/ff34cdec-65e3-4960-aa86-6316f93ffe40/worker1/golem:it/api.{add-item}", "-1/ff34cdec-65e3-4960-aa86-6316f93ffe40/worker1/golem:it/api.{add-item}", "-1/ff34cdec-65e3-4960-aa86-6316f93ffe40/worker1/golem:it/api.{add-item}", "-1/ff34cdec-65e3-4960-aa86-6316f93ffe40/worker1/golem:it/api.{update-item-quantity}"]
    //  right: ["-1/ff34cdec-65e3-4ad4-aa86-6316f93fff11/worker1/golem:it/api.{initialize-cart}", "-1/ff34cdec-65e3-4ad4-aa86-6316f93fff11/worker1/golem:it/api.{add-item}", "-1/ff34cdec-65e3-4ad4-aa86-6316f93fff11/worker1/golem:it/api.{add-item}", "-1/ff34cdec-65e3-4ad4-aa86-6316f93fff11/worker1/golem:it/api.{add-item}", "-1/ff34cdec-65e3-4ad4-aa86-6316f93fff11/worker1/golem:it/api.{update-item-quantity}"]

    let expected = vec![
        format!("-1/{component_id}/worker1/golem:it/api.{{initialize-cart}}"),
        format!("-1/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("-1/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("-1/{component_id}/worker1/golem:it/api.{{add-item}}"),
        format!("-1/{component_id}/worker1/golem:it/api.{{update-item-quantity}}"),
    ];
    assert_eq!(invocations, expected);
}
