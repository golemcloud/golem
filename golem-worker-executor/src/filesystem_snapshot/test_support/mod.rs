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

//! Independent tree fixtures and shared values used by store tests.

pub(crate) mod clock;
pub(crate) mod fixture;

use super::{
    AgentSnapshots, CallError, FilesystemSnapshotStore, ReadError, RestoreFailure, RunSlots,
    SaveError, Slot, SnapshotInfo, SnapshotName, Withdrawal,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId, Timestamp};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Opens a store over the storage of one case. Each call opens another store over the same
/// storage, as another executor has.
pub(crate) type OpenStore = Arc<dyn Fn() -> Arc<dyn FilesystemSnapshotStore> + Send + Sync>;

pub(super) fn new_scope() -> AgentSnapshots {
    AgentSnapshots::agent(
        &OwnedAgentId::new(
            EnvironmentId(Uuid::new_v4()),
            &AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "counter(\"contract\")".to_string(),
            },
        ),
        golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
    )
}

/// A limiter that counts its takes and the slots that live, and keeps the most slots that lived at
/// once. With a withdrawal, each take gives it.
#[derive(Default)]
pub(crate) struct CountingSlots {
    takes: AtomicU64,
    live: Arc<AtomicU64>,
    most: Arc<AtomicU64>,
    withdraw: Option<Withdrawal>,
}

impl CountingSlots {
    /// A limiter that withdraws each take with `cause`.
    pub(super) fn withdrawing(cause: Withdrawal) -> Self {
        Self {
            withdraw: Some(cause),
            ..Self::default()
        }
    }

    /// The takes, the slots that live now, and the most slots that lived at once.
    pub(super) fn counts(&self) -> (u64, u64, u64) {
        (
            self.takes.load(Ordering::SeqCst),
            self.live.load(Ordering::SeqCst),
            self.most.load(Ordering::SeqCst),
        )
    }
}

/// A slot of a [`CountingSlots`]: it counts as live until it drops.
struct CountedSlot(Arc<AtomicU64>);

impl Drop for CountedSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl RunSlots for CountingSlots {
    fn take(&self, _immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        self.takes.fetch_add(1, Ordering::SeqCst);
        let taken = match self.withdraw {
            Some(cause) => Err(cause),
            None => {
                let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(live, Ordering::SeqCst);
                Ok(Slot::new(CountedSlot(Arc::clone(&self.live))))
            }
        };
        std::future::ready(taken).boxed()
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        match self.withdraw {
            Some(cause) => std::future::ready(cause).boxed(),
            None => std::future::pending().boxed(),
        }
    }
}

/// The limiter of a call of a case: a counting limiter that withdraws nothing.
pub(crate) fn slots() -> CountingSlots {
    CountingSlots::default()
}

/// The failure of an error of a call, for the tests that look at it.
#[cfg(test)]
pub(crate) trait FailureOf {
    /// Gives the cause of a `Failed` error, and `None` for each other error.
    fn failure(&self) -> Option<&anyhow::Error>;

    /// Tells whether the error is `Stopped`.
    fn stopped(&self) -> bool;
}

#[cfg(test)]
impl FailureOf for CallError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::Stopped(_) => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }
}

#[cfg(test)]
impl FailureOf for SaveError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::NameInUse | Self::Source(_) | Self::Stopped(_) => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }
}

#[cfg(test)]
impl FailureOf for RestoreFailure {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::NotFound | Self::Corrupt(_) | Self::Destination(_) | Self::Stopped(_) => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }
}

#[cfg(test)]
impl FailureOf for ReadError {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            Self::Failed(failed) => Some(failed.cause()),
            Self::Corrupt(_) | Self::Stopped => None,
        }
    }

    fn stopped(&self) -> bool {
        matches!(self, Self::Stopped)
    }
}

/// A cancel of a save that never fires.
#[cfg(test)]
pub(crate) fn never_cancelled() -> &'static CancellationToken {
    static NEVER: std::sync::LazyLock<CancellationToken> =
        std::sync::LazyLock::new(CancellationToken::new);
    &NEVER
}

/// The times that a test store gives its saves: each new name gets a time ten minutes after the
/// name before it, so that retention sees times far apart.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct SpacedTimes(Mutex<HashMap<Box<str>, Timestamp>>);

#[cfg(test)]
impl SpacedTimes {
    /// The time of the first save.
    pub(crate) const FIRST_MILLIS: u64 = 1_800_000_000_000;
    /// The time between two saves.
    const SPACING_MILLIS: u64 = 10 * 60 * 1000;

    /// Gives `info` with the time of `name`, and gives a new name the next time.
    pub(crate) fn timed(&self, name: &SnapshotName, info: SnapshotInfo) -> SnapshotInfo {
        let mut times = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = times.len() as u64;
        let created_at = *times
            .entry(Box::from(name.as_str()))
            .or_insert_with(|| Timestamp::from(Self::FIRST_MILLIS + count * Self::SPACING_MILLIS));
        SnapshotInfo { created_at, ..info }
    }

    /// Gives `info` with the time of `name` when it has one, and gives no time to a new name.
    pub(crate) fn known(&self, name: &SnapshotName, info: SnapshotInfo) -> SnapshotInfo {
        let times = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        SnapshotInfo {
            created_at: times.get(name.as_str()).copied().unwrap_or(info.created_at),
            ..info
        }
    }
}
