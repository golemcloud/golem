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
                }
                .try_into()
                .unwrap(),
                allow_streams: false,
                invocation_context: InvocationContextStack::fresh(),
                caller: Caller {
                    original: crate::test_callers::user_principal(),
                    immediate,
                    tokens: Vec::new(),
                },
                scope_card: None,
            },
            jiff::Timestamp::now(),
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

/// On the direct route, a read of the stored history of the target that failed for good is the
/// failure of the awaited call, which the caller records without a retry: its error names the
/// target, the entry and the version byte, also when a failed resume wraps it. Only a read that
/// failed for a time leaves the call without an outcome, and the caller recovers. Both keep the
/// read until the observer decides.
#[test]
fn direct_route_records_a_target_read_that_failed_for_good_as_the_call_failure() {
    use super::{RpcAwaitError, RpcCallFailure, rpc_await_outcome};
    use golem_common::model::AgentId;

    let caller = AgentId::from_parts(7, 0xca11);
    let target = AgentId::from_parts(7, 0x7a29);
    let failed = WorkerExecutorError::oplog_failed(
        target,
        "oplog entry 43 has unknown serialization version 255",
    );
    let unavailable =
        WorkerExecutorError::oplog_read_unavailable("primary storage of agent 7 did not answer");
    let call_failure = Ok(Err(RpcCallFailure::UnreadableTarget(
        RpcError::RemoteInternalError {
            details: format!(
                "The oplog of {target} failed: oplog entry 43 has unknown serialization version 255"
            ),
        },
    )));
    [
        (failed.clone(), call_failure.clone()),
        (
            WorkerExecutorError::failed_to_resume_worker(target, failed.clone()),
            call_failure.clone(),
        ),
        (unavailable.clone(), Err(unavailable.clone())),
        (
            WorkerExecutorError::failed_to_resume_worker(target, unavailable.clone()),
            Err(unavailable),
        ),
    ]
    .into_iter()
    .for_each(|(error, expected)| {
        assert_eq!(
            RpcAwaitError::from(error.clone()),
            RpcAwaitError::FailedRead(Box::new(error.clone())),
            "{error:?}"
        );
        assert_eq!(
            rpc_await_outcome::<()>(&caller, Err(error.clone().into())),
            expected,
            "{error:?}"
        );
    });

    // A failure that the target recorded, and any other error, stays the outcome of the call.
    assert_eq!(
        rpc_await_outcome::<()>(
            &caller,
            Err(WorkerExecutorError::invalid_request("wrong argument shape").into())
        ),
        Ok(Err(RpcCallFailure::Outcome(RpcError::RemoteAgentError {
            error: Box::new(ModelAgentError::InvalidInput(
                "wrong argument shape".to_string()
            )),
        })))
    );
    assert_eq!(
        rpc_await_outcome::<()>(
            &caller,
            Err(WorkerExecutorError::recovery_required("trapped body cleanup failed").into())
        ),
        Ok(Err(RpcCallFailure::Outcome(
            RpcError::RemoteInternalError {
                details: "Runtime reconstruction required: trapped body cleanup failed".to_string()
            }
        )))
    );

    // An archive layer of the target that did not store its oplog is also `OplogFailed`, but it is
    // not a failed read: it stays the outcome of the call, which keeps its retry.
    let archive_write: WorkerExecutorError = crate::services::oplog::ArchiveWriteFailure {
        agent_id: target,
        level: 1,
        details: "archive storage refused the chunk".to_string(),
    }
    .into();
    [
        archive_write.clone(),
        WorkerExecutorError::failed_to_resume_worker(target, archive_write.clone()),
    ]
    .into_iter()
    .for_each(|error| {
        assert_eq!(
            rpc_await_outcome::<()>(&caller, Err(error.clone().into())),
            Ok(Err(RpcCallFailure::Outcome(
                RpcError::RemoteInternalError {
                    details: error.to_string()
                }
            ))),
            "{error:?}"
        );
    });
}

