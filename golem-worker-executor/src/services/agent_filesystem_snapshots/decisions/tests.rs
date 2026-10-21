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
fn only_a_confirmed_periodic_snapshot_deletes_older_ones_and_only_a_superseded_one_is_deleted() {
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
fn enabled_snapshots_bind_only_to_copy_on_write_storage() {
    let enabled = FilesystemSnapshotsConfig::Enabled(Box::new(
        FilesystemSnapshotStoreConfig::new(&"0".repeat(128), Duration::from_secs(30), 4, 3)
            .unwrap(),
    ));
    let disabled = FilesystemSnapshotsConfig::default();

    assert!(matches!(binding(&disabled, false), Ok(Binding::Disabled)));
    assert!(matches!(binding(&disabled, true), Ok(Binding::Disabled)));
    assert!(matches!(binding(&enabled, true), Ok(Binding::Enabled(_))));
    assert_eq!(
            binding(&enabled, false).err(),
            Some(
                "filesystem snapshots require storage with copy-on-write copies (XFS with reflink, or Apfs for local development)"
                    .to_string()
            )
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
