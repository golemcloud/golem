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

//! The decisions of the service that do not read or change its state, as pure functions over
//! plain values: whether an agent keeps files, what a job does after its confirmation, the check
//! of the store before a start, what a manual update does after a refused admission, the binding
//! of the configuration, the room of a volume, and the outcome of the publication of a fork.
//! Nothing here waits, reads a clock, draws a random number or calls the store.

use super::transitions::{JobId, Refusal, RunningJob};
use super::{ConfirmOutcome, JobDecision, SnapshotKind, SnapshotSkip};
use crate::sandbox_filesystem::FilesystemSpace;
use crate::services::golem_config::{
    FilesystemPressureConfig, FilesystemSnapshotStoreConfig, FilesystemSnapshotsConfig,
};
use golem_common::model::AgentFingerprint;
use golem_common::model::agent::AgentMode;
use std::time::Duration;

/// What a fork attempt found about the live target, after its publication or a reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ForkFound {
    /// The publication made the stage of this attempt the target.
    Published,
    /// A read of the `Create` of the live target gave this instance id.
    Live(AgentFingerprint),
    /// The publication was refused, and a read of the live target gave this instance id, or no
    /// live target.
    RefusedThenLive(Option<AgentFingerprint>),
    /// The outcome of the publication is not known.
    Unknown,
}

/// How the publication of a fork attempt ended for the snapshots of its stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ForkOutcome {
    /// The stage of the attempt is the live target.
    Published,
    /// Another stage is the live target, so nothing can publish the stage of the attempt.
    Lost,
    /// The live target is not known.
    Unknown,
}

/// Decides how a fork attempt whose stage has the instance id `stage` ended, from what it
/// `found`. Only a live target with another instance id makes the stage lost: a refused
/// publication alone does not, and a live target with the own id is the own publication.
pub(super) fn fork_outcome(found: ForkFound, stage: AgentFingerprint) -> ForkOutcome {
    match found {
        ForkFound::Published => ForkOutcome::Published,
        ForkFound::Live(live) | ForkFound::RefusedThenLive(Some(live)) if live == stage => {
            ForkOutcome::Published
        }
        ForkFound::Live(_) | ForkFound::RefusedThenLive(Some(_)) => ForkOutcome::Lost,
        ForkFound::RefusedThenLive(None) | ForkFound::Unknown => ForkOutcome::Unknown,
    }
}

/// Whether an agent in `mode` can have filesystem snapshots. Only a durable agent can: the snapshot
/// policy of an ephemeral agent resolves to disabled, so it takes no snapshot and has none in the
/// store. A delete of all snapshots of an ephemeral agent therefore makes no store call.
pub(super) fn keeps_files(mode: AgentMode) -> bool {
    match mode {
        AgentMode::Durable => true,
        AgentMode::Ephemeral => false,
    }
}

/// What a job does after its confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FollowUp {
    /// Applies retention.
    DeleteOlder,
    /// Deletes the own snapshot, which no confirmation record names.
    DeleteSuperseded,
    /// Keeps the snapshot and does nothing more.
    Keep,
}

/// The follow-up of a job of `kind` whose confirmation gave `outcome`. Only a confirmed periodic
/// snapshot applies retention, and only a superseded one is deleted.
pub(super) fn follow_up(kind: SnapshotKind, outcome: ConfirmOutcome) -> FollowUp {
    match (kind, outcome) {
        (SnapshotKind::Periodic, ConfirmOutcome::Confirmed) => FollowUp::DeleteOlder,
        (_, ConfirmOutcome::Superseded) => FollowUp::DeleteSuperseded,
        (SnapshotKind::Update, ConfirmOutcome::Confirmed) | (_, ConfirmOutcome::Deferred) => {
            FollowUp::Keep
        }
    }
}

/// What a start does after its wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StoreCheck {
    /// It asks the store nothing.
    Skip,
    /// It asks the store once, for at most this time.
    Stat(Duration),
}

