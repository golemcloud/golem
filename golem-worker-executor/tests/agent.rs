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

use crate::common::{start, TestContext};
use crate::{LastUniqueId, Tracing, WorkerExecutorTestDependencies};
use golem_common::model::agent::{DataValue, ElementValues};
use golem_common::model::{AgentInstanceKey, WorkerResourceKey};
use golem_test_framework::config::TestDependencies;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_ast::analysis::wit_parser::SharedAnalysedTypeResolve;
use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
use test_r::{inherit_test_dep, test, timeout};
use tracing::debug;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("golem_host")]
    SharedAnalysedTypeResolve
);

#[test]
#[tracing::instrument]
#[timeout(120_000)]
async fn agent_resource_registration(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
) {
    // In this test we use directly the exported agent functions to create an agent (not as
    // an indexed resource) and then drop it, and verify that the agent implementation itself
    // registered this created resource as an agent instance and then removed it when dropped.
    //
    // The test agent also exports two functions to create and drop a _second_ agent by itself from
    // the guest side, and we verify that this one is also registered as an agent instance.

    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await.unwrap().into_admin().await;

    let component_id = executor.component("low-level-agent").store().await;
    let worker_id = executor.start_worker(&component_id, "agent-1").await;

    let agent1 = executor
        .invoke_and_await(
            &worker_id,
            "golem:agent/guest.{[static]agent.create}",
            vec![
                "test-agent-type".into_value_and_type(),
                DataValue::Tuple(ElementValues { elements: vec![] }).into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    debug!("Created agent1: {:?}", agent1);
    let Value::Result(Ok(Some(agent1))) = &agent1[0] else {
        panic!("Failed to create agent1: {agent1:?}");
    };

    let component_metadata = executor.get_latest_component_metadata(&component_id).await;

    let get_id_function = component_metadata
        .find_function("golem:agent/guest.{[method]agent.get-id}")
        .await
        .expect("Failed to get 'golem:agent/guest.{[method]agent.get-id}' in component metadata")
        .expect("Function 'golem:agent/guest.{[method]agent.get-id}' is not in component metadata");

    let agent1_id = executor
        .invoke_and_await(
            &worker_id,
            "golem:agent/guest.{[method]agent.get-id}",
            vec![ValueAndType::new(
                *agent1.clone(),
                get_id_function.analysed_export.parameters[0].typ.clone(),
            )],
        )
        .await
        .unwrap();

    debug!("Agent1 ID: {:?}", agent1_id);

    let (metadata1, _) = executor.get_worker_metadata(&worker_id).await.unwrap();
    debug!("Worker metadata after creating agent1: {:#?}", metadata1);

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:agent/guest.{[method]agent.invoke}",
            vec![
                ValueAndType::new(
                    *agent1.clone(),
                    get_id_function.analysed_export.parameters[0].typ.clone(),
                ),
                "create-inner".into_value_and_type(),
                DataValue::Tuple(ElementValues { elements: vec![] }).into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    let (metadata2, _) = executor.get_worker_metadata(&worker_id).await.unwrap();
    debug!(
        "Worker metadata after creating inner agent: {:#?}",
        metadata2
    );

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:agent/guest.{[method]agent.invoke}",
            vec![
                ValueAndType::new(
                    *agent1.clone(),
                    get_id_function.analysed_export.parameters[0].typ.clone(),
                ),
                "drop-all-inner".into_value_and_type(),
                DataValue::Tuple(ElementValues { elements: vec![] }).into_value_and_type(),
            ],
        )
        .await
        .unwrap();

    let (metadata3, _) = executor.get_worker_metadata(&worker_id).await.unwrap();
    debug!(
        "Worker metadata after dropping inner agent: {:#?}",
        metadata3
    );

    let _ = executor
        .invoke_and_await(
            &worker_id,
            "golem:agent/guest.{[drop]agent}",
            vec![ValueAndType::new(
                *agent1.clone(),
                get_id_function.analysed_export.parameters[0].typ.clone(),
            )],
        )
        .await
        .unwrap();

    let (metadata4, _) = executor.get_worker_metadata(&worker_id).await.unwrap();
    debug!("Worker metadata after dropping agent1: {:#?}", metadata4);

    executor.check_oplog_is_queryable(&worker_id).await;
    drop(executor);

    let mut ids1 = metadata1
        .last_known_status
        .owned_resources
        .iter()
        .filter_map(|(key, _)| match key {
            WorkerResourceKey::ExportedResourceInstanceKey(_) => None,
            WorkerResourceKey::AgentInstanceKey(key) => Some(key.clone()),
        })
        .collect::<Vec<_>>();
    let mut ids2 = metadata2
        .last_known_status
        .owned_resources
        .iter()
        .filter_map(|(key, _)| match key {
            WorkerResourceKey::ExportedResourceInstanceKey(_) => None,
            WorkerResourceKey::AgentInstanceKey(key) => Some(key.clone()),
        })
        .collect::<Vec<_>>();
    let mut ids3 = metadata3
        .last_known_status
        .owned_resources
        .iter()
        .filter_map(|(key, _)| match key {
            WorkerResourceKey::ExportedResourceInstanceKey(_) => None,
            WorkerResourceKey::AgentInstanceKey(key) => Some(key.clone()),
        })
        .collect::<Vec<_>>();
    let mut ids4 = metadata4
        .last_known_status
        .owned_resources
        .iter()
        .filter_map(|(key, _)| match key {
            WorkerResourceKey::ExportedResourceInstanceKey(_) => None,
            WorkerResourceKey::AgentInstanceKey(key) => Some(key.clone()),
        })
        .collect::<Vec<_>>();

    ids1.sort();
    ids2.sort();
    ids3.sort();
    ids4.sort();

    assert_eq!(
        ids1,
        vec![AgentInstanceKey {
            agent_type: "TestAgent".to_string(),
            agent_id: "agent-1".to_string()
        }]
    );
    assert_eq!(
        ids2,
        vec![
            AgentInstanceKey {
                agent_type: "TestAgent".to_string(),
                agent_id: "agent-1".to_string()
            },
            AgentInstanceKey {
                agent_type: "TestAgent".to_string(),
                agent_id: "agent-2".to_string()
            }
        ]
    );
    assert_eq!(
        ids3,
        vec![AgentInstanceKey {
            agent_type: "TestAgent".to_string(),
            agent_id: "agent-1".to_string()
        }]
    );
    assert_eq!(ids4, vec![]);
}
