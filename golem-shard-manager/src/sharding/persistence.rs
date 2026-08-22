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

use super::error::ShardManagerError;
use super::model::ShardLeaseState;
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use conditional_trait_gen::trait_gen;
use golem_common::serialization::{SERIALIZATION_VERSION_V3, serialize, try_deserialize};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::RepoError;
use sqlx::Row;
use std::time::Duration;
use tracing::warn;

const PERSISTENCE_SVC: &str = "persistence";

const SHARD_LEASE_STATE_MAGIC: &[u8; 4] = b"GSL1";

#[async_trait]
pub trait RoutingTablePersistence: Send + Sync {
    async fn write(&self, shard_state: &ShardLeaseState) -> Result<(), ShardManagerError>;
    async fn read(&self) -> Result<ShardLeaseState, ShardManagerError>;
}

pub struct DbRoutingTablePersistence<DBP: Pool> {
    pool: DBP,
    number_of_shards: usize,
    lease_ttl: Duration,
}

impl<DBP: Pool> DbRoutingTablePersistence<DBP> {
    pub fn new(pool: DBP, number_of_shards: usize, lease_ttl: Duration) -> Self {
        Self {
            pool,
            number_of_shards,
            lease_ttl,
        }
    }
}

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
#[async_trait]
impl RoutingTablePersistence for DbRoutingTablePersistence<PostgresPool> {
    async fn write(&self, shard_state: &ShardLeaseState) -> Result<(), ShardManagerError> {
        let encoded = encode_shard_state(shard_state)?;

        self.pool
            .with_rw(PERSISTENCE_SVC, "write")
            .execute(
                sqlx::query(
                    "INSERT INTO shard_manager_state (id, state) VALUES (1, $1) \
                     ON CONFLICT (id) DO UPDATE SET state = EXCLUDED.state",
                )
                .bind(encoded),
            )
            .await
            .map_err(ShardManagerError::RepoError)?;

        Ok(())
    }

    async fn read(&self) -> Result<ShardLeaseState, ShardManagerError> {
        let row = self
            .pool
            .with_ro(PERSISTENCE_SVC, "read")
            .fetch_optional(sqlx::query(
                "SELECT state FROM shard_manager_state WHERE id = 1",
            ))
            .await
            .map_err(ShardManagerError::RepoError)?;

        if let Some(row) = row {
            let bytes: Vec<u8> = row
                .try_get("state")
                .map_err(|err| RepoError::InternalError(anyhow!(err)))?;
            decode_shard_state(&bytes, Utc::now(), self.lease_ttl)
        } else {
            Ok(ShardLeaseState::new(self.number_of_shards))
        }
    }
}