/// On the remote route, a rejection or a completion whose typed worker error is a read of the
/// target history that failed for a time keeps the read for the observer. An oplog that failed for
/// good crosses gRPC as a runtime error with no read kind, so it stays the outcome of the call, as
/// a completion with a recorded failure and any other runtime error do.
#[test]
fn remote_route_keeps_a_failed_target_read_out_of_the_outcome() {
    use super::RpcAwaitError;
    use super::remote::{await_error_from_invocation_finished, await_error_from_rejection};
    use golem_api_grpc::proto::golem::worker::{
        InvocationRejected, InvocationRejectionReason, InvocationSessionCompletion,
        invocation_session_completion,
    };
    use golem_common::model::AgentId;

    let target = AgentId::from_parts(7, 0x7a29);
    let failed = WorkerExecutorError::oplog_failed(
        target,
        "oplog entry 43 has unknown serialization version 255",
    );
    let unavailable =
        WorkerExecutorError::oplog_read_unavailable("primary storage of agent 7 did not answer");
    let rejected = |error: &WorkerExecutorError| InvocationRejected {
        reason: InvocationRejectionReason::Internal as i32,
        error: error.to_string(),
        worker_error: Some(error.clone().into()),
        ..Default::default()
    };
    let finished = |error: &WorkerExecutorError| InvocationSessionCompletion {
        outcome: Some(invocation_session_completion::Outcome::Failure(
            InvocationFailure {
                kind: InvocationFailureKind::Execution as i32,
                code: "oplog-unreadable".to_string(),
                message: error.to_string(),
                worker_error: Some(error.clone().into()),
            },
        )),
    };
    assert_eq!(
        await_error_from_rejection(rejected(&unavailable)),
        RpcAwaitError::FailedRead(Box::new(unavailable.clone()))
    );
    assert_eq!(
        await_error_from_invocation_finished(finished(&unavailable)),
        RpcAwaitError::FailedRead(Box::new(unavailable.clone()))
    );

    let runtime = WorkerExecutorError::runtime("the guest stack overflowed");
    [&failed, &runtime].into_iter().for_each(|error| {
        assert_eq!(
            await_error_from_rejection(rejected(error)),
            RpcAwaitError::Outcome(RpcError::RemoteInternalError {
                details: error.to_string()
            }),
            "{error:?}"
        );
    });
    assert_eq!(
        await_error_from_invocation_finished(finished(&failed)),
        RpcAwaitError::Outcome(RpcError::RemoteInternalError {
            details: format!("Runtime error: {failed}")
        })
    );
    let recorded = InvocationSessionCompletion {
        outcome: Some(invocation_session_completion::Outcome::Failure(
            InvocationFailure {
                kind: InvocationFailureKind::Execution as i32,
                code: "persisted-invocation-failure".to_string(),
                message: "the method returned an error".to_string(),
                worker_error: None,
            },
        )),
    };
    assert_eq!(
        await_error_from_invocation_finished(recorded),
        RpcAwaitError::Outcome(RpcError::RemoteInternalError {
            details: "the method returned an error".to_string()
        })
    );
}

/// A scalar awaited call has no result when the stored history of its target failed for a time,
/// on the direct route, where the target gives the worker error itself, and on the remote route,
/// where the proxy decodes it from the typed error of the worker API. On the direct route a
/// history that failed for good is the failure of the call, which is not retried; on the remote
/// route it crosses as a runtime error and stays the outcome of the call. A recorded failure of
/// the target and a proxy error that is not a worker error stay the outcome of the call.
#[test]
fn scalar_call_records_a_target_read_that_failed_for_good() {
    use super::{RpcAwaitError, RpcCallFailure, rpc_await_outcome};
    use golem_api_grpc::proto::golem::worker::v1::{AgentError, agent_error};
    use golem_common::model::AgentId;

    let caller = AgentId::from_parts(7, 0xca11);
    let target = AgentId::from_parts(7, 0x7a29);
    let failed = WorkerExecutorError::oplog_failed(
        target,
        "oplog entry 43 has unknown serialization version 255",
    );
    let unavailable =
        WorkerExecutorError::oplog_read_unavailable("primary storage of agent 7 did not answer");
    let remote = |error: &WorkerExecutorError| {
        WorkerProxyError::from(AgentError {
            error: Some(agent_error::Error::InternalError(error.clone().into())),
        })
    };
    [
        RpcAwaitError::from(unavailable.clone()),
        RpcAwaitError::from(remote(&unavailable)),
    ]
    .into_iter()
    .for_each(|awaited| {
        assert_eq!(
            awaited,
            RpcAwaitError::FailedRead(Box::new(unavailable.clone()))
        );
        assert_eq!(
            rpc_await_outcome::<()>(&caller, Err(awaited)),
            Err(unavailable.clone())
        );
    });
    assert_eq!(
        rpc_await_outcome::<()>(&caller, Err(RpcAwaitError::from(failed.clone()))),
        Ok(Err(RpcCallFailure::UnreadableTarget(
            RpcError::RemoteInternalError {
                details: failed.to_string()
            }
        )))
    );
    assert_eq!(
        rpc_await_outcome::<()>(&caller, Err(RpcAwaitError::from(remote(&failed)))),
        Ok(Err(RpcCallFailure::Outcome(
            RpcError::RemoteInternalError {
                details: format!("Runtime error: {failed}")
            }
        )))
    );

    let recorded = WorkerExecutorError::invalid_request("wrong argument shape");
    [
        RpcAwaitError::from(recorded.clone()),
        RpcAwaitError::from(remote(&recorded)),
    ]
    .into_iter()
    .for_each(|awaited| {
        assert_eq!(
            rpc_await_outcome::<()>(&caller, Err(awaited)),
            Ok(Err(RpcCallFailure::Outcome(RpcError::RemoteAgentError {
                error: Box::new(ModelAgentError::InvalidInput(
                    "wrong argument shape".to_string()
                )),
            })))
        );
    });
    assert_eq!(
        RpcAwaitError::from(WorkerProxyError::NotFound("agent-7 is gone".to_string())),
        RpcAwaitError::Outcome(RpcError::NotFound {
            details: "agent-7 is gone".to_string()
        })
    );
}

