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

use super::*;
use test_r::test;

#[test]
#[test_r::timeout("10s")]
async fn direct_claim_with_a_wrong_invocation_id_does_not_consume_the_start() {
    use super::super::tests::InMemoryOplog;
    use golem_common::model::oplog::HostRequestGolemRpcResource;
    use golem_common::model::oplog::host_functions::GolemRpcWasmRpcDrop;

    let recorded_invocation_id = DurableInvocationId::new(OplogIndex::from_u64(1), 7);
    for parent_start_index in [None, Some(OplogIndex::from_u64(19))] {
        let request = HostRequestGolemRpcResource {
            creation_index: OplogIndex::from_u64(7),
        };
        let oplog = Arc::new(InMemoryOplog::new());
        oplog
            .add(OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            })
            .await
            .unwrap();
        let start_index = oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index,
                function_name: GolemRpcWasmRpcDrop::HOST_FUNCTION_NAME,
                invocation_id: Some(recorded_invocation_id),
                observational_owner: None,
                request: Some(OplogPayload::Inline(Box::new(request.clone().into()))),
                durable_function_type: DurableFunctionType::ReadLocal,
                span_started: None,
                denial: None,
            })
            .await
            .unwrap();
        oplog
            .add(OplogEntry::End {
                timestamp: Timestamp::now_utc(),
                start_index,
                response: None,
                forced_commit: false,
                span_finished: None,
                span_attributes: None,
            })
            .await
            .unwrap();
        let replay_state = ReplayState::new_for_owner(
            golem_common::model::OwnedAgentId {
                environment_id: golem_common::model::environment::EnvironmentId::new(),
                agent_id: golem_common::model::AgentId {
                    component_id: golem_common::model::component::ComponentId::new(),
                    agent_id: "direct-claim-test".to_string(),
                },
            },
            oplog.clone(),
            golem_common::model::regions::DeletedRegions::default(),
            None,
            crate::durable_host::tool::operation::OwnerToolOperations::new(),
            oplog.current_oplog_index().await,
            None,
        )
        .await
        .unwrap();
        let begun = |invocation_id| BegunCall::<GolemRpcWasmRpcDrop, NotCancellable> {
            boundary: DurableCallBoundary::from_begin_index(OplogIndex::from_u64(1)),
            execution_scope: BegunCallExecutionScope {
                entity_parent_start_index: parent_start_index,
                parent_start_index,
                observational_owner: None,
                invocation_id: Some(invocation_id),
            },
            retry: InFunctionRetryController::new(
                DurableFunctionType::ReadLocal,
                DurableExecutionState {
                    is_live: false,
                    snapshotting_mode: false,
                    max_in_function_retry_delay: std::time::Duration::ZERO,
                },
                GolemRpcWasmRpcDrop::FQFN,
            ),
            requires_agent_authority: false,
            agent_auth_ctx: None,
            drop_sink: None,
            cleanup_sink: None,
            _phantom: PhantomData,
        };
        let wrong = replay_state
            .claim_start_or_replay_end(
                begun(DurableInvocationId::new(OplogIndex::from_u64(1), 8)).replay_claim(),
            )
            .await;
        // A top-level Start with another invocation id belongs to a concurrent sibling call,
        // so the claim retains it and goes live; under an unknown parent it is divergence.
        if parent_start_index.is_none() {
            assert!(matches!(wrong, Ok(ReplayStartClaimOutcome::ReplayEnded)));
        } else {
            assert!(wrong.is_err());
        }
        let claimed = replay_state
            .claim_start_or_replay_end(begun(recorded_invocation_id).replay_claim())
            .await
            .expect("a wrong invocation id must leave the recorded operation available");
        let ReplayStartClaimOutcome::Claimed { handle, .. } = claimed else {
            panic!("expected the matching Start to remain available");
        };
        assert_eq!(handle.start_idx(), start_index);
    }
}
