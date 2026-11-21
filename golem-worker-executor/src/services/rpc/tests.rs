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

use super::{RpcError, method_validation_revision, rpc_error_from_failure};
use crate::services::worker_proxy::WorkerProxyError;
use golem_api_grpc::proto::golem::worker::{InvocationFailure, InvocationFailureKind};
use golem_common::model::agent::{AgentError as ModelAgentError, InvocationFreshnessDisposition};
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::types::SerializableRpcError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::cell::Cell;
use test_r::test;

#[test]
async fn known_fresh_method_validation_uses_selected_revision_without_metadata_probe() {
    let probed_existing_worker = Cell::new(false);
    let revision =
        method_validation_revision(InvocationFreshnessDisposition::KnownFresh, || async {
            probed_existing_worker.set(true);
            Ok(Some(ComponentRevision::INITIAL))
        })
        .await
        .unwrap();

    assert_eq!(revision, None);
    assert!(!probed_existing_worker.get());
}

#[test]
async fn may_exist_method_validation_uses_existing_worker_revision() {
    let probed_existing_worker = Cell::new(false);
    let existing_revision = ComponentRevision::new(7).unwrap();
    let revision = method_validation_revision(InvocationFreshnessDisposition::MayExist, || async {
        probed_existing_worker.set(true);
        Ok(Some(existing_revision))
    })
    .await
    .unwrap();

    assert_eq!(revision, Some(existing_revision));
    assert!(probed_existing_worker.get());
}

/// The deployed revision is selected only when the agent is known to be fresh or is found
/// to be absent. A lookup that fails must not select it: an existing agent pinned to a
/// revision with a different signature for the method would be validated against the wrong
/// one, and a storage outage would be reported as a protocol error.
#[test]
async fn may_exist_method_validation_propagates_a_failed_lookup() {
    let result = method_validation_revision(InvocationFreshnessDisposition::MayExist, || async {
        Err(WorkerExecutorError::runtime(
            "key-value storage unavailable",
        ))
    })
    .await;

    assert!(
        result.is_err(),
        "a failed lookup selected a revision: {result:?}"
    );
}

#[test]
fn typed_worker_failure_preserves_rpc_error_category() {
    let worker_error = WorkerExecutorError::invalid_request("bad invocation");
    let error = rpc_error_from_failure(InvocationFailure {
        kind: InvocationFailureKind::Execution as i32,
        code: "worker-execution".to_string(),
        message: worker_error.to_string(),
        worker_error: Some(worker_error.into()),
    });

    assert_eq!(
        error,
        RpcError::RemoteAgentError {
            error: Box::new(ModelAgentError::InvalidInput("bad invocation".to_string(),)),
        }
    );
}
#[test]
fn cancelled_callee_is_the_same_permanent_error_before_and_after_acceptance() {
    let expected = RpcError::RemoteAgentError {
        error: Box::new(ModelAgentError::InvalidInput(
            "invocation-cancelled: Invocation was cancelled before execution".to_string(),
        )),
    };
    let finished = rpc_error_from_failure(InvocationFailure {
        kind: InvocationFailureKind::Execution as i32,
        code: "worker-execution".to_string(),
        message: WorkerExecutorError::InvocationCancelled.to_string(),
        worker_error: Some(WorkerExecutorError::InvocationCancelled.into()),
    });
    // A rejection whose reason alone would read as a transient internal error.
    let rejected = super::remote::rpc_error_from_rejection(
        golem_api_grpc::proto::golem::worker::InvocationRejected {
            reason: golem_api_grpc::proto::golem::worker::InvocationRejectionReason::Internal
                as i32,
            error: WorkerExecutorError::InvocationCancelled.to_string(),
            worker_error: Some(WorkerExecutorError::InvocationCancelled.into()),
            ..Default::default()
        },
    );

    assert_eq!(finished, expected);
    assert_eq!(rejected, expected);
}

#[test]
fn invalid_remote_request_is_an_agent_input_error() {
    let error = RpcError::from(WorkerExecutorError::invalid_request("wrong argument shape"));

    assert_eq!(
        error,
        RpcError::RemoteAgentError {
            error: Box::new(ModelAgentError::InvalidInput(
                "wrong argument shape".to_string()
            )),
        }
    );
}

#[test]
fn proxied_bad_request_is_an_agent_input_error() {
    let error = RpcError::from(WorkerProxyError::BadRequest(vec![
        "wrong argument shape".to_string(),
    ]));

    assert_eq!(
        error,
        RpcError::RemoteAgentError {
            error: Box::new(ModelAgentError::InvalidInput(
                "wrong argument shape".to_string()
            )),
        }
    );
}

#[test]
fn remote_agent_error_survives_serializable_roundtrip() {
    let error = RpcError::RemoteAgentError {
        error: Box::new(ModelAgentError::InvalidMethod("missing".to_string())),
    };

    let serialized = SerializableRpcError::from(error.clone());
    assert_eq!(RpcError::from(serialized), error);
}

#[test]
fn remote_agent_error_survives_wit_roundtrip() {
    let error = RpcError::RemoteAgentError {
        error: Box::new(ModelAgentError::InvalidAgentId("invalid".to_string())),
    };

    let wit = crate::preview2::golem::agent::host::RpcError::from(error.clone());
    assert_eq!(RpcError::from(wit), error);
}

#[test]
fn a_remote_schedule_names_the_scheduling_agent_as_parent_also_for_a_tool() {
    use super::remote::scheduled_invocation_start;
    use golem_common::model::caller::{Caller, Principal, ToolPrincipal};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::invocation_context::InvocationContextStack;
    use golem_common::model::tool::ToolName;
    use golem_common::model::{
        AgentFingerprint, AgentId, AgentInvocation, InvocationRequestKey, OwnedAgentId,
    };
    use golem_service_base::model::auth::AuthCtx;

    let environment_id = EnvironmentId(uuid::Uuid::from_u128(0xe0));
    let scheduler = AgentId::from_parts(7, 0xc0);
    let target = OwnedAgentId::new(environment_id, &AgentId::from_parts(8, 0xc1));
    let tool = Principal::Tool(ToolPrincipal {
        issuer: crate::test_callers::TEST_ISSUER.to_string(),
        environment_id,
        tool_name: ToolName::try_from("report").unwrap(),
    });
    for (case, immediate) in [
        ("the agent", crate::test_callers::agent_principal(scheduler)),
        ("a tool that the agent hosts", tool),
    ] {
        let (start, _) = scheduled_invocation_start(
            &target,
            AgentFingerprint(uuid::Uuid::from_u128(0xf1)),
            InvocationRequestKey::Unkeyed(uuid::Uuid::from_u128(0x5e)),
            AgentInvocation::AgentMethod {
                idempotency_key: golem_common::model::InvocationId::from(uuid::Uuid::from_u128(1)),
                method_name: "increment".to_string(),
                input: golem_common::schema::SchemaValue::Tuple {
                    elements: Vec::new(),
                },
                invocation_context: InvocationContextStack::fresh(),
                caller: Caller {
                    original: crate::test_callers::user_principal(),
                    immediate,
                    tokens: Vec::new(),
                },
                scope_card: None,
            },
            chrono::Utc::now(),
            &scheduler,
            &AuthCtx::System,
        )
        .unwrap();
        assert_eq!(
            start.context.unwrap().parent,
            Some(scheduler.into()),
            "{case}"
        );
    }
}
