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
use async_trait::async_trait;
use golem_common::model::card::{CardId, StoredCard, parse_polymorphic_permission};
use golem_common::model::oplog::PublicOplogEntry;
use golem_common::model::oplog::payload::HostRequestGolemApiRevertAgent;
use golem_common::model::worker::{RevertToOplogIndex, RevertWorkerTarget};
use golem_common::model::{AgentStatus, OplogIndex};
use golem_common::schema::FromSchema;
use golem_common::{agent_id, data_value};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor::services::card::{CardService, CardState};
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestCardService, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(
    #[tagged_as("host_api_tests")]
    PrecompiledComponent
);
inherit_test_dep!(Tracing);

#[derive(Default)]
struct RevertAuthority(RwLock<Option<StoredCard>>);

#[async_trait]
impl CardService for RevertAuthority {
    async fn record_revoked_cards(&self, _card_ids: &[CardId]) {}

    async fn check_cards(
        &self,
        card_ids: Vec<CardId>,
    ) -> Result<HashMap<CardId, CardState>, WorkerExecutorError> {
        let mut result = TestCardService.check_cards(card_ids).await?;
        if let Some(card) = self.0.read().unwrap().as_ref()
            && let Some(state) = result.get_mut(&card.card_id())
        {
            *state = CardState::Live(Box::new(card.clone()));
        }
        Ok(result)
    }
}

#[test]
#[timeout("120s")]
async fn replay_tail_revert_records_and_authorizes_the_resolved_index(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let authority = Arc::new(RevertAuthority::default());
    let service = authority.clone();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            create_card_service: Some(Arc::new(move || service.clone())),
            ..Default::default()
        },
    )
    .await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let target = agent_id!("GolemHostApi", "revert-index-target");
    let target_id = executor.start_agent(&component.id, target.clone()).await?;
    executor
        .invoke_and_await_agent(
            &component,
            &target,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?;
    executor
        .invoke_and_await_agent(
            &component,
            &target,
            "get_self_metadata_result",
            data_value!(),
        )
        .await?;
    let target_before = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
    let retained = target_before
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .expect("target invocation")
        .oplog_index
        .previous();

    let forbidden =
        parse_polymorphic_permission(&format!("agent(?env/*/*) @ * : revert : {retained}"))?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .name("revert-index-caller")
        .update_agent_provision_config("GolemHostApi", |config| {
            config
                .initial_permissions
                .lower_bound
                .negative
                .push(forbidden);
        })
        .store()
        .await?;
    let caller = agent_id!("GolemHostApi", "revert-index-caller");
    let card = component
        .metadata
        .agent_type_initial_permission_card(&caller.agent_type)
        .cloned()
        .expect("initial permission card");
    *authority.0.write().unwrap() = Some(StoredCard::Polymorphic(card));
    let caller_id = executor.start_agent(&component.id, caller.clone()).await?;
    let denied = executor
        .invoke_and_await_agent(
            &component,
            &caller,
            "revert_last_invocations_result",
            data_value!(target_id.clone(), 1u64),
        )
        .await?
        .into_typed::<Result<(), String>>()?;
    assert!(
        denied
            .as_ref()
            .is_err_and(|error| error.to_ascii_lowercase().contains("permission")),
        "the resolved index must return a permission denial: {denied:?}"
    );
    let before = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
    let invocation = before
        .iter()
        .rev()
        .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
        .expect("revert invocation")
        .oplog_index;

    // Retain the guest invocation but remove its first host call, so reconstruction reaches
    // revert-agent while replay is active and must publish live before resolving the target.
    executor
        .revert(
            &caller_id,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: invocation,
            }),
        )
        .await?;
    executor.resume(&caller_id, true).await?;
    executor
        .wait_for_status(&caller_id, AgentStatus::Idle, Duration::from_secs(30))
        .await?;
    let after = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
    let start = after
        .iter()
        .rev()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if entry.oplog_index > before.last().unwrap().oplog_index
                    && start.function_name == "golem::api::revert_worker" =>
            {
                Some(start)
            }
            _ => None,
        })
        .expect("replay-tail revert Start");
    let request = HostRequestGolemApiRevertAgent::from_value(
        start.request.as_ref().expect("recorded request").value(),
    )?;
    let resolved = request.resolved_revert.expect("resolved target");
    assert_eq!(resolved.last_oplog_index, retained);
    assert_eq!(
        resolved.observed_oplog_index,
        target_before.last().unwrap().oplog_index
    );
    assert!(
        start.denial.is_some(),
        "authorization must check the actual retained index"
    );
    let target_after = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
    assert_eq!(
        target_before.last().unwrap().oplog_index,
        target_after.last().unwrap().oplog_index
    );
    Ok(())
}

