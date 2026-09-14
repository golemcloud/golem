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

mod db;
mod etcd;

pub use db::DbRoutingTablePersistence;
pub use etcd::{EtcdRoutingTablePersistence, STATE_KEY};

use super::error::ShardManagerError;
use super::model::ShardLeaseState;
use async_trait::async_trait;
use golem_common::serialization::try_deserialize;

/// An opaque, backend-assigned version of the persisted [`ShardLeaseState`].
///
/// A *storage* fencing token, deliberately unrelated to [`super::model::ShardLeaseRevision`],
/// which lives inside the blob and is bumped when the routing table changes. The two drift apart
/// by design and must never be compared or derived from each other.
///
/// * [`NO_REVISION`] means "no state is stored"; any stored state carries `>= 1`.
/// * Successive successful writes return strictly increasing values. The magnitude is meaningless:
///   the SQL backend assigns `previous + 1`, etcd assigns its cluster-wide revision, which jumps.
///
/// Monotonicity holds only while state remains stored: the SQL token is derived from the row, so
/// deleting it restarts the sequence at 1 while etcd's keeps climbing. A writer holding a token
/// from before such a deletion is therefore not fenced by it.
pub type ExternalRevision = i64;

/// The revision reported by [`RoutingTablePersistence::read`] when nothing is stored, and the
/// only value [`RoutingTablePersistence::write`] accepts for a write that must create the state.
pub const NO_REVISION: ExternalRevision = 0;

#[async_trait]
pub trait RoutingTablePersistence: Send + Sync {
    /// Loads the persisted shard lease state together with the revision it is stored at.
    ///
    /// A stored blob that cannot be decoded, or that violates the state invariants, is an error.
    /// It is never silently replaced by a default state - that would drop a live routing table
    /// on a transient decoding bug.
    async fn read(&self) -> Result<(ShardLeaseState, ExternalRevision), ShardManagerError>;

    /// Stores `shard_state`, but only if the currently stored revision is exactly
    /// `prev_revision`. Returns the revision it was stored at.
    ///
    /// * `prev_revision == NO_REVISION` means **"no state must exist yet"**. The write succeeds
    ///   only if nothing is stored, and creates it. If state does exist, at any revision, the
    ///   write fails - it never overwrites.
    /// * `prev_revision > NO_REVISION` means **"the stored state must still be the one I read at
    ///   that revision"**. In particular, a write with `prev_revision > NO_REVISION` against
    ///   absent state fails; it does not resurrect it.
    ///
    /// Otherwise returns [`ShardManagerError::ConcurrentModification`] having stored nothing.
    /// Recovery is always re-read, re-derive, write; retrying the same `prev_revision` can never
    /// succeed, which is why the error is non-retriable.
    ///
    /// The returned revision is always `>= 1` and always strictly greater than `prev_revision`.
    async fn write(
        &self,
        shard_state: &ShardLeaseState,
        prev_revision: ExternalRevision,
    ) -> Result<ExternalRevision, ShardManagerError>;
}

/// Decodes a persisted state blob and refuses to load one that violates the state invariants.
fn decode_shard_state(bytes: &[u8]) -> Result<ShardLeaseState, ShardManagerError> {
    let shard_state: ShardLeaseState = try_deserialize(bytes)
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
}

/// Refuses to persist a state that violates [`ShardLeaseState::check_invariants`].
///
/// Called by every backend before any I/O, so an invalid state is refused identically everywhere
/// rather than by whichever constraint a backend happens to enforce.
fn check_state_for_write(shard_state: &ShardLeaseState) -> Result<(), ShardManagerError> {
    shard_state.check_invariants().map_err(|violation| {
        ShardManagerError::Internal(format!(
            "refusing to persist a shard lease state that violates invariants: {violation}"
        ))
    })
}

/// Rejects a revision a backend claims to have stored at, if it would be indistinguishable from
/// "absent" or would break monotonicity.
fn check_stored_revision(
    revision: ExternalRevision,
    prev_revision: ExternalRevision,
) -> Result<ExternalRevision, ShardManagerError> {
    if revision < 1 || revision <= prev_revision {
        return Err(ShardManagerError::Internal(format!(
            "persistence backend reported revision {revision} after a write guarded on \
             {prev_revision}, which is not a valid successor"
        )));
    }
    Ok(revision)
}

