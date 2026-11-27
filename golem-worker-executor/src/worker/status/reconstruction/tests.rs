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
use golem_common::model::regions::OplogRegion;
use test_r::test;

/// A processor checkpoint in a deleted region is not part of the history, so its missing
/// delivery snapshot is no read error. The same checkpoint outside the region is one.
#[test]
fn a_checkpoint_in_a_deleted_region_needs_no_delivery_snapshot() {
    let owned_agent_id = OwnedAgentId::new(
        golem_common::model::environment::EnvironmentId::new(),
        &golem_common::model::AgentId::from_parts(7, 1628),
    );
    let index = OplogIndex::from_u64(5);
    let entries = BTreeMap::from([(
        index,
        OplogEntry::OplogProcessorCheckpoint {
            timestamp: Timestamp::now_utc(),
            plugin_grant_id: EnvironmentPluginGrantId::new(),
            target_agent_id: owned_agent_id.agent_id,
            confirmed_up_to: OplogIndex::NONE,
            sending_up_to: OplogIndex::from_u64(3),
            last_batch_start: OplogIndex::INITIAL,
            source_component_revision: ComponentRevision::INITIAL,
            delivery: None,
        },
    )]);
    let deleted = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(4),
        end: OplogIndex::from_u64(6),
    }]);

    assert_eq!(
        validate_delivery_snapshots(&owned_agent_id, &deleted, &entries),
        Ok(())
    );
    let error = validate_delivery_snapshots(&owned_agent_id, &DeletedRegions::new(), &entries)
        .expect_err("a retained in-flight checkpoint needs its delivery snapshot");
    assert_eq!(error.agent_id, owned_agent_id.agent_id);
}
