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

use super::super::oplog_page_window;
use super::*;
use test_r::test;

#[test]
async fn an_oplog_page_without_a_start_requires_permission_for_the_entries_it_gives() {
    let harness = RestHarness::new(AgentMode::Durable);
    harness
        .worker_client
        .set_metadata_component_revision(harness.component_revision);
    let agent_id = harness.keyed_agent_id();
    let owned = harness.owned(&agent_id);
    let auth_ctx = auth_ctx_with_permissions(vec![oplog_target(
        &agent_id,
        OplogResourcePattern::range(Some(1), Some(3)),
    )]);

    harness
        .worker_service
        .get_oplog(
            &owned,
            None,
            None,
            3,
            None,
            None,
            ClaimVisibility::Hidden,
            auth_ctx.clone(),
        )
        .await
        .unwrap();

    *harness.worker_client.oplog_page_first_index.lock().unwrap() = 10;
    assert!(
        harness
            .worker_service
            .get_oplog(
                &owned,
                None,
                None,
                3,
                None,
                None,
                ClaimVisibility::Hidden,
                auth_ctx,
            )
            .await
            .is_err(),
        "the oplog starts at index 10, after the range of the permission"
    );
}

/// The window of a page read is `count` indexes from its start, cut at the `to` of a range
/// read. A window whose end does not fit in a `u64` has no end.
#[test]
fn an_oplog_page_window_is_count_indexes_from_its_start_cut_at_to() {
    let range = |start: u64, end: Option<u64>| OplogResourcePattern::range(Some(start), end);
    assert_eq!(oplog_page_window(1, 3, None), range(1, Some(3)));
    assert_eq!(oplog_page_window(10, 3, Some(11)), range(10, Some(11)));
    assert_eq!(oplog_page_window(10, 3, Some(12)), range(10, Some(12)));
    assert_eq!(oplog_page_window(10, 3, Some(13)), range(10, Some(12)));
    assert_eq!(oplog_page_window(10, 0, None), range(10, Some(10)));
    assert_eq!(oplog_page_window(u64::MAX, 2, None), range(u64::MAX, None));
}

#[test]
async fn an_oplog_page_without_a_start_only_requires_the_effective_window() {
    let harness = RestHarness::new(AgentMode::Durable);
    harness
        .worker_client
        .set_metadata_component_revision(harness.component_revision);
    let agent_id = harness.keyed_agent_id();
    let owned = harness.owned(&agent_id);
    let auth_ctx = auth_ctx_with_permissions(vec![oplog_target(
        &agent_id,
        OplogResourcePattern::range(Some(10), Some(12)),
    )]);

    *harness.worker_client.oplog_page_first_index.lock().unwrap() = 10;
    let page = harness
        .worker_service
        .get_oplog(
            &owned,
            None,
            None,
            3,
            None,
            None,
            ClaimVisibility::Hidden,
            auth_ctx,
        )
        .await
        .unwrap();
    assert_eq!(page.first_index_in_chunk, 10);
}
