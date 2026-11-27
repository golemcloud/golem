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
fn an_expired_oplog_read_maps_to_its_own_code() {
    let api_error: ApiEndpointError =
        WorkerServiceError::GolemError(WorkerExecutorError::oplog_expired(
            golem_common::model::AgentId::from_parts(7, 1),
            golem_common::model::OplogIndex::from_u64(42),
        ))
        .into();

    match api_error {
        ApiEndpointError::BadRequest(Json(body)) => {
            assert_eq!(body.code, "OPLOG_EXPIRED");
            assert!(body.errors.iter().any(|error| error.contains("42")));
        }
        other => panic!("expected BadRequest, got: {other:?}"),
    }
}