pub(crate) fn encode_shard_state(
    shard_state: &ShardLeaseState,
) -> Result<Vec<u8>, ShardManagerError> {
    let encoded = serialize(shard_state).map_err(ShardManagerError::SerializationError)?;
    let mut bytes = Vec::with_capacity(SHARD_LEASE_STATE_MAGIC.len() + encoded.len());
    bytes.extend_from_slice(SHARD_LEASE_STATE_MAGIC);
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

pub(crate) fn decode_shard_state(
    bytes: &[u8],
    now: DateTime<Utc>,
    lease_ttl: Duration,
) -> Result<ShardLeaseState, ShardManagerError> {
    if let Some(encoded) = bytes.strip_prefix(SHARD_LEASE_STATE_MAGIC) {
        let shard_state: ShardLeaseState = try_deserialize(encoded)
            .map_err(ShardManagerError::SerializationError)?
            .ok_or_else(|| {
                ShardManagerError::SerializationError(
                    "persisted shard lease state is empty or has an unknown serialization version"
                        .to_string(),
                )
            })?;
        shard_state.check_invariants().map_err(|violation| {
            ShardManagerError::SerializationError(format!(
                "persisted shard lease state violates invariants: {violation}"
            ))
        })?;
        Ok(shard_state)
    } else {
        match bytes.first() {
            None => {
                return Err(ShardManagerError::SerializationError(
                    "persisted state is empty".to_string(),
                ));
            }
            Some(&SERIALIZATION_VERSION_V3) => {}
            Some(other) => {
                return Err(ShardManagerError::SerializationError(format!(
                    "persisted state is neither a shard lease state nor a legacy routing table (leading byte {other:#04x})"
                )));
            }
        }
        let routing_table: legacy::RoutingTable = try_deserialize(bytes)
            .map_err(|err| {
                ShardManagerError::SerializationError(format!(
                    "persisted state is neither a shard lease state nor a legacy routing table: {err}"
                ))
            })?
            .ok_or_else(|| {
                ShardManagerError::SerializationError(
                    "persisted state is empty or has an unknown serialization version".to_string(),
                )
            })?;
        warn!(
            pods = routing_table.pod_states.len(),
            number_of_shards = routing_table.number_of_shards,
            "Converted legacy routing table blob to shard lease state; every pod received a fresh executor id and lease"
        );
        let shard_state = routing_table.into_shard_state(now, lease_ttl);
        shard_state.check_invariants().map_err(|violation| {
            ShardManagerError::SerializationError(format!(
                "legacy routing table converted into an invalid shard lease state: {violation}"
            ))
        })?;
        Ok(shard_state)
    }
}

// TODO(shard manager redesign, ticket 2): remove together with the CAS persistence migration
mod legacy {
    use super::*;
    use crate::sharding::model::ExecutorId;
    use desert_rust::BinaryCodec;
    use golem_common::model::{Pod, ShardId};
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
    #[desert(evolution())]
    pub(super) struct PodState {
        pub pod_name: Option<String>,
        pub assigned_shards: BTreeSet<ShardId>,
    }

    #[derive(Clone, Debug, Eq, PartialEq, BinaryCodec)]
    #[desert(evolution())]
    pub(super) struct RoutingTable {
        pub number_of_shards: usize,
        pub pod_states: BTreeMap<Pod, PodState>,
    }

    impl RoutingTable {
        pub(super) fn into_shard_state(
            self,
            now: DateTime<Utc>,
            lease_ttl: Duration,
        ) -> ShardLeaseState {
            let mut shard_state = ShardLeaseState::new(self.number_of_shards);
            for (pod, pod_state) in self.pod_states {
                let executor_id = ExecutorId::generate();
                shard_state.add_executor(
                    executor_id,
                    pod.into(),
                    pod_state.pod_name,
                    now,
                    lease_ttl,
                );
                for shard_id in pod_state.assigned_shards {
                    if !shard_state.contains_shard(shard_id) {
                        warn!(
                            pod = %pod,
                            shard_id = %shard_id,
                            number_of_shards = self.number_of_shards,
                            "Dropping out-of-range shard assignment from legacy routing table"
                        );
                        continue;
                    }
                    shard_state.assign_shard(executor_id, shard_id);
                }
            }
            shard_state
        }
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::sharding::model::{ExecutorAddr, ExecutorId, ShardEpoch, ShardLeaseRevision};
    use golem_common::model::{Pod, ShardId};
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::{IpAddr, Ipv4Addr};
    use uuid::Uuid;

    const TTL: Duration = Duration::from_secs(60);

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn pod(last_octet: u8, port: u16) -> Pod {
        Pod {
            ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, last_octet)),
            port,
        }
    }

    fn shards(ids: &[i64]) -> BTreeSet<ShardId> {
        ids.iter().copied().map(ShardId::new).collect()
    }

    #[test]
    fn new_format_roundtrips() {
        let mut shard_state = ShardLeaseState::new(16);
        shard_state.add_executor(
            ExecutorId(Uuid::from_u128(1)),
            ExecutorAddr::from(pod(1, 9010)),
            Some("worker-executor-0".to_string()),
            t0(),
            TTL,
        );
        shard_state.assign_shard(ExecutorId(Uuid::from_u128(1)), ShardId::new(3));
        shard_state.bump_revision().unwrap();

        let bytes = encode_shard_state(&shard_state).unwrap();
        assert!(bytes.starts_with(SHARD_LEASE_STATE_MAGIC));
        let decoded = decode_shard_state(&bytes, t0(), TTL).unwrap();
        assert_eq!(decoded, shard_state);
    }

    #[test]
    fn new_format_violating_invariants_is_rejected() {
        let mut shard_state = ShardLeaseState::new(16);
        shard_state.shard_assignments.insert(
            ShardId::new(0),
            crate::sharding::model::ShardAssignmentEntry {
                executor_id: ExecutorId(Uuid::from_u128(7)),
                epoch: ShardEpoch::initial(),
            },
        );
        let bytes = encode_shard_state(&shard_state).unwrap();
        match decode_shard_state(&bytes, t0(), TTL) {
            Err(ShardManagerError::SerializationError(msg)) => {
                assert!(msg.contains("violates invariants"), "{msg}");
            }
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }

    #[test]
    fn truncated_new_format_is_rejected() {
        let bytes: Vec<u8> = SHARD_LEASE_STATE_MAGIC
            .iter()
            .copied()
            .chain([3u8, 0u8])
            .collect();
        match decode_shard_state(&bytes, t0(), TTL) {
            Err(ShardManagerError::SerializationError(_)) => {}
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }

    #[test]
    fn legacy_blob_is_converted_with_fresh_leases() {
        let mut pod_states = BTreeMap::new();
        pod_states.insert(
            pod(1, 9010),
            legacy::PodState {
                pod_name: Some("worker-executor-0".to_string()),
                assigned_shards: shards(&[0, 1, 2]),
            },
        );
        pod_states.insert(
            pod(2, 9011),
            legacy::PodState {
                pod_name: None,
                assigned_shards: shards(&[3, 4]),
            },
        );
        let legacy_blob = serialize(&legacy::RoutingTable {
            number_of_shards: 16,
            pod_states,
        })
        .unwrap();

        let shard_state = decode_shard_state(&legacy_blob, t0(), TTL).unwrap();

        assert_eq!(shard_state.number_of_shards, 16);
        assert_eq!(shard_state.revision, ShardLeaseRevision::INITIAL);
        assert_eq!(shard_state.executor_count(), 2);
        assert!(shard_state.pending_rebalance.is_empty());
        assert!(shard_state.check_invariants().is_ok());

        let first = shard_state
            .executor_for_addr(ExecutorAddr::from(pod(1, 9010)))
            .expect("first pod should have a lease");
        let second = shard_state
            .executor_for_addr(ExecutorAddr::from(pod(2, 9011)))
            .expect("second pod should have a lease");
        assert_ne!(first, second);
        assert_eq!(
            shard_state.shards_for_executor(first),
            Some(shards(&[0, 1, 2]))
        );
        assert_eq!(
            shard_state.shards_for_executor(second),
            Some(shards(&[3, 4]))
        );
        assert_eq!(
            shard_state.executor_leases[&first].pod_name.as_deref(),
            Some("worker-executor-0")
        );
        assert_eq!(shard_state.executor_leases[&first].granted_at, t0());
        assert_eq!(
            shard_state.executor_leases[&first].expires_at,
            t0() + chrono::Duration::seconds(60)
        );
        for shard_id in 0..5 {
            assert_eq!(
                shard_state.epoch_for_shard(ShardId::new(shard_id)),
                Some(ShardEpoch::initial())
            );
        }
    }

    #[test]
    fn legacy_blob_with_out_of_range_shards_is_converted_without_them() {
        let mut pod_states = BTreeMap::new();
        pod_states.insert(
            pod(1, 9010),
            legacy::PodState {
                pod_name: None,
                assigned_shards: shards(&[0, 15, 16, 1000]),
            },
        );
        let legacy_blob = serialize(&legacy::RoutingTable {
            number_of_shards: 16,
            pod_states,
        })
        .unwrap();

        let shard_state = decode_shard_state(&legacy_blob, t0(), TTL).unwrap();
        let executor = shard_state
            .executor_for_addr(ExecutorAddr::from(pod(1, 9010)))
            .unwrap();
        assert_eq!(
            shard_state.shards_for_executor(executor),
            Some(shards(&[0, 15]))
        );
        assert!(shard_state.check_invariants().is_ok());

        // ... and the converted state survives a write/read cycle in the new format
        let bytes = encode_shard_state(&shard_state).unwrap();
        assert_eq!(decode_shard_state(&bytes, t0(), TTL).unwrap(), shard_state);
    }

    #[test]
    fn legacy_blob_with_a_shard_in_two_pods_keeps_one_owner() {
        let mut pod_states = BTreeMap::new();
        pod_states.insert(
            pod(1, 9010),
            legacy::PodState {
                pod_name: None,
                assigned_shards: shards(&[0, 1]),
            },
        );
        pod_states.insert(
            pod(2, 9011),
            legacy::PodState {
                pod_name: None,
                assigned_shards: shards(&[1, 2]),
            },
        );
        let legacy_blob = serialize(&legacy::RoutingTable {
            number_of_shards: 16,
            pod_states,
        })
        .unwrap();

        let shard_state = decode_shard_state(&legacy_blob, t0(), TTL).unwrap();
        assert!(shard_state.check_invariants().is_ok());
        assert_eq!(shard_state.get_unassigned_shards().len(), 13);
        let owners: BTreeSet<ExecutorId> = shard_state
            .shard_assignments
            .values()
            .map(|entry| entry.executor_id)
            .collect();
        assert_eq!(owners.len(), 2);
        assert!(shard_state.epoch_for_shard(ShardId::new(1)).is_some());
    }

    #[test]
    fn empty_legacy_blob_is_converted() {
        let legacy_blob = serialize(&legacy::RoutingTable {
            number_of_shards: 16,
            pod_states: BTreeMap::new(),
        })
        .unwrap();

        let shard_state = decode_shard_state(&legacy_blob, t0(), TTL).unwrap();
        assert_eq!(shard_state, ShardLeaseState::new(16));
    }

    #[test]
    fn empty_and_magic_only_blobs_are_rejected() {
        for bytes in [Vec::new(), SHARD_LEASE_STATE_MAGIC.to_vec()] {
            match decode_shard_state(&bytes, t0(), TTL) {
                Err(ShardManagerError::SerializationError(msg)) => {
                    assert!(msg.contains("empty"), "{msg}");
                }
                other => panic!("expected SerializationError, got {other:?}"),
            }
        }
    }

    #[test]
    fn foreign_blob_with_retired_serialization_version_is_rejected() {
        for version in [0u8, 1, 2, 4] {
            let bytes = vec![version, 0u8, 0u8, 0u8];
            // (version 3 is the legacy format and is exercised by the other tests)
            match decode_shard_state(&bytes, t0(), TTL) {
                Err(ShardManagerError::SerializationError(msg)) => {
                    assert!(msg.contains("leading byte"), "{msg}");
                }
                other => panic!("expected SerializationError, got {other:?}"),
            }
        }
    }

    #[test]
    fn truncated_legacy_blob_is_rejected() {
        let bytes = vec![3u8, 0u8];
        match decode_shard_state(&bytes, t0(), TTL) {
            Err(ShardManagerError::SerializationError(msg)) => {
                assert!(msg.contains("legacy routing table"), "{msg}");
            }
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }
}