/// The limits of the store check of a start.
#[derive(Clone, Copy, Debug)]
pub(super) struct StoreCheckLimits {
    /// The wait for an upload and the check after it take at most this time together.
    pub(super) confirmation_wait: Duration,
    /// The check of a start that did not wait takes at most this time.
    pub(super) store_check_limit: Duration,
}

/// The store check of a start. It skips the check when the local job gave `Superseded` or a
/// terminal interrupt waits. A start that waited for `waited` checks for what is left of the
/// confirmation wait, and a start that did not wait for the store check limit.
pub(super) fn store_check(
    decision: Option<JobDecision>,
    terminal: bool,
    waited: Option<Duration>,
    limits: StoreCheckLimits,
) -> StoreCheck {
    if terminal || decision == Some(JobDecision::Confirmed(ConfirmOutcome::Superseded)) {
        return StoreCheck::Skip;
    }
    StoreCheck::Stat(waited.map_or(limits.store_check_limit, |waited| {
        limits.confirmation_wait.saturating_sub(waited)
    }))
}

/// What a manual update does after a refused admission.
#[derive(Clone, Debug)]
pub(super) enum UpdateAdmit {
    /// It fails with the refusal.
    Refuse,
    /// It waits until the job is gone or an admission can replace it, then asks again. With
    /// `stop_deletes`, it first stops the deletes of the running job.
    WaitForEndOrReplacement {
        running: RunningJob,
        stop_deletes: bool,
    },
}

/// What a manual update does after an admission gave `refusal`. Only a job that runs makes it
/// wait: a periodic job that an admission replaces once its store call waits after a failed run or
/// waits for its late writes, or a job that ends after its deletes, such as the job of an earlier
/// manual update in its retention. The update refuses when its deadline passed or the service
/// shuts down. It stops the deletes of a running job once: not again for the job `stopped` whose
/// deletes an earlier ask stopped.
pub(super) fn update_admission(
    refusal: Refusal,
    deadline_passed: bool,
    shut_down: bool,
    stopped: Option<JobId>,
) -> UpdateAdmit {
    match (refusal.skip, refusal.running) {
        (SnapshotSkip::UploadInFlight, Some(running)) if !deadline_passed && !shut_down => {
            UpdateAdmit::WaitForEndOrReplacement {
                stop_deletes: stopped != Some(running.id),
                running,
            }
        }
        _ => UpdateAdmit::Refuse,
    }
}

/// What a service binds to.
#[derive(Debug)]
pub(super) enum Binding<'a> {
    /// The service keeps no filesystem snapshots.
    Disabled,
    /// The service keeps filesystem snapshots with the store and settings of `config`.
    Enabled(&'a FilesystemSnapshotStoreConfig),
}

/// What `config` binds to. `Enabled` needs storage whose volume makes copy-on-write copies, and
/// `copy_on_write` tells whether the executor has it. Project quotas are not needed.
pub(super) fn binding(
    config: &FilesystemSnapshotsConfig,
    copy_on_write: bool,
) -> Result<Binding<'_>, String> {
    match config {
        FilesystemSnapshotsConfig::Disabled(_) => Ok(Binding::Disabled),
        FilesystemSnapshotsConfig::Enabled(_) if !copy_on_write => Err(
            "filesystem snapshots require storage with copy-on-write copies (XFS with reflink, or Apfs for local development)"
                .to_string(),
        ),
        FilesystemSnapshotsConfig::Enabled(config) => Ok(Binding::Enabled(config)),
    }
}

/// Whether a volume with `space` has room for a new capture: its free space reaches the targets
/// of `pressure`. An unmanaged volume always has room, and a volume whose space is not known has
/// none.
pub(super) fn has_room(
    space: Option<&FilesystemSpace>,
    pressure: &FilesystemPressureConfig,
) -> bool {
    match space {
        Some(FilesystemSpace::Unlimited) => true,
        Some(FilesystemSpace::Observed {
            available_bytes,
            available_filesystem_objects,
            ..
        }) => {
            *available_bytes >= pressure.target_available_bytes()
                && *available_filesystem_objects >= pressure.target_available_filesystem_objects()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests;
