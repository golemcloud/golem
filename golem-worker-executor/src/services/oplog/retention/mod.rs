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
//! and the start only moves forward. The `Create` entry at index 1 is the identity of the agent.
//! When the start is above 1, a read of the `Create` entry alone is the only read below the start
//! that is allowed. A read of a range or a page from index 1 is expired.
//!
//! The rules here are pure functions over the retained start, and every reader uses them. A read
//! that reaches below the start fails with [`OplogReadError::Expired`], which names the start. A
//! [`OplogReadError::Gap`] that ends below the start becomes the same error. A gap that ends at or
//! after the start stays a read error, because entries that the oplog keeps are missing.

mod layer;
#[cfg(test)]
pub(crate) mod test_oplog;

pub use layer::{RetentionOplog, RetentionOplogService};

use crate::services::oplog::{OplogReadError, OplogReadSource, UnreadableOplog};
use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use bytes::Bytes;
use desert_rust::BinaryCodec;
use futures::{StreamExt, TryStreamExt};
use golem_common::model::AgentId;
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::OplogIndex;
use golem_common::serialization::{deserialize, serialize};

/// Where the retained oplog of an agent starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct OplogRetention {
    retained_start: OplogIndex,
}

/// The entries of one oplog read: `count` entries from `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OplogRange {
    pub start: OplogIndex,
    pub count: u64,
}

impl OplogRange {
    /// The read of the `Create` entry alone.
    pub const CREATE: Self = Self {
        start: OplogIndex::INITIAL,
        count: 1,
    };

    /// The index after the last entry of the range.
    pub fn end(self) -> OplogIndex {
        OplogIndex::from_u64(self.start.as_u64() + self.count)
    }
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

    /// The first index that the oplog keeps.
    pub fn start(self) -> OplogIndex {
        self.retained_start
    }

    /// The retention with the start at `to`, or this retention when its start is already at or
    /// after `to`. The start never moves back.
    pub fn advanced_to(self, to: OplogIndex) -> Self {
        Self {
            retained_start: self.retained_start.max(to),
        }
    }

    /// `index`, or the retained start when `index` is below it.
    pub fn clamp(self, index: OplogIndex) -> OplogIndex {
        index.max(self.retained_start)
    }

    /// The retained entries from `from` to `last`, both included. A `from` below the start moves
    /// to the start. `None` when the range holds no retained entry.
    pub fn range(self, from: OplogIndex, last: OplogIndex) -> Option<OplogRange> {
        let start = self.clamp(from);
        (start <= last).then(|| OplogRange {
            start,
            count: last.as_u64() - start.as_u64() + 1,
        })
    }

    /// The next read of a walk that reads at most `max` entries at a time: the retained entries
    /// from `from` to `last`, both included, cut to `max` entries. A `from` below the start moves
    /// to the start. A `max` of 0 reads one entry, so that the walk always moves forward. `None`
    /// when the range holds no retained entry, which ends the walk.
    pub fn chunk(self, from: OplogIndex, last: OplogIndex, max: u64) -> Option<OplogRange> {
        self.range(from, last).map(|range| OplogRange {
            start: range.start,
            count: range.count.min(max.max(1)),
        })
    }

    /// The read of one page: at most `max` entries from `start` to `last`, both included. Unlike
    /// [`Self::chunk`], the page does not move its start: a start below the retained start is
    /// expired, except the read of the `Create` entry alone, as in [`Self::check_read`]. `None`
    /// when the page is empty.
    pub fn page(
        self,
        start: OplogIndex,
        last: OplogIndex,
        max: u64,
    ) -> Result<Option<OplogRange>, OplogReadError> {
        self.check_read(start, max)?;
        Ok((max > 0)
            .then(|| Self::FULL.chunk(start, last, max))
            .flatten())
    }

    /// Whether the retained oplog holds the entry at `index`, that is, whether the index is at or
    /// after the start. The `Create` entry below the start is readable alone, but the retained
    /// oplog does not hold it.
    pub fn retains(self, index: OplogIndex) -> bool {
        index >= self.retained_start
    }

    /// The number of retained entries of an oplog whose last index is `last`. The `Create` entry
    /// below the start does not count.
    pub fn retained_length(self, last: OplogIndex) -> u64 {
        self.range(OplogIndex::INITIAL, last)
            .map_or(0, |range| range.count)
    }

