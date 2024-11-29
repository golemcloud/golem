// Copyright 2024 Golem Cloud
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
    ComponentTransformerDefinition, DefaultPluginOwner, DefaultPluginScope, PluginDefinition,
    PluginTypeSpecificDefinition,
};
use golem_common::model::Empty;
use golem_test_framework::config::EnvBasedTestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
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
