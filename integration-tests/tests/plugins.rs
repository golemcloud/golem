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

use golem_client::api::RegistryServiceClient;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::base64::Base64;
use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation,
    PluginSpecDto,
};
use golem_common::model::ScanCursor;
use golem_common::{agent_id, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended, WorkerInvocationResultOps};
use golem_wasm::Value;
use pretty_assertions::assert_eq;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

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
        .component(&env.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_plugin(&oplog_processor_plugin_grant.id, 0)
        .store()
        .await?;

    let repo_id = agent_id!("repository", "worker1");
    let _worker_id = user.start_agent(&component.id, repo_id.clone()).await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1000", "Golem T-Shirt M"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1001", "Golem Cloud Subscription 1y"),
    )
    .await?;

    user.invoke_and_await_agent(
        &component.id,
        &repo_id,
        "add",
        data_value!("G1002", "Mud Golem"),
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
            .await
            .collapse()?;

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
    let worker_name = repo_id.to_string();

    // TODO: to be updated once tests are no longer going through the dynamic agent invocation
    let expected = vec![
        format!("{account_id}/{component_id}/{worker_name}/golem:agent/guest.{{initialize}}"),
        format!("{account_id}/{component_id}/{worker_name}/golem:agent/guest.{{invoke}}"),
        format!("{account_id}/{component_id}/{worker_name}/golem:agent/guest.{{invoke}}"),
    ];
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
        .component(&env_1.id, "it_agent_counters_release")
        .name("it:agent-counters")
        .with_plugin(&oplog_processor_plugin_grant.id, 0)
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
            &component.id,
            &repo_id,
            "add",
            data_value!("G1000", "Golem T-Shirt M"),
        )
        .await?;

    user_1
        .invoke_and_await_agent(
            &component.id,
            &repo_id,
            "add",
            data_value!("G1001", "Golem Cloud Subscription 1y"),
        )
        .await?;

    user_1
        .invoke_and_await_agent(
            &component.id,
            &repo_id,
            "add",
            data_value!("G1002", "Mud Golem"),
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
            .await
            .collapse()?;

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
    let worker_name = repo_id.to_string();

    // TODO: to be updated once tests are no longer going through the dynamic agent invocation
    let expected = vec![
        format!("{account_id}/{component_id}/{worker_name}/golem:agent/guest.{{initialize}}"),
        format!("{account_id}/{component_id}/{worker_name}/golem:agent/guest.{{invoke}}"),
        format!("{account_id}/{component_id}/{worker_name}/golem:agent/guest.{{invoke}}"),
    ];
    assert_eq!(invocations, expected);

    Ok(())
}