/// Rejects a `prev_revision` that cannot have come from this layer.
fn check_prev_revision(prev_revision: ExternalRevision) -> Result<(), ShardManagerError> {
    if prev_revision < NO_REVISION {
        return Err(ShardManagerError::Internal(format!(
            "negative previous revision {prev_revision}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::sharding::model::{ExecutorAddr, ExecutorId, ShardAssignmentEntry, ShardEpoch};
    use chrono::{DateTime, Utc};
    use golem_common::model::{Pod, ShardId};
    use golem_common::serialization::{
        SERIALIZATION_VERSION_V1, SERIALIZATION_VERSION_V2, serialize,
    };
    use std::net::{IpAddr, Ipv4Addr};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;
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

    #[test]
    fn roundtrips() {
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

        let bytes = serialize(&shard_state).unwrap();
        let decoded = decode_shard_state(&bytes).unwrap();
        assert_eq!(decoded, shard_state);
    }

    #[test]
    fn state_violating_invariants_is_rejected() {
        let mut shard_state = ShardLeaseState::new(16);
        shard_state.shard_assignments.insert(
            ShardId::new(0),
            ShardAssignmentEntry {
                executor_id: ExecutorId(Uuid::from_u128(7)),
                epoch: ShardEpoch::initial(),
            },
        );
        let bytes = serialize(&shard_state).unwrap();
        match decode_shard_state(&bytes) {
            Err(ShardManagerError::SerializationError(msg)) => {
                assert!(msg.contains("violates invariants"), "{msg}");
            }
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }

    #[test]
    fn empty_blob_is_rejected() {
        match decode_shard_state(&[]) {
            Err(ShardManagerError::SerializationError(msg)) => {
                assert!(msg.contains("empty"), "{msg}");
            }
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }

    #[test]
    fn truncated_blob_is_rejected() {
        let bytes = [3u8, 0u8];
        match decode_shard_state(&bytes) {
            Err(ShardManagerError::SerializationError(_)) => {}
            other => panic!("expected SerializationError, got {other:?}"),
        }
    }

    #[test]
    fn a_dropped_serialization_version_is_rejected() {
        for version in [SERIALIZATION_VERSION_V1, SERIALIZATION_VERSION_V2] {
            let bytes = [version, 0u8, 0u8, 0u8];
            match decode_shard_state(&bytes) {
                Err(ShardManagerError::SerializationError(msg)) => {
                    assert!(msg.contains("no longer supported"), "{msg}");
                }
                other => panic!("expected SerializationError, got {other:?}"),
            }
        }
    }

    const POD_NAME: &str = "worker-executor-0";

    /// A valid blob whose `pod_name` length prefix has been made negative.
    ///
    /// desert encodes a string's length as a zigzag varint, so a name under 64 bytes is prefixed
    /// by the single byte `2 * len`; setting its low bit decodes as `-(len + 1)`, which reaches
    /// `read_bytes` as a `usize` near `usize::MAX`.
    fn blob_with_a_negative_pod_name_length() -> Vec<u8> {
        let mut shard_state = ShardLeaseState::new(16);
        shard_state.add_executor(
            ExecutorId(Uuid::from_u128(1)),
            ExecutorAddr::from(pod(1, 9010)),
            Some(POD_NAME.to_string()),
            t0(),
            TTL,
        );
        let mut bytes = serialize(&shard_state).unwrap();

        let at = bytes
            .windows(POD_NAME.len())
            .position(|window| window == POD_NAME.as_bytes())
            .expect("the pod name is stored verbatim");
        let length_prefix = at - 1;
        assert_eq!(bytes[length_prefix], (POD_NAME.len() as u8) << 1);
        bytes[length_prefix] |= 1;
        bytes
    }

    fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
        payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>")
    }

    /// Characterisation, not a guarantee: a negative length prefix crashes rather than errors.
    ///
    /// Catchable only because cargo forces `panic = "unwind"` on test targets; the profiles the
    /// shard manager ships in abort the process instead. The panic message on stderr is expected.
    #[test]
    fn a_negative_length_prefix_panics_the_decoder() {
        let blob = blob_with_a_negative_pod_name_length();
        let caught = catch_unwind(AssertUnwindSafe(|| decode_shard_state(&blob)));
        match caught {
            // With overflow checks on the add traps; without them the following slice index
            // panics instead.
            Err(payload) => {
                let message = panic_message(&*payload);
                assert!(
                    message.contains("attempt to add with overflow")
                        || message.contains("slice index starts at"),
                    "the panic did not come from desert's bounds check: {message}"
                );
            }
            Ok(Ok(_)) => panic!("a negative length prefix decoded successfully"),
            Ok(Err(err)) => panic!(
                "a negative length prefix now returns {err:?}; if the decoder rejects it, replace \
                 this characterisation with an assertion on the error and restore the guard"
            ),
        }
    }

    #[test]
    fn a_state_violating_invariants_is_refused_for_write() {
        let mut shard_state = ShardLeaseState::new(16);
        shard_state.shard_assignments.insert(
            ShardId::new(0),
            ShardAssignmentEntry {
                executor_id: ExecutorId(Uuid::from_u128(7)),
                epoch: ShardEpoch::initial(),
            },
        );
        match check_state_for_write(&shard_state) {
            Err(ShardManagerError::Internal(msg)) => {
                assert!(msg.contains("violates invariants"), "{msg}");
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn stored_revision_must_be_a_valid_successor() {
        assert_eq!(check_stored_revision(1, NO_REVISION).unwrap(), 1);
        assert_eq!(check_stored_revision(9, 4).unwrap(), 9);
        // Indistinguishable from "absent".
        assert!(check_stored_revision(0, NO_REVISION).is_err());
        // Not monotonic.
        assert!(check_stored_revision(4, 4).is_err());
        assert!(check_stored_revision(3, 4).is_err());
    }
}