/// A read of the oplog of another agent that failed for good is the failure of the call, also in
/// the reason of a failed resume. The same failed read of the oplog of the observer stops the call
/// without an outcome, so it fails the observer.
#[test]
fn only_a_failed_read_of_another_agent_is_the_failure_of_the_call() {
    use super::observed_read_failure;
    use golem_common::model::AgentId;

    let observer = AgentId::from_parts(7, 0xca11);
    let target = AgentId::from_parts(7, 0x7a29);
    let other = WorkerExecutorError::oplog_failed(target, "oplog entry 43 has version 255");
    let own = WorkerExecutorError::oplog_failed(observer, "oplog entry 9 has version 254");

    assert_eq!(
        observed_read_failure(&observer, other.clone()),
        Ok(other.clone())
    );
    assert_eq!(
        observed_read_failure(
            &observer,
            WorkerExecutorError::failed_to_resume_worker(target, other.clone())
        ),
        Ok(other)
    );
    assert_eq!(observed_read_failure(&observer, own.clone()), Err(own));
}

/// A cold producer or consumer whose history does not read gives the failed read with the agent
/// whose history it is, so a durable stream reader reports the read and waits for a read that
/// failed for a time. A failed archive write and other errors are not failed reads.
#[test]
fn a_failed_history_read_keeps_its_agent_and_whether_it_can_pass_later() {
    use super::unreadable_history;
    use golem_common::model::AgentId;

    let owner = AgentId::from_parts(7, 0x0e1);
    let resumed = AgentId::from_parts(7, 0x2e5);
    let named = AgentId::from_parts(7, 0x7a29);
    let transient = WorkerExecutorError::oplog_read_unavailable("storage did not answer");
    let permanent = WorkerExecutorError::oplog_failed(named, "oplog entry 43 has version 255");

    let read = unreadable_history(&transient, &owner).unwrap();
    assert_eq!((read.agent_id, read.is_transient()), (owner, true));
    // A read that failed for a time and names its agent keeps that agent, not the owner.
    let read = unreadable_history(
        &WorkerExecutorError::oplog_read_unavailable_of(named, "storage did not answer"),
        &owner,
    )
    .unwrap();
    assert_eq!((read.agent_id, read.is_transient()), (named, true));
    let read = unreadable_history(
        &WorkerExecutorError::failed_to_resume_worker(resumed, transient),
        &owner,
    )
    .unwrap();
    assert_eq!((read.agent_id, read.is_transient()), (resumed, true));
    let read = unreadable_history(
        &WorkerExecutorError::failed_to_resume_worker(resumed, permanent),
        &owner,
    )
    .unwrap();
    assert_eq!((read.agent_id, read.is_transient()), (named, false));
    assert_eq!(
        unreadable_history(
            &WorkerExecutorError::oplog_archive_write_failed(named, "archive did not store"),
            &owner
        ),
        None
    );
    assert_eq!(
        unreadable_history(&WorkerExecutorError::recovery_required("replay"), &owner),
        None
    );
}