#[test]
#[timeout("120s")]
async fn revert_resolution_errors_survive_replay_tail_and_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    for missing in [false, true] {
        let context = TestContext::new(last_unique_id);
        let executor = start_with_overrides(deps, &context, Default::default()).await?;
        let component = executor
            .component_dep(&context.default_environment_id, host_api_tests)
            .store()
            .await?;
        let target = agent_id!("GolemHostApi", format!("resolution-target-{missing}"));
        let target_id = executor.start_agent(&component.id, target.clone()).await?;
        if missing {
            executor.delete_worker(&target_id).await?;
        }
        let count = if missing { 1u64 } else { 4u64 };
        let caller = agent_id!("GolemHostApi", format!("resolution-caller-{missing}"));
        let caller_id = executor.start_agent(&component.id, caller.clone()).await?;
        let error = executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "revert_last_invocations_result",
                data_value!(target_id.clone(), count),
            )
            .await?
            .into_typed::<Result<(), String>>()?
            .expect_err("count resolution must fail");
        assert!(error.contains(if missing {
            "does not exist"
        } else {
            "Could not find 4 invocations"
        }));
        let before = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
        let invocation = before
            .iter()
            .rev()
            .find(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .unwrap()
            .oplog_index;
        // Remove the failed call, but retain its invocation so it runs at the replay tail.
        executor
            .revert(
                &caller_id,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: invocation,
                }),
            )
            .await?;
        executor.resume(&caller_id, true).await?;
        executor
            .wait_for_status(&caller_id, AgentStatus::Idle, Duration::from_secs(30))
            .await?;
        let after = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
        let start = after
            .iter()
            .find(|entry| {
                entry.oplog_index > before.last().unwrap().oplog_index
                    && matches!(&entry.entry, PublicOplogEntry::Start(start)
                    if start.function_name == "golem::api::revert_worker" && start.denial.is_some())
            })
            .expect("failed replay-tail resolution must record its rejection in Start");

        // Both failures would now resolve successfully. Historical replay must not consult
        // this newer target state, even if the rejection's End is lost.
        if missing {
            executor.start_agent(&component.id, target.clone()).await?;
        }
        for _ in 0..6 {
            executor
                .invoke_and_await_agent(
                    &component,
                    &target,
                    "get_self_metadata_result",
                    data_value!(),
                )
                .await?;
        }
        let target_before = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
        executor
            .revert(
                &caller_id,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: start.oplog_index,
                }),
            )
            .await?;
        drop(executor);
        let executor = start_with_overrides(deps, &context, Default::default()).await?;
        executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "get_self_metadata_result",
                data_value!(),
            )
            .await?;
        let recovered = executor.get_oplog(&caller_id, OplogIndex::INITIAL).await?;
        let response = recovered
            .iter()
            .rev()
            .find_map(|entry| match &entry.entry {
                PublicOplogEntry::End(end)
                    if end.start_index == start.oplog_index
                        && entry.oplog_index > after.last().unwrap().oplog_index =>
                {
                    end.response.as_ref()
                }
                _ => None,
            })
            .expect("recovery must complete the failed resolution");
        let response = golem_common::model::oplog::payload::HostResponseGolemApiUnit::from_value(
            response.value(),
        )?;
        let recovered_error = response.result.expect_err("recovery must retain the error");
        assert!(recovered_error.contains(if missing {
            "does not exist"
        } else {
            "Could not find 4 invocations"
        }));
        let target_after = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            target_before.last().unwrap().oplog_index,
            target_after.last().unwrap().oplog_index
        );
        // Reconstruct the completed failure too, not just its incomplete Start.
        drop(executor);
        let executor = start_with_overrides(deps, &context, Default::default()).await?;
        executor
            .invoke_and_await_agent(
                &component,
                &caller,
                "get_self_metadata_result",
                data_value!(),
            )
            .await?;
        let target_after = executor.get_oplog(&target_id, OplogIndex::INITIAL).await?;
        assert_eq!(
            target_before.last().unwrap().oplog_index,
            target_after.last().unwrap().oplog_index
        );
    }
    Ok(())
}
