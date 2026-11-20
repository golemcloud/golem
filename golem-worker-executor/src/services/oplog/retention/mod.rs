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

//! The retained start of an agent oplog: the first index that the oplog keeps.
//!
//! Oplog indexes never renumber. A cut of the oplog removes the entries before the retained start,
//! and the start only moves forward. The `Create` entry at index 1 is the identity of the agent,
//! and every reader can read it also when the start is above 1.
//!
//! The rules here are pure functions over the retained start. A read that reaches below the start
//! fails with [`OplogReadError::Expired`], which names the start. A [`OplogReadError::Gap`] below
//! the start becomes the same error. A gap at or after the start stays a read error.

mod layer;

pub use layer::{RetentionOplog, RetentionOplogService};

use crate::services::oplog::{OplogReadError, OplogReadSource, UnreadableOplog};
use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use desert_rust::BinaryCodec;
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::model::AgentId;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::OplogIndex;
use golem_common::serialization::{deserialize, serialize};

/// Where the retained oplog of an agent starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct OplogRetention {
    pub retained_start: OplogIndex,
}

impl Default for OplogRetention {
    fn default() -> Self {
        Self::FULL
    }
}

impl OplogRetention {
    /// The retention of an oplog that keeps every entry.
    pub const FULL: Self = Self {
        retained_start: OplogIndex::INITIAL,
    };

    /// The retention with the start at `to`, or this retention when its start is already at or
    /// after `to`. The start never moves back.
    pub fn advanced_to(self, to: OplogIndex) -> Self {
        Self {
            retained_start: self.retained_start.max(to),
        }
    }

    /// The first index that a read of the whole retained oplog starts at.
    pub fn first_index(self) -> OplogIndex {
        self.retained_start
    }

    /// The number of retained entries of an oplog whose last index is `last`. The `Create` entry
    /// below the start does not count.
    pub fn retained_length(self, last: OplogIndex) -> u64 {
        (last.as_u64() + 1).saturating_sub(self.retained_start.as_u64())
    }

    /// Checks a read of `count` entries from `start`. A read that reaches below the retained start
    /// fails with [`OplogReadError::Expired`]. A read of the `Create` entry alone, at index 1, is
    /// always allowed.
    pub fn check_read(self, start: OplogIndex, count: u64) -> Result<(), OplogReadError> {
        let reads_create_only = start == OplogIndex::INITIAL && count == 1;
        if count == 0 || start >= self.retained_start || reads_create_only {
            Ok(())
        } else {
            Err(self.expired())
        }
    }

    /// The error of a read, with a gap that starts below the retained start given as
    /// [`OplogReadError::Expired`]. Every other error stays as it is.
    pub fn classify(self, error: OplogReadError) -> OplogReadError {
        match error {
            OplogReadError::Gap { start, .. } if start < self.retained_start => self.expired(),
            other => other,
        }
    }

    fn expired(self) -> OplogReadError {
        OplogReadError::Expired {
            retained_start: self.retained_start,
        }
    }
}

const SVC_NAME: &str = "oplog";
const ENTITY_NAME: &str = "oplog_retention";
const SOURCE: OplogReadSource = OplogReadSource::Other("oplog retention");

fn namespace(agent_id: &AgentId) -> KeyValueStorageNamespace {
    KeyValueStorageNamespace::AgentOplogRetention {
        agent_id: agent_id.clone(),
    }
}

fn field(agent_mode: AgentMode) -> String {
    agent_mode.to_string()
}

fn unreadable(agent_id: &AgentId, error: KeyValueStorageError) -> UnreadableOplog {
    let detail = error.to_string();
    UnreadableOplog::new(
        agent_id,
        if error.is_retryable(true) {
            OplogReadError::source_unavailable(SOURCE, detail)
        } else {
            OplogReadError::source_failure(SOURCE, detail)
        },
    )
}

/// The stored retention and its encoded form. An absent record is [`OplogRetention::FULL`].
async fn load(
    storage: &(dyn KeyValueStorage + Send + Sync),
    agent_id: &AgentId,
    agent_mode: AgentMode,
) -> Result<(OplogRetention, Option<Vec<u8>>), UnreadableOplog> {
    let stored = storage
        .get(
            SVC_NAME,
            "read_retention",
            ENTITY_NAME,
            namespace(agent_id),
            &field(agent_mode),
        )
        .await
        .map_err(|error| unreadable(agent_id, error))?;
    stored.map_or(Ok((OplogRetention::FULL, None)), |bytes| {
        deserialize(&bytes)
            .map(|retention| (retention, Some(bytes.to_vec())))
            .map_err(|error| {
                UnreadableOplog::new(
                    agent_id,
                    OplogReadError::corruption(SOURCE, format!("undecodable record: {error}")),
                )
            })
    })
}

