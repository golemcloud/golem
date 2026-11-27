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
//! Oplog indexes never renumber. The retained start comes from a record in the
//! `AgentOplogRetention` key-value namespace, and [`advance_oplog_retention`] only moves it
//! forward. The oplog gives no entry before the start. The `Create` entry at index 1 is the
//! identity of the agent.
//! When the start is above 1, an empty read and a read of the `Create` entry alone are the only
//! reads below the start that are allowed. A read of more than the `Create` entry from index 1 is
//! expired.
//!
//! The rules here are pure functions over the retained start. A walk in chunks reads with
//! [`OplogRetention::chunk`], and a public page reads with [`OplogRetention::page`]. A walk with
//! a fixed start, such as a fold from a status baseline, a replay cursor walk or the two
//! whole-history walks below, counts its reads itself and does not move its start, so the layer
//! refuses its read below the start. The lookup of the component revision of a public read
//! (`scan_component_revision` in `model/public_oplog/reading.rs`) charges each batch to its
//! allowance.
//! A read that reaches below the start fails with [`OplogReadError::Expired`], which names the
//! start. A [`OplogReadError::Gap`] that ends below the start becomes the same error. A gap that
//! ends at or after the start stays a read error, because entries that the oplog keeps are
//! missing.
//!
//! Some walks need the whole history, because a walk from the start gives a wrong result: the
//! full status rebuild, the revert preflight, the epoch fence of a stream fork revert
//! (`max_issued_epoch`) and the creation receipt of a fork export. The full status rebuild, the
//! revert preflight and `max_issued_epoch` with no earlier fork cut read more than the `Create`
//! entry from index 1, so they fail as expired at any start above 1. Only the creation receipt
//! reads the `Create` entry alone and then from index 2, so it reads with no error at a start of
//! 2.

mod layer;
#[cfg(test)]
pub(crate) mod test_oplog;

pub use layer::{RetentionOplog, RetentionOplogService};