    /// The read of the `Create` entry alone, which a reader of the retained oplog makes apart from
    /// its range when the entry is below the start. `None` when the start is at index 1, because
    /// the retained oplog then holds the entry.
    pub fn create_read(self) -> Option<OplogRange> {
        (self.retained_start > OplogIndex::INITIAL).then_some(OplogRange::CREATE)
    }

    /// Whether the check of a read of `count` entries from `start` needs the retained start. An
    /// empty read and a read of the `Create` entry alone are always allowed, because the `Create`
    /// entry is the identity of the agent.
    pub(super) fn read_needs_start(start: OplogIndex, count: u64) -> bool {
        count != 0 && OplogRange { start, count } != OplogRange::CREATE
    }

    /// Checks a read of `count` entries from `start`. A read that reaches below the retained start
    /// fails with [`OplogReadError::Expired`], except the reads that
    /// [`Self::read_needs_start`] always allows.
    pub fn check_read(self, start: OplogIndex, count: u64) -> Result<(), OplogReadError> {
        if Self::read_needs_start(start, count) {
            self.check_from(start)
        } else {
            Ok(())
        }
    }

    /// Checks a read from `start` that has no fixed end, such as a page that a cursor continues,
    /// or a copy of the whole history. A start below the retained start fails with
    /// [`OplogReadError::Expired`], also at index 1.
    pub fn check_from(self, start: OplogIndex) -> Result<(), OplogReadError> {
        if self.retains(start) {
            Ok(())
        } else {
            Err(self.expired())
        }
    }

    /// [`Self::check_read`] for the oplog of `agent_id`. The error is an [`UnreadableOplog`].
    pub fn check_agent_read(
        self,
        agent_id: &AgentId,
        start: OplogIndex,
        count: u64,
    ) -> Result<(), UnreadableOplog> {
        self.check_read(start, count)
            .map_err(|error| UnreadableOplog::new(agent_id, error))
    }

    /// [`Self::check_from`] for the oplog of `agent_id`. The error is an [`UnreadableOplog`].
    pub fn check_agent_from(
        self,
        agent_id: &AgentId,
        start: OplogIndex,
    ) -> Result<(), UnreadableOplog> {
        self.check_from(start)
            .map_err(|error| UnreadableOplog::new(agent_id, error))
    }

    /// The error of a read, with a gap that ends below the retained start given as
    /// [`OplogReadError::Expired`]. A gap that ends at or after the start stays a gap, because
    /// entries that the oplog keeps are missing. Every other error stays as it is.
    pub fn classify(self, error: OplogReadError) -> OplogReadError {
        match error {
            OplogReadError::Gap { end, .. } if end < self.retained_start => self.expired(),
            other => other,
        }
    }

