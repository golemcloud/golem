// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use golem_client::api::RegistryServiceClient;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::base64::Base64;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginSpecDto,
};
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{inherit_test_dep, test};
use tracing::Instrument;

inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
async fn oplog_processor(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let client = user.registry_service_client().await;
    let (_, env) = user.app_and_env().await?;

    // Set up an HTTP server to receive callbacks from the oplog processor
    let received_invocations: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_invocations_clone = received_invocations.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let _http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/callback",
            post(move |body: Bytes| {
                async move {
                    let body_str = String::from_utf8(body.to_vec()).unwrap();
                    let items: Vec<String> = serde_json::from_str(&body_str).unwrap();
                    let mut invocations = received_invocations_clone.lock().unwrap();
                    invocations.extend(items);
                    "ok"
                }
                .in_current_span()
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    let callback_url = format!("http://localhost:{host_http_port}/callback");

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

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    let repo_id = agent_id!("repository", "worker1");
    let _worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
    )
    .await?;

    // Wait for the oplog processor to POST invocations to our callback server
    let account_id = user.account_id;
    let component_id = component.id;
    let worker_name = repo_id.to_string();

    let expected = vec![
        format!("{account_id}/{component_id}/{worker_name}/agent-initialization"),
        format!("{account_id}/{component_id}/{worker_name}/add"),
        format!("{account_id}/{component_id}/{worker_name}/add"),
        format!("{account_id}/{component_id}/{worker_name}/add"),
    ];

    let start = tokio::time::Instant::now();
    let deadline = Duration::from_secs(60);
    let invocations = loop {
        let invocations = received_invocations.lock().unwrap().clone();
        if invocations.len() >= expected.len() {
            break invocations;
        }
        if start.elapsed() > deadline {
            panic!(
                "Timed out waiting for oplog processor callback (got {} of {} expected: {:?})",
                invocations.len(),
                expected.len(),
                invocations,
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };

    assert_eq!(invocations, expected);

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

    // Set up an HTTP server to receive callbacks from the oplog processor
    let received_invocations: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_invocations_clone = received_invocations.clone();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let host_http_port = listener.local_addr().unwrap().port();

    let _http_server = tokio::spawn(async move {
        let route = Router::new().route(
            "/callback",
            post(move |body: Bytes| {
                async move {
                    let body_str = String::from_utf8(body.to_vec()).unwrap();
                    let items: Vec<String> = serde_json::from_str(&body_str).unwrap();
                    let mut invocations = received_invocations_clone.lock().unwrap();
                    invocations.extend(items);
                    "ok"
                }
                .in_current_span()
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    let callback_url = format!("http://localhost:{host_http_port}/callback");

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

    let mut plugin_params = BTreeMap::new();
    plugin_params.insert("callback-url".to_string(), callback_url.clone());

    let component = user_1
        .component(&env_1.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_parametrized_plugin(&oplog_processor_plugin_grant.id, 0, plugin_params)
        .store()
        .await?;

    client_2
        .delete_environment_plugin_grant(&oplog_processor_plugin_grant.id.0)
        .await?;
    client_2.delete_plugin(&oplog_processor_plugin.id.0).await?;

    let repo_id = agent_id!("repository", "worker1");
    let _worker_id = user_1.start_agent(&component.id, repo_id.clone()).await?;

    user_1
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    user_1
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

    user_1
        .invoke_and_await_agent(
            &component,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
        )
        .await?;

    // Wait for the oplog processor to POST invocations to our callback server
    let account_id = user_1.account_id;
    let component_id = component.id;
    let worker_name = repo_id.to_string();

    let expected = vec![
        format!("{account_id}/{component_id}/{worker_name}/agent-initialization"),
        format!("{account_id}/{component_id}/{worker_name}/add"),
        format!("{account_id}/{component_id}/{worker_name}/add"),
        format!("{account_id}/{component_id}/{worker_name}/add"),
    ];

    let start = tokio::time::Instant::now();
    let deadline = Duration::from_secs(60);
    let invocations = loop {
        let invocations = received_invocations.lock().unwrap().clone();
        if invocations.len() >= expected.len() {
            break invocations;
        }
        if start.elapsed() > deadline {
            panic!(
                "Timed out waiting for oplog processor callback (got {} of {} expected: {:?})",
                invocations.len(),
                expected.len(),
                invocations,
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };

    assert_eq!(invocations, expected);

    Ok(())
}
