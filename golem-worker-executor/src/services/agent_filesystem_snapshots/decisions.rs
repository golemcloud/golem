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

/// Whether an agent in `mode` keeps filesystem snapshots. Only a durable agent does: nothing
/// restores an ephemeral agent, and nothing deletes the snapshots of one.
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
    Managed(&'a FilesystemSnapshotStoreConfig),
}

/// What `config` binds to. `Managed` needs a sandbox provisioning on managed XFS storage, and
/// `managed_storage` tells whether the executor has it.
pub(super) fn binding(
    config: &FilesystemSnapshotsConfig,
    managed_storage: bool,
) -> Result<Binding<'_>, String> {
    match config {
        FilesystemSnapshotsConfig::Disabled(_) => Ok(Binding::Disabled),
        FilesystemSnapshotsConfig::Managed(_) if !managed_storage => {
            Err("filesystem snapshots require managed XFS storage".to_string())
        }
        FilesystemSnapshotsConfig::Managed(config) => Ok(Binding::Managed(config)),
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
mod tests {
    use super::*;
    use test_r::test;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn only_a_durable_agent_keeps_files() {
        assert_eq!(
            [AgentMode::Durable, AgentMode::Ephemeral].map(keeps_files),
            [true, false]
        );
    }

    #[test]
    fn a_live_target_with_the_own_stage_id_is_published_not_deleted() {
        let own = AgentFingerprint(uuid::Uuid::new_v4());
        let other = AgentFingerprint(uuid::Uuid::new_v4());

        assert_eq!(
            [
                fork_outcome(ForkFound::Published, own),
                fork_outcome(ForkFound::Live(own), own),
                fork_outcome(ForkFound::Live(other), own),
                fork_outcome(ForkFound::Unknown, own),
            ],
            [
                ForkOutcome::Published,
                ForkOutcome::Published,
                ForkOutcome::Lost,
                ForkOutcome::Unknown
            ]
        );
    }

    #[test]
    fn a_refused_publication_whose_target_is_the_own_stage_keeps_it() {
        let own = AgentFingerprint(uuid::Uuid::new_v4());
        let other = AgentFingerprint(uuid::Uuid::new_v4());

        assert_eq!(
            [
                fork_outcome(ForkFound::RefusedThenLive(Some(own)), own),
                fork_outcome(ForkFound::RefusedThenLive(Some(other)), own),
                fork_outcome(ForkFound::RefusedThenLive(None), own),
            ],
            [
                ForkOutcome::Published,
                ForkOutcome::Lost,
                ForkOutcome::Unknown
            ]
        );
    }

    #[test]
    fn only_a_confirmed_periodic_snapshot_deletes_older_ones_and_only_a_superseded_one_is_deleted()
    {
        assert_eq!(
            [
                (SnapshotKind::Periodic, ConfirmOutcome::Confirmed),
                (SnapshotKind::Periodic, ConfirmOutcome::Superseded),
                (SnapshotKind::Periodic, ConfirmOutcome::Deferred),
                (SnapshotKind::Update, ConfirmOutcome::Confirmed),
                (SnapshotKind::Update, ConfirmOutcome::Superseded),
                (SnapshotKind::Update, ConfirmOutcome::Deferred),
            ]
            .map(|(kind, outcome)| follow_up(kind, outcome)),
            [
                FollowUp::DeleteOlder,
                FollowUp::DeleteSuperseded,
                FollowUp::Keep,
                FollowUp::Keep,
                FollowUp::DeleteSuperseded,
                FollowUp::Keep,
            ]
        );
    }

    #[test]
    fn a_start_skips_the_check_after_superseded_or_a_terminal_interrupt_and_limits_it_otherwise() {
        let limits = StoreCheckLimits {
            confirmation_wait: Duration::from_secs(60),
            store_check_limit: Duration::from_secs(5),
        };
        let superseded = Some(JobDecision::Confirmed(ConfirmOutcome::Superseded));
        let deferred = Some(JobDecision::Confirmed(ConfirmOutcome::Deferred));
        assert_eq!(
            [
                store_check(superseded, false, None, limits),
                store_check(None, true, Some(Duration::from_secs(1)), limits),
                store_check(None, false, None, limits),
                store_check(deferred, false, Some(Duration::from_secs(20)), limits),
                store_check(
                    Some(JobDecision::Stopped),
                    false,
                    Some(Duration::from_secs(70)),
                    limits
                ),
            ],
            [
                StoreCheck::Skip,
                StoreCheck::Skip,
                StoreCheck::Stat(Duration::from_secs(5)),
                StoreCheck::Stat(Duration::from_secs(40)),
                StoreCheck::Stat(Duration::ZERO),
            ]
        );
    }

    /// The job that an update with `refusal` waits for.
    fn waits_for(refusal: Refusal) -> Option<JobId> {
        match update_admission(refusal, false, false, None) {
            UpdateAdmit::WaitForEndOrReplacement { running, .. } => Some(running.id),
            UpdateAdmit::Refuse => None,
        }
    }

    /// What an update that waited for the job `stopped` does after `refusal`, before its deadline
    /// and its shutdown with `ends`: the job that it waits for, and whether it stops its deletes.
    fn update_after(
        refusal: Refusal,
        ends: (bool, bool),
        stopped: Option<JobId>,
    ) -> Option<(JobId, bool)> {
        match update_admission(refusal, ends.0, ends.1, stopped) {
            UpdateAdmit::WaitForEndOrReplacement {
                running,
                stop_deletes,
            } => Some((running.id, stop_deletes)),
            UpdateAdmit::Refuse => None,
        }
    }

    #[test]
    fn a_manual_update_refuses_after_its_deadline_and_at_a_shutdown_and_stops_the_deletes_of_a_job_once()
     {
        let in_flight = || Refusal {
            skip: SnapshotSkip::UploadInFlight,
            running: Some(running(4)),
        };

        assert_eq!(
            [
                update_after(in_flight(), (false, false), None),
                update_after(in_flight(), (false, false), Some(4)),
                update_after(in_flight(), (false, false), Some(3)),
                update_after(in_flight(), (true, false), None),
                update_after(in_flight(), (false, true), None),
            ],
            [
                Some((4, true)),
                Some((4, false)),
                Some((4, true)),
                None,
                None,
            ]
        );
    }

    #[test]
    fn a_manual_update_waits_only_for_a_running_upload() {
        assert_eq!(
            [
                waits_for(Refusal {
                    skip: SnapshotSkip::UploadInFlight,
                    running: Some(running(4))
                }),
                waits_for(Refusal {
                    skip: SnapshotSkip::DeletingAllSnapshots,
                    running: Some(running(4))
                }),
                waits_for(Refusal {
                    skip: SnapshotSkip::VolumeUnderPressure,
                    running: Some(running(4))
                }),
                waits_for(Refusal {
                    skip: SnapshotSkip::UploadInFlight,
                    running: None
                }),
            ],
            [Some(4), None, None, None]
        );
    }

    #[test]
    fn managed_snapshots_bind_only_to_managed_storage() {
        let managed = FilesystemSnapshotsConfig::Managed(Box::new(
            FilesystemSnapshotStoreConfig::new(&"0".repeat(128), Duration::from_secs(30), 4, 3)
                .unwrap(),
        ));
        let disabled = FilesystemSnapshotsConfig::default();

        assert!(matches!(binding(&disabled, false), Ok(Binding::Disabled)));
        assert!(matches!(binding(&disabled, true), Ok(Binding::Disabled)));
        assert!(matches!(binding(&managed, true), Ok(Binding::Managed(_))));
        assert_eq!(
            binding(&managed, false).err(),
            Some("filesystem snapshots require managed XFS storage".to_string())
        );
    }

    #[test]
    fn a_volume_has_room_when_its_free_space_reaches_both_targets() {
        let pressure = FilesystemPressureConfig::default();
        let observed = |bytes_above: bool, objects_above: bool| FilesystemSpace::Observed {
            total_bytes: u64::MAX,
            available_bytes: if bytes_above {
                pressure.target_available_bytes()
            } else {
                pressure.target_available_bytes() - 1
            },
            total_filesystem_objects: u64::MAX,
            available_filesystem_objects: if objects_above {
                pressure.target_available_filesystem_objects()
            } else {
                pressure.target_available_filesystem_objects() - 1
            },
        };
        assert_eq!(
            [
                has_room(Some(&FilesystemSpace::Unlimited), &pressure),
                has_room(Some(&observed(true, true)), &pressure),
                has_room(Some(&observed(false, true)), &pressure),
                has_room(Some(&observed(true, false)), &pressure),
                has_room(None, &pressure),
            ],
            [true, true, false, false, false]
        );
    }

    fn running(id: JobId) -> RunningJob {
        RunningJob {
            id,
            retention_stop: CancellationToken::new(),
        }
    }
}
