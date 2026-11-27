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
fn an_expired_oplog_keeps_its_retained_start_through_grpc() {
    let expected =
        WorkerExecutorError::oplog_expired(AgentId::from_parts(7, 1), OplogIndex::from_u64(42));

    let status: Status = expected.clone().into();

    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(status.message().contains("starts at index 42"));
    assert_eq!(
        WorkerExecutorError::from_status_details(&status),
        Some(expected)
    );
}

#[test]
fn an_unreadable_oplog_still_crosses_grpc_as_its_text() {
    let failed = WorkerExecutorError::oplog_failed(AgentId::from_parts(7, 1), "gap");

    assert_eq!(
        WorkerExecutorError::try_from(golem::worker::v1::WorkerExecutionError::from(
            failed.clone()
        )),
        Ok(WorkerExecutorError::Runtime {
            details: failed.to_string()
        })
    );
}
