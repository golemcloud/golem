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

use crate::Tracing;
use crate::instance_layer::{
    activation, owner_component_metadata, run_synchronous_entity_invocation,
};
use golem_common::base_model::agent::{AgentPrincipal, Principal};
use golem_common::model::OwnedAgentId;
use golem_common::model::entity::{AgentEntity, ExecutableTarget, FilesystemCapability};
use golem_common::model::tool::ToolName;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::durable_host::DurableWorkerCtxView;
use golem_worker_executor::preview2::golem_api_1_x::host::Host as GolemApiHost;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use std::sync::Arc;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[derive(Debug)]
struct StoreObservation {
    owner: OwnedAgentId,
    principal: Principal,
    store_address: usize,
    oplog_index: u64,
}

#[test]
#[timeout("120s")]
#[tracing::instrument]
async fn same_tool_has_fresh_stores_and_owner_isolated_durable_state_after_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let owners = [
        agent_id!("Clock", "tool-owner-a"),
        agent_id!("Clock", "tool-owner-b"),
    ];
    let mut owner_ids = Vec::new();
    let mut oplog_before_reconstruction = Vec::new();

    for owner in &owners {
        let worker_id = executor.start_agent(&component.id, owner.clone()).await?;
        executor
            .invoke_and_await_agent(&component, owner, "healthcheck", data_value!())
            .await?;
        let owner_id = OwnedAgentId::new(context.default_environment_id, &worker_id);
        let active = executor
            .active_agent(&owner_id)
            .await
            .expect("owner is active before reconstruction");
        assert_eq!(active.owner_id(), &owner_id);
        oplog_before_reconstruction.push(active.execution().oplog().current_oplog_index().await);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !executor.stop_worker_if_idle(&owner_id).await? {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            anyhow::Ok(())
        })
        .await??;
        executor.retire_unloaded_worker(&owner_id).await?;
        assert!(executor.active_agent(&owner_id).await.is_none());

        executor
            .invoke_and_await_agent(&component, owner, "healthcheck", data_value!())
            .await?;
        let reconstructed = executor
            .active_agent(&owner_id)
            .await
            .expect("owner is reconstructed");
        assert_eq!(reconstructed.owner_id(), &owner_id);
        assert!(
            reconstructed
                .execution()
                .oplog()
                .current_oplog_index()
                .await
                > *oplog_before_reconstruction.last().unwrap()
        );
        owner_ids.push(owner_id);
    }

    let tool_name = ToolName::try_from("owner-isolation-probe").map_err(anyhow::Error::msg)?;
    let entity = AgentEntity::Tool(tool_name.clone());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut first_calls = Vec::new();

    for (owner, owner_id) in owners.iter().zip(&owner_ids) {
        let active = executor
            .active_agent(owner_id)
            .await
            .expect("owner is active");
        let metadata = owner_component_metadata(&active, component.id, component.revision).await?;
        let activation = Arc::new(activation(
            ExecutableTarget::new(component.id, component.revision),
            "test:owner-isolation-probe",
            owner.agent_type.clone(),
            tool_name.clone(),
            context.account_id,
            FilesystemCapability::Incapable,
        ));
        let barrier = barrier.clone();
        let active = active.clone();
        let owner_id = owner_id.clone();
        let entity = entity.clone();
        first_calls.push(tokio::spawn(async move {
            run_synchronous_entity_invocation(
                &active,
                metadata,
                &owner_id,
                &entity,
                activation,
                move |_, store, principal| {
                    Box::pin(async move {
                        barrier.wait().await;
                        let observed_owner = store.data().durable_ctx().owned_agent_id().clone();
                        let store_address = store as *mut _ as usize;
                        let oplog_index = u64::from(
                            GolemApiHost::get_oplog_index(store.data_mut().durable_ctx_mut())
                                .await?,
                        );
                        Ok(StoreObservation {
                            owner: observed_owner,
                            principal,
                            store_address,
                            oplog_index,
                        })
                    })
                },
            )
            .await
        }));
    }

    let mut observations = Vec::new();
    for call in first_calls {
        observations.push(call.await??);
    }
    observations.sort_by_key(|observation| observation.owner.agent_name());
    assert_eq!(observations[0].owner, owner_ids[0]);
    assert_eq!(observations[1].owner, owner_ids[1]);
    assert_eq!(
        observations[0].principal,
        Principal::Agent(AgentPrincipal {
            agent_id: owner_ids[0].agent_id.clone(),
        })
    );
    assert_eq!(
        observations[1].principal,
        Principal::Agent(AgentPrincipal {
            agent_id: owner_ids[1].agent_id.clone(),
        })
    );
    assert_ne!(observations[0].store_address, observations[1].store_address);

    for ((owner, owner_id), first) in owners.iter().zip(&owner_ids).zip(&observations) {
        let active = executor
            .active_agent(owner_id)
            .await
            .expect("reconstructed owner remains active");
        let metadata = owner_component_metadata(&active, component.id, component.revision).await?;
        let activation = Arc::new(activation(
            ExecutableTarget::new(component.id, component.revision),
            "test:owner-isolation-probe",
            owner.agent_type.clone(),
            tool_name.clone(),
            context.account_id,
            FilesystemCapability::Incapable,
        ));
        let second = run_synchronous_entity_invocation(
            &active,
            metadata,
            owner_id,
            &entity,
            activation,
            |_, store, principal| {
                Box::pin(async move {
                    let observed_owner = store.data().durable_ctx().owned_agent_id().clone();
                    let store_address = store as *mut _ as usize;
                    let oplog_index = u64::from(
                        GolemApiHost::get_oplog_index(store.data_mut().durable_ctx_mut()).await?,
                    );
                    Ok(StoreObservation {
                        owner: observed_owner,
                        principal,
                        store_address,
                        oplog_index,
                    })
                })
            },
        )
        .await?;

        assert_eq!(&second.owner, owner_id);
        assert_ne!(second.store_address, first.store_address);
        assert!(
            second.oplog_index > first.oplog_index,
            "durable progress from both transient Stores must be carried by the owner oplog"
        );
    }

    Ok(())
}
