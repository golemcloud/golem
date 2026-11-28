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
use crate::service::worker::OplogRangeRead;
use test_r::test;

#[test]
async fn an_oplog_page_with_an_end_needs_permission_only_up_to_its_end() {
    let harness = RestHarness::new(AgentMode::Durable);
    harness
        .worker_client
        .set_metadata_component_revision(harness.component_revision);
    let agent_id = harness.keyed_agent_id();
    let owned = harness.owned(&agent_id);
    let auth_ctx = auth_ctx_with_permissions(vec![oplog_target(
        &agent_id,
        OplogResourcePattern::range(Some(5), Some(7)),
    )]);
    let get_oplog = |to: Option<u64>, cursor| {
        harness.worker_service.get_oplog(
            &owned,
            OplogIndex::from_u64(5),
            cursor,
            50,
            None,
            to.map(|to| OplogRangeRead {
                to: OplogIndex::from_u64(to),
                allowance: None,
            }),
            ClaimVisibility::Hidden,
            auth_ctx.clone(),
        )
    };

    get_oplog(Some(7), None).await.unwrap();
    assert!(
        get_oplog(None, None).await.is_err(),
        "a page of 50 entries from index 5 reads past the permitted index 7"
    );
    assert!(get_oplog(Some(8), None).await.is_err());

    let prefix_cursor = OplogCursor {
        next_oplog_index: 2,
        current_component_revision: 1,
        query_hash: OplogCursor::query_hash(None),
        scan_to: Some(5),
    };
    get_oplog(Some(7), Some(prefix_cursor)).await.unwrap();
}
