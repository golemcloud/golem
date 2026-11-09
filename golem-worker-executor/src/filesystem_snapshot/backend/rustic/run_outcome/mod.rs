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

//! Answers, failures and late-write settlement shared by store operations and the call driver.

use crate::filesystem_snapshot::SnapshotInfo;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// How a run ended without an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RunEnd {
    /// A storage call failed after its tries.
    CallFailed,
    /// A failure that no new run can change.
    Permanent,
    /// Another call invalidated this run, so a new run can try again.
    RaceRunAgain,
    /// The save's backup exceeded its bound.
    BoundPassed,
    /// The save was cancelled or the store shut down.
    Cancelled,
}

/// What the check of the own name of a save found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Checked {
    Found(SnapshotInfo),
    Other,
    Absent,
    Unreadable,
}

/// Every run either gives an answer or settlement, or ends without an answer.
pub(super) type RunResult<T> = Result<Ran<T>, Ended>;

/// A run's chosen answer, or its pending late-write settlement.
pub(super) enum Ran<T> {
    Answered(T),
    AnsweredAfter(T, LateWrite),
    /// The caller returns the slot before waiting, then uses `finish` to obtain the run result.
    Settling(Settle<T>),
}

/// Settlement after a run returned its slot.
pub(super) struct Settle<T> {
    pub(super) wait: futures::future::BoxFuture<'static, ()>,
    pub(super) finish: Box<dyn FnOnce(Settled) -> RunResult<T> + Send>,
    pub(super) may_land: bool,
}

/// How settlement ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Settled {
    Waited,
    ShutDown,
}

/// Gives the answer after its writes have landed or can no longer land at `until`.
pub(super) fn answered_after<T>(answer: T, until: Option<Instant>) -> RunResult<T> {
    Ok(match until {
        Some(until) => Ran::AnsweredAfter(answer, LateWrite { until, own: None }),
        None => Ran::Answered(answer),
    })
}

/// A run that ended without an answer.
#[derive(Debug)]
pub(super) struct Ended {
    pub(super) end: RunEnd,
    pub(super) failure: anyhow::Error,
    pub(super) late: Option<LateWrite>,
}

impl Ended {
    pub(super) fn new(end: RunEnd, failure: anyhow::Error) -> Self {
        Self {
            end,
            failure,
            late: None,
        }
    }
}

/// Writes that ended without an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LateWrite {
    /// After this instant, each write has landed or never lands.
    pub(super) until: Instant,
    /// The staged snapshot file that the save's own-name check looks for.
    pub(super) own: Option<OwnFile>,
}

/// A staged snapshot file and the info of its snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnFile {
    pub(super) path: Arc<Path>,
    pub(super) info: SnapshotInfo,
}
