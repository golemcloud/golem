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

/// A first batch of a new search that failed, or that the grant denied, leaves the search
/// new. The next batch starts the search at the retained start 5, and not at the index that
/// the guest gave. A batch that read moves the search to its response.
#[test]
async fn only_a_batch_that_read_moves_a_guest_search() {
    use crate::model::public_oplog::guest_search_start;
    use crate::services::oplog::retention::test_oplog::{RetainedOplog, at, idx, jump};
    use futures::StreamExt;
    use golem_common::model::ShardEpoch;
    use golem_common::model::agent::AgentMode;

    let oplog = RetainedOplog::new().await;
    let handle = oplog.create(oplog.create_entry(), ShardEpoch(1)).await;
    oplog.add((2..=10).map(jump)).await;
    drop(handle);
    oplog.advance(5).await;
    let new_search = || {
        SearchOplogEntry::new(
            oplog.owned_agent_id.clone(),
            OplogIndex::INITIAL,
            ComponentRevision::INITIAL,
            10,
            "query".to_string(),
        )
    };
    let response = |result| HostResponseGolemApiOplogChunk {
        result,
        next_oplog_index: OplogIndex::INITIAL,
        current_component_revision: ComponentRevision::INITIAL,
        last_oplog_index: Some(idx(10)),
    };
    futures::stream::iter([
        Err(AGENT_OPERATION_PERMISSION_DENIED.to_string()),
        Err("the storage failed".to_string()),
    ])
    .for_each(|result| {
        let mut search = new_search();
        search.apply(&response(result));
        let oplog = &oplog;
        async move {
            assert!(!search.initialized);
            assert_eq!(search.last_oplog_index, None);
            assert_eq!(
                guest_search_start(
                    oplog.service.clone(),
                    &oplog.owned_agent_id,
                    AgentMode::Durable,
                    at(5),
                    search.next_oplog_index,
                    search.continued(),
                )
                .await,
                Ok((idx(5), ComponentRevision::INITIAL))
            );
        }
    })
    .await;

    let mut search = new_search();
    search.apply(&HostResponseGolemApiOplogChunk {
        result: Ok(None),
        next_oplog_index: idx(7),
        current_component_revision: ComponentRevision::try_from(2u64).unwrap(),
        last_oplog_index: Some(idx(10)),
    });
    assert!(search.initialized);
    assert_eq!(
        (
            search.next_oplog_index,
            search.current_component_revision,
            search.last_oplog_index
        ),
        (
            idx(7),
            ComponentRevision::try_from(2u64).unwrap(),
            Some(idx(10))
        )
    );
    assert_eq!(
        search.continued(),
        Some(ComponentRevision::try_from(2u64).unwrap())
    );
}

/// A new `get-oplog` read has no revision to continue from, so its first chunk finds the
/// revision at its cursor. After a chunk, the read continues from the revision of that chunk.
#[test]
fn a_get_oplog_read_continues_from_a_revision_only_after_a_chunk() {
    let mut entry = GetOplogEntry::new(
        golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId::new(),
            &golem_common::model::AgentId::from_parts(7, 1),
        ),
        OplogIndex::INITIAL,
        ComponentRevision::INITIAL,
        10,
    );
    assert_eq!(entry.continued(), None);
    entry.update(
        OplogIndex::from_u64(4),
        ComponentRevision::try_from(2u64).unwrap(),
    );
    assert_eq!(
        entry.continued(),
        Some(ComponentRevision::try_from(2u64).unwrap())
    );
}