    /// [`Self::classify`] for a failed read of the oplog of `agent_id`.
    pub fn classify_unreadable(self, unreadable: UnreadableOplog) -> UnreadableOplog {
        UnreadableOplog {
            error: self.classify(unreadable.error),
            agent_id: unreadable.agent_id,
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

fn field(agent_mode: AgentMode) -> &'static str {
    match agent_mode {
        AgentMode::Durable => "Durable",
        AgentMode::Ephemeral => "Ephemeral",
    }
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
) -> Result<(OplogRetention, Option<Bytes>), UnreadableOplog> {
    let stored = storage
        .get(
            SVC_NAME,
            "read_retention",
            ENTITY_NAME,
            namespace(agent_id),
            field(agent_mode),
        )
        .await
        .map_err(|error| unreadable(agent_id, error))?;
    stored.map_or(Ok((OplogRetention::FULL, None)), |bytes| {
        deserialize(&bytes)
            .map(|retention| (retention, Some(bytes)))
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
/// record it read, so a writer with an old start cannot move the start back. When another writer
/// changes the record between the read and the write, the attempt starts again from a new read.
pub async fn advance_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    agent_id: &AgentId,
    agent_mode: AgentMode,
    to: OplogIndex,
) -> Result<OplogRetention, UnreadableOplog> {
    let attempts = futures::stream::repeat(())
        .then(|()| try_advance_oplog_retention(storage, agent_id, agent_mode, to))
        .try_filter_map(|written| futures::future::ready(Ok(written)));
    std::pin::pin!(attempts).try_next().await?.ok_or_else(|| {
        UnreadableOplog::new(
            agent_id,
            OplogReadError::source_failure(SOURCE, "the advance attempts ended"),
        )
    })
}

/// One attempt of [`advance_oplog_retention`]. `None` when another writer changed the record
/// after the read.
async fn try_advance_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    agent_id: &AgentId,
    agent_mode: AgentMode,
    to: OplogIndex,
) -> Result<Option<OplogRetention>, UnreadableOplog> {
    let (current, stored) = load(storage, agent_id, agent_mode).await?;
    let advanced = current.advanced_to(to);
    if advanced == current {
        return Ok(Some(current));
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
            field,
            stored.as_deref(),
            &[],
            &[(field, &encoded)],
        )
        .await
        .map_err(|error| unreadable(agent_id, error))?;
    Ok(written.then_some(advanced))
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
            field(agent_mode),
        )
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::test_oplog::{at, idx};
    use super::*;
    use crate::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use std::sync::Arc;
    use test_r::test;

    fn range(start: u64, count: u64) -> Option<OplogRange> {
        Some(OplogRange {
            start: idx(start),
            count,
        })
    }

    fn expired(start: u64) -> Result<(), OplogReadError> {
        Err(OplogReadError::Expired {
            retained_start: idx(start),
        })
    }

    #[test]
    fn the_start_never_moves_back() {
        assert_eq!(at(5).advanced_to(idx(4)).start(), idx(5));
        assert_eq!(at(5).advanced_to(idx(5)).start(), idx(5));
        assert_eq!(at(5).advanced_to(idx(6)).start(), idx(6));
        assert_eq!(OplogRetention::default(), OplogRetention::FULL);
        assert_eq!(OplogRetention::FULL.start(), OplogIndex::INITIAL);
    }

    #[test]
    fn the_retention_round_trips_through_its_encoding() {
        let encoded = serialize(&at(7)).unwrap();
        let decoded: OplogRetention = deserialize(&encoded).unwrap();
        assert_eq!(decoded, at(7));
    }

    #[test]
    fn the_clamp_moves_only_an_index_below_the_start() {
        assert_eq!(at(4).clamp(idx(3)), idx(4));
        assert_eq!(at(4).clamp(idx(4)), idx(4));
        assert_eq!(at(4).clamp(idx(5)), idx(5));
        assert_eq!(at(4).clamp(OplogIndex::NONE), idx(4));
    }

    #[test]
    fn only_an_index_at_or_after_the_start_is_retained() {
        assert!(!at(4).retains(idx(3)));
        assert!(at(4).retains(idx(4)));
        assert!(at(4).retains(idx(5)));
        assert!(
            !at(4).retains(OplogIndex::INITIAL),
            "the `Create` entry below the start"
        );
        assert!(at(1).retains(OplogIndex::INITIAL));
        assert!(!at(1).retains(OplogIndex::NONE));
    }

    #[test]
    fn the_range_holds_the_retained_entries_and_is_none_when_empty() {
        assert_eq!(at(4).range(idx(1), idx(10)), range(4, 7));
        assert_eq!(at(4).range(idx(3), idx(10)), range(4, 7));
        assert_eq!(at(4).range(idx(4), idx(10)), range(4, 7));
        assert_eq!(at(4).range(idx(5), idx(10)), range(5, 6));
        assert_eq!(at(4).range(idx(10), idx(10)), range(10, 1));
        assert_eq!(at(4).range(idx(11), idx(10)), None);
        assert_eq!(at(4).range(idx(1), idx(4)), range(4, 1));
        assert_eq!(at(4).range(idx(1), idx(3)), None);
        assert_eq!(at(1).range(OplogIndex::NONE, idx(2)), range(1, 2));
        assert_eq!(at(1).range(idx(1), OplogIndex::NONE), None);
    }

    #[test]
    fn a_range_ends_before_the_index_after_its_last_entry() {
        assert_eq!(range(4, 7).unwrap().end(), idx(11));
        assert_eq!(range(4, 1).unwrap().end(), idx(5));
        assert_eq!(OplogRange::CREATE.end(), idx(2));
    }

    #[test]
    fn a_chunk_is_the_retained_range_cut_to_the_maximum() {
        assert_eq!(at(4).chunk(idx(1), idx(10), 3), range(4, 3));
        assert_eq!(at(4).chunk(idx(3), idx(10), 3), range(4, 3));
        assert_eq!(at(4).chunk(idx(4), idx(10), 3), range(4, 3));
        assert_eq!(at(4).chunk(idx(5), idx(10), 3), range(5, 3));
        assert_eq!(at(4).chunk(idx(8), idx(10), 3), range(8, 3));
        assert_eq!(at(4).chunk(idx(9), idx(10), 3), range(9, 2));
        assert_eq!(at(4).chunk(idx(10), idx(10), 3), range(10, 1));
        assert_eq!(at(4).chunk(idx(11), idx(10), 3), None);
        assert_eq!(at(4).chunk(idx(1), idx(3), 3), None);
        assert_eq!(at(4).chunk(idx(4), idx(10), 7), range(4, 7));
        assert_eq!(at(4).chunk(idx(4), idx(10), 8), range(4, 7));
        assert_eq!(at(4).chunk(idx(4), idx(10), 0), range(4, 1));
        assert_eq!(at(1).chunk(OplogIndex::NONE, idx(2), 1024), range(1, 2));
    }

    #[test]
    fn a_walk_of_chunks_reads_every_retained_entry_once() {
        let walk = |retention: OplogRetention, last: u64, max: u64| {
            std::iter::successors(retention.chunk(OplogIndex::NONE, idx(last), max), |chunk| {
                retention.chunk(chunk.end(), idx(last), max)
            })
            .collect::<Vec<_>>()
        };
        assert_eq!(
            walk(at(4), 10, 3),
            vec![range(4, 3), range(7, 3), range(10, 1)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            walk(at(4), 9, 3),
            vec![range(4, 3), range(7, 3)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
        );
        assert_eq!(walk(at(4), 3, 3), Vec::new());
    }

    #[test]
    fn a_page_keeps_its_start_and_is_expired_below_the_retained_start() {
        assert_eq!(at(4).page(idx(4), idx(10), 3), Ok(range(4, 3)));
        assert_eq!(at(4).page(idx(5), idx(10), 3), Ok(range(5, 3)));
        assert_eq!(at(4).page(idx(9), idx(10), 3), Ok(range(9, 2)));
        assert_eq!(at(4).page(idx(10), idx(10), 3), Ok(range(10, 1)));
        assert_eq!(at(4).page(idx(11), idx(10), 3), Ok(None));
        assert_eq!(at(4).page(idx(4), idx(10), 0), Ok(None));
        assert_eq!(at(4).page(idx(3), idx(10), 3), expired(4).map(|()| None));
        assert_eq!(at(4).page(idx(2), idx(10), 1), expired(4).map(|()| None));
        assert_eq!(
            at(4).page(idx(1), idx(10), 1),
            Ok(range(1, 1)),
            "the `Create` entry alone"
        );
        assert_eq!(at(4).page(idx(1), idx(10), 2), expired(4).map(|()| None));
        assert_eq!(at(1).page(idx(1), idx(10), 20), Ok(range(1, 10)));
    }

    #[test]
    fn the_retained_length_counts_from_the_start_to_the_last_index() {
        assert_eq!(at(1).retained_length(idx(10)), 10);
        assert_eq!(at(4).retained_length(idx(10)), 7);
        assert_eq!(at(10).retained_length(idx(10)), 1);
        assert_eq!(at(11).retained_length(idx(10)), 0);
        assert_eq!(at(4).retained_length(OplogIndex::NONE), 0);
    }

    #[test]
    fn the_create_entry_is_read_alone_only_when_it_is_below_the_start() {
        assert_eq!(at(1).create_read(), None);
        assert_eq!(at(2).create_read(), Some(OplogRange::CREATE));
        assert_eq!(OplogRange::CREATE, range(1, 1).unwrap());
    }

    #[test]
    fn a_read_below_the_start_is_expired_except_an_empty_read_and_the_create_entry_alone() {
        assert_eq!(at(4).check_read(idx(4), 3), Ok(()));
        assert_eq!(at(4).check_read(idx(5), 3), Ok(()));
        assert_eq!(at(4).check_read(idx(3), 3), expired(4));
        assert_eq!(at(4).check_read(idx(3), 1), expired(4));
        assert_eq!(at(4).check_read(idx(1), 1), Ok(()));
        assert_eq!(at(4).check_read(idx(1), 2), expired(4));
        assert_eq!(at(4).check_read(idx(2), 1), expired(4));
        assert_eq!(at(4).check_read(idx(2), 0), Ok(()));
        assert_eq!(at(4).check_read(OplogIndex::NONE, 0), Ok(()));
        assert_eq!(at(4).check_read(OplogIndex::NONE, 1), expired(4));
        assert_eq!(at(1).check_read(idx(1), 10), Ok(()));
        assert_eq!(at(1).check_read(OplogIndex::NONE, 1), expired(1));
    }

    #[test]
    fn a_read_without_an_end_below_the_start_is_expired_also_at_index_1() {
        assert_eq!(at(4).check_from(idx(3)), expired(4));
        assert_eq!(at(4).check_from(idx(4)), Ok(()));
        assert_eq!(at(4).check_from(idx(5)), Ok(()));
        assert_eq!(at(4).check_from(idx(1)), expired(4));
        assert_eq!(at(1).check_from(idx(1)), Ok(()));
    }

    #[test]
    fn the_agent_checks_name_the_agent() {
        let agent_id = AgentId {
            component_id: golem_common::model::component::ComponentId::new(),
            agent_id: "checked".to_string(),
        };
        let unreadable = UnreadableOplog::new(&agent_id, expired(4).unwrap_err());
        assert_eq!(
            at(4).check_agent_read(&agent_id, idx(3), 1),
            Err(unreadable.clone())
        );
        assert_eq!(at(4).check_agent_read(&agent_id, idx(4), 1), Ok(()));
        assert_eq!(
            at(4).check_agent_from(&agent_id, idx(3)),
            Err(unreadable.clone())
        );
        assert_eq!(at(4).check_agent_from(&agent_id, idx(4)), Ok(()));
        let gap = OplogReadError::Gap {
            start: idx(1),
            end: idx(3),
        };
        assert_eq!(
            at(4).classify_unreadable(UnreadableOplog::new(&agent_id, gap.clone())),
            unreadable
        );
        assert_eq!(
            at(3).classify_unreadable(UnreadableOplog::new(&agent_id, gap.clone())),
            UnreadableOplog::new(&agent_id, gap)
        );
    }

    #[test]
    fn only_a_gap_that_ends_below_the_start_is_expired() {
        let gap = |start, end| OplogReadError::Gap {
            start: idx(start),
            end: idx(end),
        };
        let expired = OplogReadError::Expired {
            retained_start: idx(4),
        };
        assert_eq!(at(4).classify(gap(1, 3)), expired);
        assert_eq!(at(4).classify(gap(1, 1)), expired);
        assert_eq!(at(4).classify(gap(2, 4)), gap(2, 4));
        assert_eq!(at(4).classify(gap(2, 5)), gap(2, 5));
        assert_eq!(at(4).classify(gap(4, 5)), gap(4, 5));
        assert_eq!(at(4).classify(gap(6, 8)), gap(6, 8));
        let corrupt = OplogReadError::corruption(OplogReadSource::Primary, "bad");
        assert_eq!(at(4).classify(corrupt.clone()), corrupt);
    }

    /// A writer that read the record before another writer moved the start to 5 loses its
    /// compare-and-set, reads the record again, and keeps the start at 5 without a second write.
    #[test]
    async fn a_stale_writer_does_not_move_the_start_back() {
        let faults = KeyValueStorageFaults::default();
        let inner: Arc<dyn KeyValueStorage + Send + Sync> =
            Arc::new(InMemoryKeyValueStorage::new());
        let storage = FaultInjectingKeyValueStorage::new(inner, faults.clone());
        let agent_id = AgentId {
            component_id: golem_common::model::component::ComponentId::new(),
            agent_id: "stale-writer".to_string(),
        };
        let gate = faults.pause_next("advance_retention");
        let (stale, other) = futures::join!(
            advance_oplog_retention(&storage, &agent_id, AgentMode::Durable, idx(3)),
            async {
                gate.entered().await;
                let other =
                    advance_oplog_retention(&storage, &agent_id, AgentMode::Durable, idx(5)).await;
                gate.release();
                other
            }
        );
        assert_eq!(other.unwrap(), at(5));
        assert_eq!(stale.unwrap(), at(5));
        assert_eq!(
            faults.calls("advance_retention"),
            2,
            "the stale writer lost its compare-and-set and wrote nothing after it read 5"
        );
        assert_eq!(
            read_oplog_retention(&storage, &agent_id, AgentMode::Durable)
                .await
                .unwrap(),
            at(5)
        );
        assert_eq!(
            read_oplog_retention(&storage, &agent_id, AgentMode::Ephemeral)
                .await
                .unwrap(),
            OplogRetention::FULL,
            "each agent mode has its own record"
        );
    }
}