use crate::services::oplog::{OplogReadError, OplogReadSource, UnreadableOplog};
use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use bytes::Bytes;
use desert_rust::BinaryCodec;
use futures::{StreamExt, TryStreamExt};
use golem_common::model::agent::AgentMode;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentId, OwnedAgentId};
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

    /// The read of a walk with a fixed start: the entries from `from` to `last`, both included,
    /// cut to `max` entries. A `max` of 0 reads one entry, so that the walk always moves forward.
    /// `None` when `from` is after `last`, which ends the walk. Unlike
    /// [`OplogRetention::chunk`], the range keeps `from`: a walk that must not skip an entry reads
    /// from where it is, and the oplog refuses a read below the retained start. The count is exact
    /// unless `from` is 0 and `last` is `u64::MAX`.
    pub fn within(from: OplogIndex, last: OplogIndex, max: u64) -> Option<Self> {
        (from <= last).then(|| Self {
            start: from,
            count: (last.as_u64() - from.as_u64())
                .saturating_add(1)
                .min(max.max(1)),
        })
    }

    /// The index after the last entry of the range. The last entry must be below `u64::MAX`,
    /// because the index after it does not exist otherwise.
    pub fn end(self) -> OplogIndex {
        OplogIndex::from_u64(self.start.as_u64() + self.count)
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
    fn advanced_to(self, to: OplogIndex) -> Self {
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
        OplogRange::within(self.clamp(from), last, u64::MAX)
    }

    /// The next read of a walk that reads at most `max` entries at a time: the retained entries
    /// from `from` to `last`, both included, cut to `max` entries. A `from` below the start moves
    /// to the start. A `max` of 0 reads one entry, so that the walk always moves forward. `None`
    /// when the range holds no retained entry, which ends the walk.
    pub fn chunk(self, from: OplogIndex, last: OplogIndex, max: u64) -> Option<OplogRange> {
        OplogRange::within(self.clamp(from), last, max)
    }

    /// The read of one page of the oplog of `agent_id`: at most `max` entries from `start` to
    /// `last`, both included. Unlike [`Self::chunk`], the page does not move its start. A page
    /// has no fixed end, because a cursor can continue it, so a start below the retained start is
    /// expired, as in [`Self::check_agent_from`], also at index 1. `None` when the page is empty;
    /// a page of 0 entries reads nothing and is never expired.
    pub fn page(
        self,
        agent_id: &AgentId,
        start: OplogIndex,
        last: OplogIndex,
        max: u64,
    ) -> Result<Option<OplogRange>, UnreadableOplog> {
        if max == 0 {
            return Ok(None);
        }
        self.check_agent_from(agent_id, start)?;
        Ok(OplogRange::within(start, last, max))
    }

    /// Whether the retained oplog holds the entry at `index`, that is, whether the index is at or
    /// after the start. The `Create` entry below the start is readable alone, but the retained
    /// oplog does not hold it.
    pub fn retains(self, index: OplogIndex) -> bool {
        index >= self.retained_start
    }

    /// The number of retained entries of an oplog whose last index is `last`. The `Create` entry
    /// below the start does not count.
    fn retained_length(self, last: OplogIndex) -> u64 {
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

    /// Checks a read of `count` entries from `start` in the oplog of `agent_id`. A read that
    /// reaches below the retained start fails with [`OplogReadError::Expired`] in an
    /// [`UnreadableOplog`], except the reads that [`Self::read_needs_start`] always allows.
    pub fn check_agent_read(
        self,
        agent_id: &AgentId,
        start: OplogIndex,
        count: u64,
    ) -> Result<(), UnreadableOplog> {
        if Self::read_needs_start(start, count) {
            self.check_agent_from(agent_id, start)
        } else {
            Ok(())
        }
    }

    /// Checks a read from `start` in the oplog of `agent_id` that has no fixed end, such as a page
    /// that a cursor continues, or a copy of the whole history. A start below the retained start
    /// fails with [`OplogReadError::Expired`] in an [`UnreadableOplog`], also at index 1.
    pub fn check_agent_from(
        self,
        agent_id: &AgentId,
        start: OplogIndex,
    ) -> Result<(), UnreadableOplog> {
        if self.retains(start) {
            Ok(())
        } else {
            Err(UnreadableOplog::new(agent_id, self.expired()))
        }
    }

    /// The error of a failed read of the oplog of an agent, with a gap that ends below the
    /// retained start given as [`OplogReadError::Expired`]. A gap that ends at or after the start
    /// stays a gap, because entries that the oplog keeps are missing. Every other error stays as
    /// it is.
    fn classify_unreadable(self, unreadable: UnreadableOplog) -> UnreadableOplog {
        let error = match unreadable.error {
            OplogReadError::Gap { end, .. } if end < self.retained_start => self.expired(),
            other => other,
        };
        UnreadableOplog {
            error,
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

fn namespace(owned_agent_id: &OwnedAgentId) -> KeyValueStorageNamespace {
    KeyValueStorageNamespace::AgentOplogRetention {
        owned_agent_id: owned_agent_id.clone(),
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

/// The stored retention and its encoded form. An absent record is [`OplogRetention::FULL`]. A
/// record with a start of 0 is corrupt: the first index of an oplog is 1, and the rules compute
/// the index before the start and after a range.
async fn load(
    storage: &(dyn KeyValueStorage + Send + Sync),
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
) -> Result<(OplogRetention, Option<Bytes>), UnreadableOplog> {
    let stored = storage
        .get(
            SVC_NAME,
            "read_retention",
            ENTITY_NAME,
            namespace(owned_agent_id),
            field(agent_mode),
        )
        .await
        .map_err(|error| unreadable(&owned_agent_id.agent_id, error))?;
    stored.map_or(Ok((OplogRetention::FULL, None)), |bytes| {
        let retention: OplogRetention = deserialize(&bytes).map_err(|error| {
            UnreadableOplog::new(
                &owned_agent_id.agent_id,
                OplogReadError::corruption(SOURCE, format!("undecodable record: {error}")),
            )
        })?;
        if retention.retained_start < OplogIndex::INITIAL {
            return Err(UnreadableOplog::new(
                &owned_agent_id.agent_id,
                OplogReadError::corruption(SOURCE, "the record has a retained start of 0"),
            ));
        }
        Ok((retention, Some(bytes)))
    })
}

/// The retention of the oplog of the agent in `agent_mode`.
pub async fn read_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
) -> Result<OplogRetention, UnreadableOplog> {
    Ok(load(storage, owned_agent_id, agent_mode).await?.0)
}

/// Moves the retained start of the oplog of the agent in `agent_mode` forward to `to`, and returns
/// the stored retention. A start that is already at or after `to` stays. The write compares the
/// record it read, so a writer with an old start cannot move the start back. When another writer
/// changes the record between the read and the write, the attempt starts again from a new read.
pub async fn advance_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    to: OplogIndex,
) -> Result<OplogRetention, UnreadableOplog> {
    // The stream of attempts never ends: it gives a value only when an attempt wrote the record or
    // found the start already at or after `to`, and an error ends the advance.
    let attempts = futures::stream::repeat(())
        .then(|()| try_advance_oplog_retention(storage, owned_agent_id, agent_mode, to))
        .try_filter_map(|written| futures::future::ready(Ok(written)));
    std::pin::pin!(attempts).try_next().await?.ok_or_else(|| {
        UnreadableOplog::new(
            &owned_agent_id.agent_id,
            OplogReadError::source_failure(SOURCE, "the advance attempts ended"),
        )
    })
}

/// One attempt of [`advance_oplog_retention`]. `None` when another writer changed the record
/// after the read.
async fn try_advance_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
    to: OplogIndex,
) -> Result<Option<OplogRetention>, UnreadableOplog> {
    let (current, stored) = load(storage, owned_agent_id, agent_mode).await?;
    let advanced = current.advanced_to(to);
    if advanced == current {
        return Ok(Some(current));
    }
    let encoded = serialize(&advanced).map_err(|error| {
        UnreadableOplog::new(
            &owned_agent_id.agent_id,
            OplogReadError::corruption(SOURCE, error),
        )
    })?;
    let field = field(agent_mode);
    let written = storage
        .compare_and_set_many(
            SVC_NAME,
            "advance_retention",
            ENTITY_NAME,
            namespace(owned_agent_id),
            field,
            stored.as_deref(),
            &[],
            &[(field, &encoded)],
        )
        .await
        .map_err(|error| unreadable(&owned_agent_id.agent_id, error))?;
    Ok(written.then_some(advanced))
}

/// Removes the retention record of the oplog of the agent in `agent_mode`.
pub(crate) async fn remove_oplog_retention(
    storage: &(dyn KeyValueStorage + Send + Sync),
    owned_agent_id: &OwnedAgentId,
    agent_mode: AgentMode,
) -> Result<(), String> {
    storage
        .del(
            SVC_NAME,
            "remove_retention",
            namespace(owned_agent_id),
            field(agent_mode),
        )
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