/// The retention of the oplog of the agent in `agent_mode`.
pub async fn read_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    agent_id: &AgentId,
    agent_mode: AgentMode,
) -> Result<OplogRetention, UnreadableOplog> {
    Ok(load(storage, agent_id, agent_mode).await?.0)
}

/// Moves the retained start of the oplog of the agent in `agent_mode` forward to `to`, and returns
/// the stored retention. A start that is already at or after `to` stays. The write compares the
/// record it read, so a writer with an old start cannot move the start back.
pub fn advance_oplog_retention<'a>(
    storage: &'a (dyn KeyValueStorage + Send + Sync),
    agent_id: &'a AgentId,
    agent_mode: AgentMode,
    to: OplogIndex,
) -> BoxFuture<'a, Result<OplogRetention, UnreadableOplog>> {
    async move {
        let (current, stored) = load(storage, agent_id, agent_mode).await?;
        let advanced = current.advanced_to(to);
        if advanced == current {
            return Ok(current);
        }
        let encoded = serialize(&advanced).map_err(|error| {
            UnreadableOplog::new(agent_id, OplogReadError::corruption(SOURCE, error))
        })?;
        let field = field(agent_mode);
        let written = storage
            .compare_and_set_many(
                SVC_NAME,
                "advance_retention",
                ENTITY_NAME,
                namespace(agent_id),
                &field,
                stored.as_deref(),
                &[],
                &[(&field, &encoded)],
            )
            .await
            .map_err(|error| unreadable(agent_id, error))?;
        if written {
            Ok(advanced)
        } else {
            advance_oplog_retention(storage, agent_id, agent_mode, to).await
        }
    }
    .boxed()
}

/// Removes the retention record of the oplog of the agent in `agent_mode`.
pub(crate) async fn remove_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    agent_id: &AgentId,
    agent_mode: AgentMode,
) -> Result<(), String> {
    storage
        .del(
            SVC_NAME,
            "remove_retention",
            namespace(agent_id),
            &field(agent_mode),
        )
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn idx(value: u64) -> OplogIndex {
        OplogIndex::from_u64(value)
    }

    fn at(start: u64) -> OplogRetention {
        OplogRetention {
            retained_start: idx(start),
        }
    }

    #[test]
    fn the_start_never_moves_back() {
        assert_eq!(at(5).advanced_to(idx(3)), at(5));
        assert_eq!(at(5).advanced_to(idx(9)), at(9));
        assert_eq!(OplogRetention::default().advanced_to(idx(1)), at(1));
    }

    #[test]
    fn the_retention_round_trips_through_its_encoding() {
        let encoded = golem_common::serialization::serialize(&at(7)).unwrap();
        let decoded: OplogRetention = golem_common::serialization::deserialize(&encoded).unwrap();
        assert_eq!(decoded, at(7));
    }

    #[test]
    fn the_retained_length_counts_from_the_start_to_the_last_index() {
        assert_eq!(at(1).retained_length(idx(10)), 10);
        assert_eq!(at(4).retained_length(idx(10)), 7);
        assert_eq!(at(10).retained_length(idx(10)), 1);
        assert_eq!(at(4).retained_length(OplogIndex::NONE), 0);
    }

    #[test]
    fn a_read_below_the_start_is_expired_except_the_create_entry_alone() {
        let expired = Err(OplogReadError::Expired {
            retained_start: idx(4),
        });
        assert_eq!(at(4).check_read(idx(4), 3), Ok(()));
        assert_eq!(at(4).check_read(idx(3), 3), expired);
        assert_eq!(at(4).check_read(idx(1), 1), Ok(()));
        assert_eq!(at(4).check_read(idx(1), 2), expired);
        assert_eq!(at(4).check_read(idx(2), 1), expired);
        assert_eq!(at(4).check_read(idx(2), 0), Ok(()));
        assert_eq!(at(1).check_read(idx(1), 10), Ok(()));
    }

    #[test]
    fn only_a_gap_below_the_start_is_expired() {
        let gap = |start, end| OplogReadError::Gap {
            start: idx(start),
            end: idx(end),
        };
        assert_eq!(
            at(4).classify(gap(2, 5)),
            OplogReadError::Expired {
                retained_start: idx(4)
            }
        );
        assert_eq!(at(4).classify(gap(4, 5)), gap(4, 5));
        assert_eq!(at(4).classify(gap(6, 8)), gap(6, 8));
        let corrupt =
            OplogReadError::corruption(crate::services::oplog::OplogReadSource::Primary, "bad");
        assert_eq!(at(4).classify(corrupt.clone()), corrupt);
    }
}
