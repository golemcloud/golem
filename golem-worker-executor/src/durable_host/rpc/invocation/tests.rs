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

use super::{await_outcome, cancel_outcome, status_outcome};
use crate::services::rpc::RpcError;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamMapping, InvocationSessionResult, invocation_session_result,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    InvocationHandleSuccess, InvocationLifecycleState, invocation_handle_success,
};
use golem_common::model::oplog::types::SerializableInvocationStatus;
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::proto::golem::common::Empty;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use test_r::test;

fn success(result: invocation_handle_success::Result) -> InvocationHandleSuccess {
    InvocationHandleSuccess {
        result: Some(result),
    }
}

fn status(state: InvocationLifecycleState) -> InvocationHandleSuccess {
    success(invocation_handle_success::Result::Status(state as i32))
}

fn output(result: invocation_session_result::Result) -> InvocationSessionResult {
    InvocationSessionResult {
        result: Some(result),
        ..Default::default()
    }
}

fn unestablished() -> WorkerExecutorError {
    WorkerExecutorError::runtime(
        "authority over the invocation could not be established; try again",
    )
}

#[test]
fn status_records_every_lifecycle_state_including_background_failure() {
    assert_eq!(
        status_outcome(Ok(status(InvocationLifecycleState::BackgroundFailed))).unwrap(),
        Ok(SerializableInvocationStatus::BackgroundFailed)
    );
    assert_eq!(
        status_outcome(Ok(status(InvocationLifecycleState::Scheduled))).unwrap(),
        Ok(SerializableInvocationStatus::Scheduled)
    );
    assert!(status_outcome(Ok(status(InvocationLifecycleState::Unspecified))).is_err());
}

#[test]
fn authorization_refusals_are_recorded_and_operational_failures_trap() {
    assert_eq!(
        status_outcome(Err(WorkerExecutorError::permission_denied(
            "not the starter"
        )))
        .unwrap(),
        Err(RpcError::Denied {
            details: "not the starter".to_string()
        })
    );
    assert!(status_outcome(Err(unestablished())).is_err());
    assert!(await_outcome(Err(unestablished())).is_err());
    assert!(cancel_outcome(Err(unestablished())).is_err());
    assert!(
        cancel_outcome(Err(WorkerExecutorError::AgentNotFound {
            agent_id: crate::test_agent_id("target")
        }))
        .is_err()
    );
}

#[test]
fn not_found_is_recorded_for_every_operation() {
    let not_found = || success(invocation_handle_success::Result::NotFound(Empty {}));
    assert!(matches!(
        status_outcome(Ok(not_found())).unwrap(),
        Err(RpcError::NotFound { .. })
    ));
    assert!(matches!(
        await_outcome(Ok(not_found())).unwrap(),
        Err(RpcError::NotFound { .. })
    ));
    assert!(matches!(
        cancel_outcome(Ok(not_found())).unwrap(),
        Err(RpcError::NotFound { .. })
    ));
}

#[test]
fn cancel_records_its_decision_and_a_running_refusal() {
    assert_eq!(
        cancel_outcome(Ok(success(invocation_handle_success::Result::Cancelled(
            false
        ))))
        .unwrap(),
        Ok(false)
    );
    assert!(matches!(
        cancel_outcome(Err(WorkerExecutorError::CannotCancelRunningInvocation)).unwrap(),
        Err(RpcError::CannotCancelRunning { .. })
    ));
}

#[test]
fn await_of_a_cancelled_invocation_records_cancelled() {
    let failure = success(invocation_handle_success::Result::InvocationFailure(
        WorkerExecutorError::InvocationCancelled.into(),
    ));
    assert!(matches!(
        await_outcome(Ok(failure)).unwrap(),
        Err(RpcError::Cancelled { .. })
    ));
}

#[test]
fn await_records_a_unit_result_and_refuses_streams_and_tool_results() {
    assert_eq!(
        await_outcome(Ok(success(invocation_handle_success::Result::Output(
            output(invocation_session_result::Result::NoResult(Empty {}))
        ))))
        .unwrap(),
        Ok(SchemaValue::Tuple {
            elements: Vec::new()
        })
    );
    let mut streaming = output(invocation_session_result::Result::NoResult(Empty {}));
    streaming.new_stream_mappings = vec![DurableStreamMapping::default()];
    assert!(matches!(
        await_outcome(Ok(success(invocation_handle_success::Result::Output(
            streaming
        ))))
        .unwrap(),
        Err(RpcError::ProtocolError { .. })
    ));
    assert!(matches!(
        await_outcome(Ok(success(invocation_handle_success::Result::Output(
            output(invocation_session_result::Result::ToolResult(
                Default::default()
            ))
        ))))
        .unwrap(),
        Err(RpcError::ProtocolError { .. })
    ));
}

#[test]
fn a_result_of_another_operation_traps() {
    assert!(await_outcome(Ok(status(InvocationLifecycleState::Completed))).is_err());
    assert!(
        status_outcome(Ok(success(invocation_handle_success::Result::Cancelled(
            true
        ))))
        .is_err()
    );
    assert!(cancel_outcome(Ok(InvocationHandleSuccess { result: None })).is_err());
}
