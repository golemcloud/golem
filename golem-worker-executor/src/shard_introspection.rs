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

//! `GET /shard-assignment` — what *this* executor believes it owns (GOL-364).
//!
//! The shard-manager's routing table is the cluster's intended assignment. It is
//! not the same thing as what each executor actually believes, and the gap
//! between the two is the whole point of this endpoint.
//!
//! Under a network partition an executor keeps serving the shards it was last
//! told about, because it has no way to learn otherwise — that is correct
//! behaviour, not a bug. What must never happen is two executors believing they
//! own the same shard once the partition heals, because an agent with two owners
//! is an agent whose state can fork. Detecting that needs each executor's *own*
//! view; asking the shard-manager only ever returns the shard-manager's opinion
//! of everyone, which is precisely the opinion under test.
//!
//! ### Why the health/metrics port
//!
//! This is read-only introspection of one process's belief about itself, which
//! is what that port already carries. The gRPC API is how the platform's own
//! components talk to each other, and a partition is exactly the condition under
//! which those paths are unreliable — an introspection endpoint that could be
//! cut off by the fault it exists to observe would be useless.

use crate::identity::executor_id;
use crate::services::shard::ShardService;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// One executor's own view of what it owns.
///
/// Stable enough to be consumed by external tooling: this is the contract the
/// chaos suite's ownership oracle reads, and archived samples of it are compared
/// across runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardAssignmentSnapshot {
    /// Pod name of the responding executor, from `POD_NAME`/`HOSTNAME`.
    ///
    /// Present so a caller reaching several executors through forwarded ports
    /// can confirm it is talking to the one it thinks it is. A crossed forward
    /// would otherwise produce a perfectly plausible — and completely wrong —
    /// ownership picture.
    pub executor_id: String,
    /// Whether this executor has been assigned anything yet.
    ///
    /// `false` is a real answer, not an error: a freshly started executor, or
    /// one that has been cut off since before it first registered, genuinely
    /// owns nothing. Reporting that as a failed read would hide it.
    pub assigned: bool,
    /// Total shards in the cluster, as this executor last understood it. Two
    /// executors disagreeing about *this* is a different and more serious
    /// finding than disagreeing about who owns what.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number_of_shards: Option<usize>,
    /// How many shards this executor believes it owns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shard_count: Option<usize>,
    /// The shard ids themselves, sorted so two snapshots can be compared
    /// directly and a diff is readable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shard_ids: Vec<i64>,
}

impl ShardAssignmentSnapshot {
    fn from_service(shard_service: &dyn ShardService) -> Self {
        match shard_service.try_get_current_assignment() {
            Some(assignment) => {
                let mut shard_ids: Vec<i64> =
                    assignment.shard_ids.iter().map(|id| id.value()).collect();
                shard_ids.sort_unstable();
                Self {
                    executor_id: executor_id().to_string(),
                    assigned: true,
                    number_of_shards: Some(assignment.number_of_shards),
                    shard_count: Some(shard_ids.len()),
                    shard_ids,
                }
            }
            None => Self {
                executor_id: executor_id().to_string(),
                assigned: false,
                number_of_shards: None,
                shard_count: None,
                shard_ids: Vec::new(),
            },
        }
    }
}

/// The introspection routes, to be merged onto the health/metrics listener.
pub fn router(shard_service: Arc<dyn ShardService>) -> Router {
    Router::new()
        .route("/shard-assignment", get(shard_assignment))
        .with_state(shard_service)
}

async fn shard_assignment(
    State(shard_service): State<Arc<dyn ShardService>>,
) -> Json<ShardAssignmentSnapshot> {
    Json(ShardAssignmentSnapshot::from_service(
        shard_service.as_ref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::shard::ShardServiceDefault;
    use golem_common::model::ShardId;
    use std::collections::HashSet;
    use test_r::test;

    #[test]
    fn an_unassigned_executor_reports_that_rather_than_failing() {
        let service = ShardServiceDefault::new();
        let snapshot = ShardAssignmentSnapshot::from_service(&service);

        assert!(!snapshot.assigned);
        assert_eq!(snapshot.number_of_shards, None);
        assert!(snapshot.shard_ids.is_empty());
    }

    /// The ownership oracle compares these lists directly, so a stable order is
    /// part of the contract rather than an implementation detail.
    #[test]
    fn shard_ids_are_reported_sorted() {
        let service = ShardServiceDefault::new();
        service.register(
            1024,
            &HashSet::from([ShardId::new(9), ShardId::new(2), ShardId::new(41)]),
        );

        let snapshot = ShardAssignmentSnapshot::from_service(&service);
        assert!(snapshot.assigned);
        assert_eq!(snapshot.number_of_shards, Some(1024));
        assert_eq!(snapshot.shard_count, Some(3));
        assert_eq!(snapshot.shard_ids, vec![2, 9, 41]);
    }

    /// A caller reaching several executors through forwarded ports uses this to
    /// confirm which one answered.
    #[test]
    fn a_snapshot_names_the_executor_that_produced_it() {
        let service = ShardServiceDefault::new();
        let snapshot = ShardAssignmentSnapshot::from_service(&service);
        assert_eq!(snapshot.executor_id, executor_id());
    }

    /// The chaos oracle parses this shape out of an archived run, so the JSON
    /// field names are part of the contract, not an internal detail.
    #[test]
    fn the_json_shape_is_camel_case_and_omits_absent_fields() {
        let service = ShardServiceDefault::new();
        let json = serde_json::to_value(ShardAssignmentSnapshot::from_service(&service)).unwrap();
        assert!(json.get("executorId").is_some());
        assert_eq!(json.get("assigned"), Some(&serde_json::Value::Bool(false)));
        assert!(
            json.get("numberOfShards").is_none(),
            "an unassigned executor must omit the field rather than report zero shards"
        );

        service.register(64, &HashSet::from([ShardId::new(1)]));
        let json = serde_json::to_value(ShardAssignmentSnapshot::from_service(&service)).unwrap();
        assert_eq!(json["numberOfShards"], 64);
        assert_eq!(json["shardCount"], 1);
        assert_eq!(json["shardIds"], serde_json::json!([1]));
    }
}
