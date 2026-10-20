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
use golem_common::model::AgentId;
use golem_common::model::Timestamp;
use std::sync::Arc;
use test_r::test;

/// Gives a mark of a new generation and a later mark of the same generation.
fn marks() -> (TreeMark, TreeMark) {
    crate::services::agent_filesystem::test_tree_marks()
}

fn confirmed(name: &FilesystemSnapshotName, mark: TreeMark) -> ConfirmedFilesystemSnapshot {
    ConfirmedFilesystemSnapshot {
        name: Some(name.clone()),
        mark,
        baseline: ConfirmedBaseline::Periodic,
    }
}

fn without_name(mark: TreeMark) -> ConfirmedFilesystemSnapshot {
    ConfirmedFilesystemSnapshot {
        name: None,
        mark,
        baseline: ConfirmedBaseline::Periodic,
    }
}

fn selected(name: Option<&FilesystemSnapshotName>) -> UsableAutomaticSnapshot {
    UsableAutomaticSnapshot {
        index: OplogIndex::from_u64(10),
        component_revision: golem_common::model::component::ComponentRevision::INITIAL,
        filesystem_snapshot: name.cloned(),
    }
}

#[test]
async fn the_capture_compares_with_the_confirmed_snapshot_only_while_a_start_selects_it() {
    let (mark, _) = marks();
    let name = FilesystemSnapshotName::periodic();
    let other = FilesystemSnapshotName::periodic();
    let periodic = confirmed(&name, mark);
    let manual = ConfirmedFilesystemSnapshot {
        baseline: ConfirmedBaseline::Authoritative(OplogIndex::from_u64(4)),
        ..periodic.clone()
    };

    let cases = [
        since(Some(&periodic), Some(&selected(Some(&name))), None).is_some(),
        since(Some(&periodic), Some(&selected(Some(&other))), None).is_some(),
        since(Some(&periodic), Some(&selected(None)), None).is_some(),
        since(Some(&periodic), None, None).is_some(),
        since(Some(&manual), None, Some(OplogIndex::from_u64(4))).is_some(),
        since(Some(&manual), None, Some(OplogIndex::from_u64(5))).is_some(),
        since(
            Some(&manual),
            Some(&selected(None)),
            Some(OplogIndex::from_u64(4)),
        )
        .is_some(),
        since(None, Some(&selected(Some(&name))), None).is_some(),
    ];

    assert_eq!(
        cases,
        [true, false, false, false, true, false, false, false]
    );
}

#[test]
async fn a_snapshot_without_a_name_is_compared_with_only_while_a_start_selects_a_record_without_a_name()
 {
    let (mark, _) = marks();
    let written = without_name(mark);
    let name = FilesystemSnapshotName::periodic();

    let cases = [
        since(Some(&written), Some(&selected(None)), None),
        since(Some(&written), Some(&selected(Some(&name))), None),
        since(Some(&written), None, None),
    ];

    assert_eq!(cases, [Some(written), None, None]);
}

fn captured(detection: ChangeDetection) -> CaptureFinding<u8> {
    CaptureFinding::Captured { copy: 1, detection }
}

#[test]
async fn the_record_of_a_periodic_snapshot_follows_the_finding_of_the_capture() {
    let (mark, _) = marks();
    let name = FilesystemSnapshotName::periodic();
    let since = confirmed(&name, mark);

    let initial = without_name(mark);

    let cases = [
        plan_periodic_record(CaptureFinding::Unchanged, Some(&since)),
        plan_periodic_record(CaptureFinding::Unchanged, Some(&initial)),
        plan_periodic_record(CaptureFinding::Unchanged, None),
        plan_periodic_record(CaptureFinding::InitialFiles { mark }, Some(&since)),
        plan_periodic_record(CaptureFinding::InitialFiles { mark }, None),
        plan_periodic_record(captured(ChangeDetection::SizeMtime), Some(&since)),
        plan_periodic_record(captured(ChangeDetection::SizeMtime), Some(&initial)),
        plan_periodic_record(captured(ChangeDetection::Full), Some(&since)),
        plan_periodic_record(captured(ChangeDetection::Full), None),
    ];

    assert_eq!(
        cases,
        [
            Some(PeriodicRecord::Reused(name.clone())),
            Some(PeriodicRecord::WithoutName),
            None,
            Some(PeriodicRecord::InitialFiles { mark }),
            Some(PeriodicRecord::InitialFiles { mark }),
            Some(PeriodicRecord::Own {
                copy: 1,
                parent: Some((name, StoreChangeDetection::SizeMtime)),
            }),
            Some(PeriodicRecord::Own {
                copy: 1,
                parent: None
            }),
            Some(PeriodicRecord::Own {
                copy: 1,
                parent: None
            }),
            Some(PeriodicRecord::Own {
                copy: 1,
                parent: None
            }),
        ]
    );
}

#[test]
async fn only_the_owner_of_the_agent_passes_the_gate_before_the_admission() {
    let (mark, _) = marks();
    let attempt = Uuid::new_v4();
    let running = Confirmer::Running(mark);
    let start = Confirmer::Start(attempt);
    let matching = InstanceView::Running {
        generation_matches: true,
    };

    assert_eq!(
        [
            owner_gate(matching, &running, false, false, false),
            owner_gate(matching, &running, true, false, false),
            owner_gate(
                InstanceView::Running {
                    generation_matches: false
                },
                &running,
                false,
                false,
                false
            ),
            owner_gate(matching, &start, false, false, false),
            owner_gate(
                InstanceView::WaitingForPermit(attempt),
                &start,
                false,
                false,
                false
            ),
            owner_gate(
                InstanceView::WaitingForPermit(attempt),
                &start,
                true,
                false,
                false
            ),
            owner_gate(
                InstanceView::WaitingForPermit(Uuid::new_v4()),
                &start,
                false,
                false,
                false
            ),
            owner_gate(
                InstanceView::WaitingForPermit(attempt),
                &running,
                false,
                false,
                false
            ),
            owner_gate(InstanceView::Other, &running, false, false, false),
            owner_gate(matching, &running, false, true, false),
            owner_gate(matching, &running, false, false, true),
        ],
        [
            OwnerGate::NeedsAdmission,
            OwnerGate::NeedsAdmission,
            OwnerGate::Refused,
            OwnerGate::Refused,
            OwnerGate::NeedsAdmission,
            OwnerGate::Refused,
            OwnerGate::Refused,
            OwnerGate::Refused,
            OwnerGate::Refused,
            OwnerGate::Refused,
            OwnerGate::Refused,
        ]
    );
}

#[test]
async fn the_admission_is_asked_once_and_only_when_the_gate_needs_it() {
    let asked = std::cell::Cell::new(0);
    let admission = |admitted| {
        let asked = &asked;
        move || {
            asked.set(asked.get() + 1);
            admitted
        }
    };

    let refused = admit_if_needed(OwnerGate::Refused, admission(true));
    let asked_after_refused = asked.get();
    let admitted = admit_if_needed(OwnerGate::NeedsAdmission, admission(true));
    let not_admitted = admit_if_needed(OwnerGate::NeedsAdmission, admission(false));

    assert_eq!(
        (
            refused,
            asked_after_refused,
            admitted,
            not_admitted,
            asked.get()
        ),
        (false, 0, true, false, 2)
    );
}

#[test]
async fn the_revision_before_an_update_record_follows_the_oplog_order_and_not_the_clocks() {
    let update =
        |millis: u64, revision: u64, index: u64| golem_common::model::SuccessfulUpdateRecord {
            timestamp: Timestamp::from(millis),
            target_revision: ComponentRevision::new(revision).unwrap(),
            oplog_index: OplogIndex::from_u64(index),
            filesystem_snapshot: None,
            pending_update: None,
            snapshot_assisted_details: None,
        };
    // The update to revision 3 applies the pending record at index 7 on an executor whose
    // clock is behind: its timestamp is earlier than the pending record, and its index is
    // later.
    let updates = [update(1_000, 2, 3), update(4_000, 3, 9)];

    assert_eq!(
        [
            revision_before(&updates, OplogIndex::from_u64(7)),
            revision_before(&updates, OplogIndex::from_u64(10)),
            revision_before(&updates, OplogIndex::from_u64(3)),
            revision_before(&[], OplogIndex::from_u64(7)),
        ],
        [
            Some(ComponentRevision::new(2).unwrap()),
            Some(ComponentRevision::new(3).unwrap()),
            None,
            None,
        ]
    );
}

fn head(
    kind: golem_common::model::PendingUpdateKind,
    index: u64,
) -> golem_common::model::PendingUpdateRef {
    golem_common::model::PendingUpdateRef {
        timestamp: Timestamp::from(1_000),
        oplog_index: OplogIndex::from_u64(index),
        admission_index: OplogIndex::from_u64(index),
        target_revision: ComponentRevision::new(3).unwrap(),
        kind,
    }
}

fn record(index: u64, name: Option<&FilesystemSnapshotName>) -> UsableAutomaticSnapshot {
    UsableAutomaticSnapshot {
        index: OplogIndex::from_u64(index),
        component_revision: ComponentRevision::new(2).unwrap(),
        filesystem_snapshot: name.cloned(),
    }
}

/// One plan gives the tree and the replay inputs of every baseline: a start loads the
/// application snapshot only of the record whose files it restores, skips the history before
/// a pending record, and keeps only the regions of the status for an authoritative one.
#[test]
async fn a_start_plans_the_tree_and_the_replay_of_its_baseline_from_one_record() {
    use crate::worker::snapshot_selection::SelectedBaseline;
    use golem_common::model::{AssistedSelection, AuthoritativeSnapshot, PendingUpdateKind};
    let name = FilesystemSnapshotName::periodic();
    let update = FilesystemSnapshotName::update();
    let index = OplogIndex::from_u64;
    let assisted_head = |name: Option<&FilesystemSnapshotName>| {
        Arc::new(head(
            PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
                source_revision_start_index: index(4),
                snapshot: record(10, name),
            })),
            12,
        ))
    };
    let manual_head = || {
        Arc::new(head(
            PendingUpdateKind::SnapshotBased {
                filesystem_snapshot: None,
            },
            7,
        ))
    };
    let previous = AuthoritativeSnapshot {
        index: index(5),
        kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
            filesystem_snapshot: None,
        },
    };
    let plan = |source, snapshot, skip_through, role| StartPlan {
        source,
        replay: ReplayBaseline {
            snapshot,
            skip_through,
            role,
        },
    };
    let periodic = SnapshotSource::Automatic;
    let assisted = SnapshotSource::SnapshotAssistedAutomatic;
    let manual = SnapshotSource::ManualUpdate;
    // (selected, manual record name, enabled, expected plan)
    type Case<'a> = (
        SelectedBaseline,
        Option<&'a FilesystemSnapshotName>,
        bool,
        Result<StartPlan, ()>,
    );
    let cases: Vec<Case<'_>> = vec![
        (
            SelectedBaseline::Periodic(record(10, Some(&name))),
            None,
            true,
            Ok(plan(
                TreeSource::Store(name.clone()),
                Some((index(10), periodic)),
                Some(index(10)),
                BaselineRole::Periodic(index(10)),
            )),
        ),
        (
            SelectedBaseline::Periodic(record(10, Some(&name))),
            None,
            false,
            Err(()),
        ),
        (
            SelectedBaseline::Periodic(record(10, None)),
            None,
            false,
            Ok(plan(
                TreeSource::InitialFiles,
                Some((index(10), periodic)),
                Some(index(10)),
                BaselineRole::Periodic(index(10)),
            )),
        ),
        (
            SelectedBaseline::AssistedPending {
                snapshot: record(10, Some(&name)),
                head: assisted_head(Some(&name)),
            },
            None,
            true,
            Ok(plan(
                TreeSource::Store(name.clone()),
                Some((index(10), assisted)),
                Some(index(10)),
                BaselineRole::AssistedPending(assisted_head(Some(&name))),
            )),
        ),
        (
            SelectedBaseline::AssistedPending {
                snapshot: record(10, Some(&name)),
                head: assisted_head(Some(&name)),
            },
            None,
            false,
            Err(()),
        ),
        (
            SelectedBaseline::AssistedPending {
                snapshot: record(10, None),
                head: assisted_head(None),
            },
            None,
            false,
            Ok(plan(
                TreeSource::InitialFiles,
                Some((index(10), assisted)),
                Some(index(10)),
                BaselineRole::AssistedPending(assisted_head(None)),
            )),
        ),
        (
            SelectedBaseline::AssistedPromoted {
                index: index(10),
                name: Some(name.clone()),
            },
            None,
            true,
            Ok(plan(
                TreeSource::Store(name.clone()),
                Some((index(10), assisted)),
                None,
                BaselineRole::AssistedPromoted,
            )),
        ),
        (
            SelectedBaseline::AssistedPromoted {
                index: index(10),
                name: Some(name.clone()),
            },
            None,
            false,
            Err(()),
        ),
        (
            SelectedBaseline::AssistedPromoted {
                index: index(10),
                name: None,
            },
            None,
            false,
            Ok(plan(
                TreeSource::InitialFiles,
                Some((index(10), assisted)),
                None,
                BaselineRole::AssistedPromoted,
            )),
        ),
        (
            SelectedBaseline::ManualPending {
                head: manual_head(),
                previous: Some(previous.clone()),
            },
            Some(&update),
            true,
            Ok(plan(
                TreeSource::Store(update.clone()),
                Some((index(5), assisted)),
                Some(index(7)),
                BaselineRole::ManualPending(manual_head()),
            )),
        ),
        (
            SelectedBaseline::ManualPending {
                head: manual_head(),
                previous: Some(AuthoritativeSnapshot {
                    index: index(5),
                    kind: AuthoritativeSnapshotKind::ManualUpdate,
                }),
            },
            Some(&update),
            true,
            Ok(plan(
                TreeSource::Store(update.clone()),
                Some((index(5), manual)),
                Some(index(7)),
                BaselineRole::ManualPending(manual_head()),
            )),
        ),
        (
            SelectedBaseline::ManualPending {
                head: manual_head(),
                previous: None,
            },
            Some(&update),
            false,
            Err(()),
        ),
        (
            SelectedBaseline::ManualPending {
                head: manual_head(),
                previous: None,
            },
            None,
            false,
            Ok(plan(
                TreeSource::SourceFiles(SourceRevision::Current),
                None,
                Some(index(7)),
                BaselineRole::ManualPending(manual_head()),
            )),
        ),
        (
            SelectedBaseline::ManualPromoted { index: index(7) },
            Some(&update),
            true,
            Ok(plan(
                TreeSource::Store(update.clone()),
                Some((index(7), manual)),
                None,
                BaselineRole::ManualPromoted,
            )),
        ),
        (
            SelectedBaseline::ManualPromoted { index: index(7) },
            Some(&update),
            false,
            Err(()),
        ),
        (
            SelectedBaseline::ManualPromoted { index: index(7) },
            None,
            true,
            Ok(plan(
                TreeSource::SourceFiles(SourceRevision::Before(index(7))),
                Some((index(7), manual)),
                None,
                BaselineRole::ManualPromoted,
            )),
        ),
        (
            SelectedBaseline::InitialFiles,
            None,
            false,
            Ok(plan(
                TreeSource::InitialFiles,
                None,
                None,
                BaselineRole::InitialFiles,
            )),
        ),
    ];
    cases
        .into_iter()
        .for_each(|(selected, manual_name, enabled, expected)| {
            assert_eq!(
                plan_start(&selected, manual_name, enabled).map_err(|_| ()),
                expected,
                "{selected:?} with {manual_name:?}, enabled: {enabled}"
            );
        });
}

#[test]
async fn the_skipped_regions_of_a_start_skip_the_history_before_a_pending_record_only() {
    let index = OplogIndex::from_u64;
    let status_regions = {
        let mut regions = DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
            index(3)..=index(4),
        )])
        .build();
        regions.set_override(
            DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
                index(2)..=index(9),
            )])
            .build(),
        );
        regions
    };
    let replay = |skip_through| ReplayBaseline {
        snapshot: None,
        skip_through,
        role: BaselineRole::InitialFiles,
    };
    let pending = replay(Some(index(6))).skipped_regions(&status_regions);
    let promoted = replay(None).skipped_regions(&status_regions);

    assert_eq!(
        (
            pending.get_override(),
            pending.is_in_deleted_region(index(5)),
            promoted.get_override(),
            promoted.is_in_deleted_region(index(5)),
            promoted.is_in_deleted_region(index(3)),
        ),
        (
            Some(
                DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
                    index(2)..=index(6),
                )])
                .build()
            ),
            true,
            None,
            false,
            true,
        )
    );

    let plain = DeletedRegionsBuilder::from_regions(vec![OplogRegion::from_index_range(
        index(3)..=index(4),
    )])
    .build();
    assert_eq!(replay(None).skipped_regions(&plain), plain);
}

#[test]
async fn only_a_non_empty_set_of_read_only_initial_files_restores() {
    use golem_common::model::agent::AgentFileContentHash;
    use golem_common::model::component::{AgentFilePath, AgentFilePermissions, InitialAgentFile};
    let file = |permissions| InitialAgentFile {
        content_hash: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
        path: AgentFilePath::from_abs_str("/file").unwrap(),
        permissions,
        size: 1,
    };
    assert_eq!(
        [
            InitialFilesRestore::of_read_only(Box::new([])).is_some(),
            InitialFilesRestore::of_read_only(Box::new([file(AgentFilePermissions::ReadOnly)]))
                .is_some(),
            InitialFilesRestore::of_read_only(Box::new([
                file(AgentFilePermissions::ReadOnly),
                file(AgentFilePermissions::ReadWrite),
            ]))
            .is_some(),
        ],
        [false, true, false]
    );
}

#[test]
async fn only_a_start_restore_from_the_store_gives_the_times_of_a_save() {
    use golem_common::model::agent::AgentFileContentHash;
    use golem_common::model::component::{AgentFilePath, AgentFilePermissions, InitialAgentFile};
    let (snapshots, shutdown) = enabled_service();
    let store = StartRestore::Store(
        snapshots
            .restore(
                &agent_snapshots("saved-times"),
                &FilesystemSnapshotName::periodic(),
            )
            .unwrap(),
    );
    let initial_files = StartRestore::InitialFiles(
        InitialFilesRestore::of_read_only(Box::new([InitialAgentFile {
            content_hash: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
            path: AgentFilePath::from_abs_str("/file").unwrap(),
            permissions: AgentFilePermissions::ReadOnly,
            size: 1,
        }]))
        .unwrap(),
    );

    assert_eq!(
        [store.gives_saved_times(), initial_files.gives_saved_times()],
        [true, false]
    );
    shut_down(shutdown).await;
}

/// A host that records its calls and answers as the test says.
struct ScriptedHost {
    calls: std::sync::Mutex<Vec<Box<str>>>,
    guest: Option<Result<RawSnapshotData, &'static str>>,
    since: Option<ConfirmedFilesystemSnapshot>,
    capture: std::sync::Mutex<Option<CaptureOutcome>>,
    whole: std::sync::Mutex<Option<WholeCapture>>,
    check: std::sync::Mutex<Option<InitialFilesCheck>>,
    entry_fails: bool,
    write_fails: bool,
    terminal: watch::Sender<bool>,
    lost_shard: bool,
}

impl ScriptedHost {
    fn new() -> Self {
        Self {
            calls: std::sync::Mutex::default(),
            guest: Some(Ok(RawSnapshotData {
                data: vec![1],
                mime_type: "application/octet-stream".to_string(),
            })),
            since: None,
            capture: std::sync::Mutex::default(),
            whole: std::sync::Mutex::default(),
            check: std::sync::Mutex::default(),
            entry_fails: false,
            write_fails: false,
            terminal: watch::channel(false).0,
            lost_shard: false,
        }
    }

    fn call(&self, call: String) {
        self.calls.lock().unwrap().push(call.into_boxed_str());
    }

    fn check(&self, wait: Duration) -> Result<InitialFilesCheck, CaptureError> {
        self.call(format!("check({wait:?})"));
        self.check.lock().unwrap().take().ok_or(CaptureError::Busy)
    }

    fn checking(check: InitialFilesCheck) -> Self {
        Self {
            check: std::sync::Mutex::new(Some(check)),
            ..Self::new()
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.to_string())
            .collect()
    }
}

fn named(name: Option<&FilesystemSnapshotName>) -> String {
    name.map_or_else(|| "none".to_string(), |name| name.as_str().to_string())
}

impl PeriodicSnapshotHost for ScriptedHost {
    type Stop = &'static str;

    async fn snapshot_guest(&mut self) -> Result<RawSnapshotData, &'static str> {
        self.call("snapshot_guest".to_string());
        self.guest.take().unwrap_or(Err("saved twice"))
    }

    fn since(&self) -> Option<ConfirmedFilesystemSnapshot> {
        self.call("since".to_string());
        self.since.clone()
    }

    async fn capture(&self, _wait: Duration, since: Option<TreeMark>) -> Option<CaptureOutcome> {
        self.call(format!("capture({})", since.is_some()));
        self.capture.lock().unwrap().take()
    }

    async fn check_initial_files(&self, wait: Duration) -> Result<InitialFilesCheck, CaptureError> {
        ScriptedHost::check(self, wait)
    }

    async fn entry(
        &self,
        _snapshot: RawSnapshotData,
        name: Option<FilesystemSnapshotName>,
    ) -> Result<OplogEntry, String> {
        self.call(format!("entry({})", named(name.as_ref())));
        if self.entry_fails {
            Err("no payload".to_string())
        } else {
            Ok(OplogEntry::interrupted())
        }
    }

    async fn write(
        &self,
        confirmed_at_once: Option<FilesystemSnapshotName>,
        _entry: OplogEntry,
    ) -> Result<(), OplogError> {
        self.call(format!("write({})", named(confirmed_at_once.as_ref())));
        if self.write_fails {
            Err(OplogError::Payload("refused".to_string()))
        } else {
            Ok(())
        }
    }

    fn confirm(&self, _mark: TreeMark) -> Confirm {
        self.call("confirm".to_string());
        Box::new(|_| Box::pin(async { Confirmation::Deferred }))
    }

    fn initial_files_written(&self, _mark: TreeMark) {
        self.call("initial_files_written".to_string());
    }
}

impl UpdateSnapshotHost for ScriptedHost {
    type Stop = &'static str;

    async fn snapshot_guest(&mut self) -> Result<RawSnapshotData, &'static str> {
        PeriodicSnapshotHost::snapshot_guest(self).await
    }

    async fn capture_whole(&self, _wait: Duration) -> Option<WholeCapture> {
        self.call("capture_whole".to_string());
        self.whole.lock().unwrap().take()
    }

    async fn check_initial_files(&self, wait: Duration) -> Result<InitialFilesCheck, CaptureError> {
        ScriptedHost::check(self, wait)
    }

    fn terminal(&self) -> watch::Receiver<bool> {
        self.terminal.subscribe()
    }

    fn lost_shard(&self) -> super::super::LostShard {
        watch::channel(
            self.lost_shard
                .then_some(super::super::RetirementReason::ShardRevoked),
        )
        .1
    }
}

fn agent_snapshots(name: &str) -> AgentSnapshots {
    AgentSnapshots::agent(
        &golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId::new(),
            &AgentId {
                component_id: golem_common::model::component::ComponentId::new(),
                agent_id: name.to_string(),
            },
        ),
        golem_common::model::AgentFingerprint(uuid::Uuid::new_v4()),
    )
}

/// An enabled service over an in-memory store, with the shutdown that keeps it running.
fn enabled_service() -> (
    Arc<AgentFilesystemSnapshots>,
    crate::services::shutdown::Shutdown,
) {
    let shutdown = crate::services::shutdown::Shutdown::new();
    let snapshots = AgentFilesystemSnapshots::bind(
        &crate::services::golem_config::FilesystemSnapshotsConfig::default(),
        crate::services::agent_filesystem_snapshots::StoreSource::given(
            Arc::new(crate::filesystem_snapshot::InMemorySnapshotStore::new()),
            crate::services::golem_config::FilesystemSnapshotUploadConfig::default(),
        ),
        false,
        &shutdown,
    )
    .unwrap();
    (snapshots, shutdown)
}

/// Shuts down the service of `shutdown`, and waits until it stopped.
async fn shut_down(shutdown: crate::services::shutdown::Shutdown) {
    shutdown.cancel();
    assert!(
        shutdown.wait_for_tracked(Duration::from_secs(10)).await,
        "the filesystem snapshot service did not stop"
    );
}

/// A service that keeps no filesystem snapshots, with the shutdown that keeps it running.
fn disabled_service() -> (
    Arc<AgentFilesystemSnapshots>,
    crate::services::shutdown::Shutdown,
) {
    let shutdown = crate::services::shutdown::Shutdown::new();
    let snapshots = AgentFilesystemSnapshots::bind(
        &crate::services::golem_config::FilesystemSnapshotsConfig::default(),
        crate::services::agent_filesystem_snapshots::StoreSource::configured(
            Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new()),
            crate::services::agent_filesystem_snapshots::VolumeRoom::Unlimited,
        ),
        false,
        &shutdown,
    )
    .unwrap();
    (snapshots, shutdown)
}

fn outcome<Stop: std::fmt::Debug>(result: &PeriodicResult<Stop>) -> String {
    format!("{result:?}")
}

fn strings(calls: &[&str]) -> Vec<String> {
    calls.iter().map(|call| call.to_string()).collect()
}

#[test]
async fn without_snapshots_a_periodic_record_is_written_only_for_a_tree_of_initial_files() {
    let (disabled, disabled_shutdown) = disabled_service();
    let agent = agent_snapshots("periodic-disabled");
    let run = |host: ScriptedHost| {
        let disabled = disabled.as_ref();
        let agent = &agent;
        async move {
            let mut host = host;
            let result = periodic_snapshot(&mut host, disabled, agent).await;
            (outcome(&result), host.calls())
        }
    };

    let written = run(ScriptedHost::checking(InitialFilesCheck::InitialFiles)).await;
    let changed = run(ScriptedHost::checking(InitialFilesCheck::Changed)).await;
    let check_failed = run(ScriptedHost::new()).await;
    let guest_stop = run(ScriptedHost {
        guest: Some(Err("stop")),
        ..ScriptedHost::checking(InitialFilesCheck::InitialFiles)
    })
    .await;
    let no_entry = run(ScriptedHost {
        entry_fails: true,
        ..ScriptedHost::checking(InitialFilesCheck::InitialFiles)
    })
    .await;
    let not_written = run(ScriptedHost {
        write_fails: true,
        ..ScriptedHost::checking(InitialFilesCheck::InitialFiles)
    })
    .await;

    assert_eq!(
        written,
        (
            "Continue".to_string(),
            strings(&["snapshot_guest", "check(5s)", "entry(none)", "write(none)"])
        )
    );
    assert_eq!(
        changed,
        (
            "Continue".to_string(),
            strings(&["snapshot_guest", "check(5s)"])
        )
    );
    assert_eq!(
        check_failed,
        (
            "Continue".to_string(),
            strings(&["snapshot_guest", "check(5s)"])
        )
    );
    assert_eq!(
        guest_stop,
        ("Guest(\"stop\")".to_string(), strings(&["snapshot_guest"]))
    );
    assert_eq!(
        no_entry,
        (
            "NotWritten(Entry(\"no payload\"))".to_string(),
            strings(&["snapshot_guest", "check(5s)", "entry(none)"])
        )
    );
    assert_eq!(
        not_written.0,
        "NotWritten(Write(Payload(\"refused\")))".to_string()
    );
    shut_down(disabled_shutdown).await;
}

#[test]
async fn a_manual_update_without_snapshots_needs_a_tree_of_initial_files() {
    let (disabled, disabled_shutdown) = disabled_service();
    let agent = agent_snapshots("update-disabled");
    let run = |host: ScriptedHost| {
        let disabled = disabled.as_ref();
        let agent = &agent;
        async move {
            let mut host = host;
            let result = update_snapshot(&mut host, disabled, agent).await;
            (update_outcome(&result), host.calls())
        }
    };

    let initial_files = run(ScriptedHost::checking(InitialFilesCheck::InitialFiles)).await;
    let changed = run(ScriptedHost::checking(InitialFilesCheck::Changed)).await;
    let check_failed = run(ScriptedHost::new()).await;

    assert_eq!(
        initial_files,
        (
            "Saved(none, false)".to_string(),
            strings(&["snapshot_guest", "check(5s)"])
        )
    );
    assert_eq!(
        changed,
        (
            format!("Fail({})", needs_filesystem_snapshots()),
            strings(&["snapshot_guest", "check(5s)"])
        )
    );
    assert_eq!(
        check_failed,
        (
            "Fail(failed to check the agent filesystem for the update: agent filesystem has \
                 an open call)"
                .to_string(),
            strings(&["snapshot_guest", "check(5s)"])
        )
    );
    assert_eq!(
        needs_filesystem_snapshots(),
        "UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS: cannot take a snapshot for the update: the files \
             of the agent differ from its initial files, and filesystem snapshots are disabled on \
             this executor"
    );
    shut_down(disabled_shutdown).await;
}

#[test]
async fn a_periodic_snapshot_admits_before_the_guest_saves_and_captures_after() {
    let (mark, _) = marks();
    let (snapshots, shutdown) = enabled_service();
    let agent = agent_snapshots("periodic-enabled");
    let since = confirmed(&FilesystemSnapshotName::periodic(), mark);

    let held = snapshots.admit_periodic(&agent).await.unwrap();
    let mut refused = ScriptedHost::new();
    let while_held = outcome(&periodic_snapshot(&mut refused, &snapshots, &agent).await);
    drop(held);
    let mut failed_capture = ScriptedHost::new();
    let capture_failed = outcome(&periodic_snapshot(&mut failed_capture, &snapshots, &agent).await);
    let mut initial = ScriptedHost {
        capture: std::sync::Mutex::new(Some(CaptureOutcome::InitialFiles { mark })),
        ..ScriptedHost::new()
    };
    let initial_files = outcome(&periodic_snapshot(&mut initial, &snapshots, &agent).await);
    let mut unchanged = ScriptedHost {
        since: Some(since.clone()),
        capture: std::sync::Mutex::new(Some(CaptureOutcome::Unchanged)),
        ..ScriptedHost::new()
    };
    let reused = outcome(&periodic_snapshot(&mut unchanged, &snapshots, &agent).await);
    let free_after = snapshots.admit_periodic(&agent).await.is_ok();

    let name = since.name.as_ref().unwrap().as_str().to_string();
    assert_eq!(
        (while_held, refused.calls()),
        ("Continue".to_string(), vec![])
    );
    assert_eq!(
        (capture_failed, failed_capture.calls()),
        (
            "Continue".to_string(),
            vec!["snapshot_guest", "since", "capture(false)"]
                .into_iter()
                .map(String::from)
                .collect()
        )
    );
    assert_eq!(
        (initial_files, initial.calls()),
        (
            "Continue".to_string(),
            vec![
                "snapshot_guest",
                "since",
                "capture(false)",
                "entry(none)",
                "write(none)",
                "initial_files_written"
            ]
            .into_iter()
            .map(String::from)
            .collect()
        )
    );
    assert_eq!(
        (reused, unchanged.calls()),
        (
            "Continue".to_string(),
            vec![
                "snapshot_guest".to_string(),
                "since".to_string(),
                "capture(true)".to_string(),
                format!("entry({name})"),
                format!("write({name})"),
            ]
        )
    );
    assert!(free_after);
    shut_down(shutdown).await;
}

fn update_outcome(result: &UpdateSnapshot<&'static str>) -> String {
    match result {
        UpdateSnapshot::Saved {
            name, retention, ..
        } => {
            format!("Saved({}, {})", named(name.as_ref()), retention.is_some())
        }
        UpdateSnapshot::Fail(details) => format!("Fail({details})"),
        UpdateSnapshot::WriteNothing => "WriteNothing".to_string(),
        UpdateSnapshot::Guest(stop) => format!("Guest({stop})"),
    }
}

#[test]
async fn a_manual_update_snapshot_saves_the_guest_then_captures_the_whole_tree() {
    let (disabled, disabled_shutdown) = disabled_service();
    let (snapshots, shutdown) = enabled_service();
    let agent = agent_snapshots("update");

    let mut without = ScriptedHost::checking(InitialFilesCheck::InitialFiles);
    let without_snapshots = update_outcome(&update_snapshot(&mut without, &disabled, &agent).await);
    let mut stopped = ScriptedHost {
        guest: Some(Err("stop")),
        ..ScriptedHost::new()
    };
    let guest_stop = update_outcome(&update_snapshot(&mut stopped, &snapshots, &agent).await);
    let mut failed = ScriptedHost::new();
    let capture_failed = update_outcome(&update_snapshot(&mut failed, &snapshots, &agent).await);
    let mut initial = ScriptedHost {
        whole: std::sync::Mutex::new(Some(WholeCapture::InitialFiles)),
        ..ScriptedHost::new()
    };
    let initial_files = update_outcome(&update_snapshot(&mut initial, &snapshots, &agent).await);

    assert_eq!(
        (without_snapshots, without.calls()),
        (
            "Saved(none, false)".to_string(),
            vec!["snapshot_guest".to_string(), "check(5s)".to_string()]
        )
    );
    assert_eq!(guest_stop, "Guest(stop)");
    assert_eq!(
        (capture_failed, failed.calls()),
        (
            "Fail(failed to capture the agent filesystem for the update)".to_string(),
            vec!["snapshot_guest".to_string(), "capture_whole".to_string()]
        )
    );
    assert_eq!(initial_files, "Saved(none, false)");
    assert!(snapshots.admit_periodic(&agent).await.is_ok());
    shut_down(disabled_shutdown).await;
    shut_down(shutdown).await;
}

#[test]
async fn an_interrupted_wait_of_a_manual_update_fails_it_or_writes_nothing_on_a_lost_shard() {
    let (snapshots, shutdown) = enabled_service();
    let agent = agent_snapshots("update-interrupted");
    let held = snapshots.admit_periodic(&agent).await.unwrap();
    let run = |lost_shard| {
        let snapshots = &snapshots;
        let agent = &agent;
        async move {
            let mut host = ScriptedHost {
                lost_shard,
                ..ScriptedHost::new()
            };
            host.terminal.send_replace(true);
            let result = update_snapshot(&mut host, snapshots, agent).await;
            (update_outcome(&result), host.calls())
        }
    };

    let failed = run(false).await;
    let lost = run(true).await;
    drop(held);

    assert_eq!(
        failed,
        (
            "Fail(the update was interrupted while it waited for an upload of a filesystem \
                 snapshot of the agent)"
                .to_string(),
            Vec::<String>::new()
        )
    );
    assert_eq!(lost, ("WriteNothing".to_string(), Vec::<String>::new()));
    shut_down(shutdown).await;
}

#[test]
async fn an_upload_that_a_stop_ended_is_interrupted_and_a_failed_one_fails() {
    let store = UploadNowError::Store(crate::filesystem_snapshot::SaveError::NameInUse);
    let uploaded = |error: &UploadNowError, lost_shard: bool| {
        update_outcome(&failed_update_upload(error, lost_shard))
    };
    assert_eq!(
        [
            uploaded(&UploadNowError::Stopped, false),
            uploaded(&UploadNowError::Stopped, true),
            uploaded(&store, false),
            uploaded(&store, true),
            uploaded(&UploadNowError::NoSlot, false),
        ],
        [
            "Fail(the update was interrupted while it uploaded the filesystem snapshot)",
            "WriteNothing",
            "Fail(failed to upload the filesystem snapshot for the update: a filesystem \
                 snapshot already has the name)",
            "Fail(failed to upload the filesystem snapshot for the update: a filesystem \
                 snapshot already has the name)",
            "Fail(failed to upload the filesystem snapshot for the update: no slot of the \
                 uploads of filesystem snapshots was free in the wait)",
        ]
        .map(String::from)
    );
}

#[test]
async fn a_confirmation_of_another_generation_leaves_the_slot_unchanged() {
    let (first, later) = marks();
    let (other_generation, _) = marks();
    let restored = FilesystemSnapshotName::periodic();
    let confirmed_now = FilesystemSnapshotName::periodic();
    let slot = FilesystemSnapshotSlot::at_start(
        first,
        Some((restored.clone(), ConfirmedBaseline::Periodic)),
    );

    let after_other = confirmed_slot(
        &slot,
        Some(&FilesystemSnapshotName::periodic()),
        other_generation,
    );
    let after_own = confirmed_slot(&slot, Some(&confirmed_now), later);

    assert!(after_other.is_none());
    assert_eq!(
        slot.confirmed().map(|confirmed| confirmed.name.clone()),
        Some(Some(restored))
    );
    assert_eq!(
        after_own
            .as_ref()
            .and_then(|slot| slot.confirmed().cloned()),
        Some(confirmed(&confirmed_now, later))
    );
}

#[test]
async fn a_baseline_restored_the_name_of_its_record_as_a_periodic_or_an_authoritative_one() {
    use golem_common::model::PendingUpdateKind;
    let name = FilesystemSnapshotName::periodic();
    let update = FilesystemSnapshotName::update();
    let index = OplogIndex::from_u64;
    let plan = |source, snapshot, role| StartPlan {
        source,
        replay: ReplayBaseline {
            snapshot,
            skip_through: None,
            role,
        },
    };
    let manual_head = Arc::new(head(
        PendingUpdateKind::SnapshotBased {
            filesystem_snapshot: Some(update.clone()),
        },
        7,
    ));

    assert_eq!(
        [
            plan(TreeSource::InitialFiles, None, BaselineRole::InitialFiles).restored(),
            plan(
                TreeSource::Store(name.clone()),
                Some((index(10), SnapshotSource::Automatic)),
                BaselineRole::Periodic(index(10))
            )
            .restored(),
            plan(
                TreeSource::InitialFiles,
                Some((index(10), SnapshotSource::Automatic)),
                BaselineRole::Periodic(index(10))
            )
            .restored(),
            plan(
                TreeSource::Store(update.clone()),
                Some((index(7), SnapshotSource::ManualUpdate)),
                BaselineRole::ManualPromoted
            )
            .restored(),
            plan(
                TreeSource::Store(update.clone()),
                None,
                BaselineRole::ManualPending(manual_head)
            )
            .restored(),
            plan(
                TreeSource::Store(name.clone()),
                Some((index(10), SnapshotSource::SnapshotAssistedAutomatic)),
                BaselineRole::AssistedPromoted
            )
            .restored(),
            plan(
                TreeSource::SourceFiles(SourceRevision::Current),
                None,
                BaselineRole::InitialFiles
            )
            .restored(),
        ],
        [
            None,
            Some((name.clone(), ConfirmedBaseline::Periodic)),
            None,
            Some((update.clone(), ConfirmedBaseline::Authoritative(index(7)))),
            Some((update, ConfirmedBaseline::Authoritative(index(7)))),
            Some((name, ConfirmedBaseline::Authoritative(index(10)))),
            None,
        ]
    );
}

#[test]
async fn a_start_restore_of_a_snapshot_that_the_store_does_not_hold_fails() {
    let (snapshots, shutdown) = enabled_service();
    let agent = agent_snapshots("restore-missing");
    let restore = StartRestore::Store(
        snapshots
            .restore(&agent, &FilesystemSnapshotName::periodic())
            .unwrap(),
    );
    let into = tempfile::tempdir().unwrap();

    assert!(restore.restore(into.path()).await.is_err());
    shut_down(shutdown).await;
}

#[test]
async fn only_a_service_with_a_store_is_enabled() {
    let (enabled, enabled_shutdown) = enabled_service();
    let (disabled, disabled_shutdown) = disabled_service();

    assert_eq!((enabled.is_enabled(), disabled.is_enabled()), (true, false));
    shut_down(enabled_shutdown).await;
    shut_down(disabled_shutdown).await;
}

#[test]
async fn the_shared_slot_takes_a_record_only_from_the_generation_that_runs() {
    let (first, later) = marks();
    let (other, _) = marks();
    let name = FilesystemSnapshotName::periodic();
    let selected_name = selected(Some(&name));
    let slot = SnapshotSlot::default();

    slot.record(Some(&name), later);
    let before_start = slot.since(Some(&selected_name), None);
    let generation_before_start = slot.generation();
    slot.start(first, None);
    let running_generation = slot.generation();
    slot.record(Some(&name), other);
    let of_other_generation = slot.since(Some(&selected_name), None);
    slot.record(Some(&name), later);
    let of_own_generation = slot.since(Some(&selected_name), None);
    let owns = (slot.owns(later), slot.owns(other));
    slot.end();
    let ended = (slot.since(Some(&selected_name), None), slot.owns(later));

    assert_eq!(
        (
            generation_before_start,
            running_generation,
            slot.generation()
        ),
        (None, Some(first), None)
    );
    assert_eq!(before_start, None);
    assert_eq!(of_other_generation, None);
    assert_eq!(of_own_generation, Some(confirmed(&name, later)));
    assert_eq!(owns, (true, false));
    assert_eq!(ended, (None, false));
}

#[test]
async fn a_written_record_of_initial_files_sets_the_slot_without_a_name_only_in_its_generation() {
    let (first, later) = marks();
    let (other_generation, _) = marks();
    let restored = FilesystemSnapshotName::periodic();
    let slot = FilesystemSnapshotSlot::at_start(
        first,
        Some((restored.clone(), ConfirmedBaseline::Periodic)),
    );

    let after_other = confirmed_slot(&slot, None, other_generation);
    let after_own = confirmed_slot(&slot, None, later);

    assert!(after_other.is_none());
    assert_eq!(
        after_own
            .as_ref()
            .and_then(|slot| slot.confirmed().cloned()),
        Some(without_name(later))
    );
}

#[test]
async fn an_unchanged_tree_of_initial_files_writes_a_record_without_a_name_and_confirms_nothing() {
    let (mark, _) = marks();
    let (snapshots, shutdown) = enabled_service();
    let agent = agent_snapshots("periodic-initial-files");
    let mut unchanged = ScriptedHost {
        since: Some(without_name(mark)),
        capture: std::sync::Mutex::new(Some(CaptureOutcome::Unchanged)),
        ..ScriptedHost::new()
    };
    let mut not_written = ScriptedHost {
        capture: std::sync::Mutex::new(Some(CaptureOutcome::InitialFiles { mark })),
        write_fails: true,
        ..ScriptedHost::new()
    };

    let unchanged_result = outcome(&periodic_snapshot(&mut unchanged, &snapshots, &agent).await);
    let not_written_result =
        outcome(&periodic_snapshot(&mut not_written, &snapshots, &agent).await);

    assert_eq!(
        (unchanged_result, unchanged.calls()),
        (
            "Continue".to_string(),
            [
                "snapshot_guest",
                "since",
                "capture(true)",
                "entry(none)",
                "write(none)"
            ]
            .map(String::from)
            .to_vec()
        )
    );
    assert_eq!(
        (not_written_result, not_written.calls()),
        (
            "NotWritten(Write(Payload(\"refused\")))".to_string(),
            [
                "snapshot_guest",
                "since",
                "capture(false)",
                "entry(none)",
                "write(none)"
            ]
            .map(String::from)
            .to_vec()
        )
    );
    shut_down(shutdown).await;
}

#[test]
async fn a_written_periodic_record_that_uploads_its_capture_asks_for_its_confirmation() {
    let (mark, _) = marks();
    let (snapshots, shutdown) = enabled_service();
    let agent = agent_snapshots("periodic-upload");
    let scratch = crate::services::agent_filesystem::scratch_directory().await;
    let mut host = ScriptedHost {
        capture: std::sync::Mutex::new(Some(CaptureOutcome::Captured {
            capture: FilesystemCapture::empty_in(&scratch).await,
            mark,
            detection: ChangeDetection::Full,
        })),
        ..ScriptedHost::new()
    };

    let result = outcome(&periodic_snapshot(&mut host, &snapshots, &agent).await);

    assert_eq!(
        (result, host.calls().last().cloned()),
        ("Continue".to_string(), Some("confirm".to_string()))
    );
    shut_down(shutdown).await;
}

/// A store over an in-memory store that gives each save a time ten minutes after the time of
/// the save before it, so that a retention sees snapshots older than its clock-skew margin,
/// and that reports each deleted name.
#[derive(Default)]
struct SpacedStore {
    memory: crate::filesystem_snapshot::InMemorySnapshotStore,
    times: crate::filesystem_snapshot::SpacedTimes,
    deleted: watch::Sender<Vec<Box<str>>>,
}

#[async_trait::async_trait]
impl crate::filesystem_snapshot::FilesystemSnapshotStore for SpacedStore {
    async fn save(
        &self,
        agent: &AgentSnapshots,
        name: &crate::filesystem_snapshot::SnapshotName,
        tree: &Path,
        parent: Option<(
            &crate::filesystem_snapshot::SnapshotName,
            StoreChangeDetection,
        )>,
        cancel: &tokio_util::sync::CancellationToken,
        slots: &dyn crate::filesystem_snapshot::RunSlots,
    ) -> Result<crate::filesystem_snapshot::SnapshotInfo, crate::filesystem_snapshot::SaveError>
    {
        let info = self
            .memory
            .save(agent, name, tree, parent, cancel, slots)
            .await?;
        Ok(self.times.timed(name, info))
    }

    async fn restore(
        &self,
        agent: &AgentSnapshots,
        name: &crate::filesystem_snapshot::SnapshotName,
        into: &Path,
        slots: &dyn crate::filesystem_snapshot::RunSlots,
    ) -> Result<crate::filesystem_snapshot::SnapshotInfo, crate::filesystem_snapshot::RestoreFailure>
    {
        self.memory.restore(agent, name, into, slots).await
    }

    async fn stat(
        &self,
        agent: &AgentSnapshots,
        name: &crate::filesystem_snapshot::SnapshotName,
    ) -> Result<
        Option<crate::filesystem_snapshot::SnapshotInfo>,
        crate::filesystem_snapshot::ReadError,
    > {
        self.memory.stat(agent, name).await
    }

    async fn list(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn crate::filesystem_snapshot::RunSlots,
    ) -> Result<
        Box<
            [(
                crate::filesystem_snapshot::SnapshotName,
                crate::filesystem_snapshot::SnapshotInfo,
            )],
        >,
        crate::filesystem_snapshot::CallError,
    > {
        Ok(self
            .memory
            .list(agent, slots)
            .await?
            .iter()
            .map(|(name, info)| (name.clone(), self.times.timed(name, *info)))
            .collect())
    }

    async fn delete(
        &self,
        agent: &AgentSnapshots,
        names: &[crate::filesystem_snapshot::SnapshotName],
        slots: &dyn crate::filesystem_snapshot::RunSlots,
    ) -> Result<(), crate::filesystem_snapshot::CallError> {
        self.memory.delete(agent, names, slots).await?;
        self.deleted.send_modify(|deleted| {
            deleted.extend(names.iter().map(|name| Box::from(name.as_str())))
        });
        Ok(())
    }

    async fn delete_all(
        &self,
        agent: &AgentSnapshots,
        slots: &dyn crate::filesystem_snapshot::RunSlots,
    ) -> Result<(), crate::filesystem_snapshot::CallError> {
        self.memory.delete_all(agent, slots).await
    }

    async fn copy_all(
        &self,
        from: &AgentSnapshots,
        to: &AgentSnapshots,
        slots: &dyn crate::filesystem_snapshot::RunSlots,
    ) -> Result<(), crate::filesystem_snapshot::CallError> {
        self.memory.copy_all(from, to, slots).await
    }
}

/// Saves `older` update snapshots, then a manual update whose retention keeps the older
/// snapshots at the indexes `kept`, and gives the names that the retention deleted.
async fn update_retention(older: usize, kept: &[usize]) -> (Vec<Box<str>>, Vec<Box<str>>) {
    update_retention_with(older, kept, &[]).await
}

/// As `update_retention`, with the snapshots `other` saved before the older update snapshots
/// and kept by the retention.
async fn update_retention_with(
    older: usize,
    kept: &[usize],
    other: &[FilesystemSnapshotName],
) -> (Vec<Box<str>>, Vec<Box<str>>) {
    let store = Arc::new(SpacedStore::default());
    let shutdown = crate::services::shutdown::Shutdown::new();
    let snapshots = AgentFilesystemSnapshots::bind(
        &crate::services::golem_config::FilesystemSnapshotsConfig::default(),
        crate::services::agent_filesystem_snapshots::StoreSource::given(
            store.clone(),
            crate::services::golem_config::FilesystemSnapshotUploadConfig::default(),
        ),
        false,
        &shutdown,
    )
    .unwrap();
    let agent = agent_snapshots("update-retention");
    let tree = tempfile::tempdir().unwrap();
    let older = (0..older)
        .map(|_| {
            crate::filesystem_snapshot::SnapshotName::new(FilesystemSnapshotName::update().as_str())
                .unwrap()
        })
        .collect::<Vec<_>>();
    let other_names = other
        .iter()
        .map(|name| crate::filesystem_snapshot::SnapshotName::new(name.as_str()).unwrap())
        .collect::<Vec<_>>();
    let saved = futures::StreamExt::then(
        futures::stream::iter(other_names.iter().chain(&older)),
        |name| {
            crate::filesystem_snapshot::FilesystemSnapshotStore::save(
                store.as_ref(),
                &agent,
                name,
                tree.path(),
                None,
                crate::filesystem_snapshot::never_cancelled(),
                &crate::filesystem_snapshot::Unlimited,
            )
        },
    );
    futures::TryStreamExt::try_collect::<Vec<_>>(saved)
        .await
        .unwrap();
    let (mark, _) = marks();
    let scratch = crate::services::agent_filesystem::scratch_directory().await;
    let mut host = ScriptedHost {
        whole: std::sync::Mutex::new(Some(WholeCapture::Captured {
            capture: FilesystemCapture::empty_in(&scratch).await,
            mark,
        })),
        ..ScriptedHost::new()
    };
    let UpdateSnapshot::Saved {
        retention: Some(retention),
        ..
    } = update_snapshot(&mut host, &snapshots, &agent).await
    else {
        panic!("the manual update saves its snapshot with a retention");
    };
    let mut deleted = store.deleted.subscribe();

    retention.delete_older_snapshots(
        &kept
            .iter()
            .map(|index| {
                older[*index]
                    .as_str()
                    .parse::<FilesystemSnapshotName>()
                    .unwrap()
            })
            .chain(other.iter().cloned())
            .collect::<Box<[_]>>(),
    );
    let deleted = tokio::time::timeout(
        Duration::from_secs(10),
        deleted.wait_for(|deleted| !deleted.is_empty()),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .map(|deleted| deleted.clone())
    .unwrap_or_default();
    shut_down(shutdown).await;
    (
        deleted,
        older.iter().map(|name| Box::from(name.as_str())).collect(),
    )
}

#[test]
async fn the_retention_of_a_saved_manual_update_deletes_the_update_snapshots_beyond_the_kept_count()
{
    let (deleted, older) = update_retention(2, &[]).await;

    assert_eq!(deleted, vec![older[0].clone()]);
}

#[test]
async fn update_retention_keeps_every_successful_update_name() {
    // The oldest name is kept, as the status of a successful update holds it; the next one is
    // beyond the count.
    let (deleted, older) = update_retention(3, &[0]).await;

    assert_eq!(deleted, vec![older[1].clone()]);
}

#[test]
async fn update_retention_keeps_the_record_of_a_successful_assisted_update_and_counts_only_update_snapshots()
 {
    let assisted = FilesystemSnapshotName::periodic();
    let status = golem_common::model::AgentStatusRecord {
        authoritative_snapshot: Some(golem_common::model::AuthoritativeSnapshot {
            index: OplogIndex::from_u64(3),
            kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                filesystem_snapshot: Some(assisted.clone()),
            },
        }),
        ..Default::default()
    };
    let kept = crate::worker::snapshot_selection::names_in_use(&status);

    let (deleted, older) = update_retention_with(2, &[], &kept).await;

    assert_eq!(kept, Box::from([assisted]));
    assert_eq!(deleted, vec![older[0].clone()]);
}

/// A snapshot record with the filesystem snapshot `name`.
fn snapshot_record(name: Option<&FilesystemSnapshotName>) -> OplogEntry {
    OplogEntry::Snapshot {
        timestamp: golem_common::model::Timestamp::from(1_000),
        data: golem_common::model::oplog::OplogPayload::Inline(Box::new(vec![])),
        mime_type: "application/octet-stream".to_string(),
        active_cards: Vec::new(),
        wallet_generation: 0,
        filesystem_snapshot: name.cloned(),
    }
}

/// A snapshot-based update record with the filesystem snapshot `name`.
fn update_record(name: &FilesystemSnapshotName) -> OplogEntry {
    OplogEntry::pending_update(
        UpdateDescription::SnapshotBased {
            target_revision: ComponentRevision::INITIAL,
            payload: golem_common::model::oplog::OplogPayload::Inline(Box::new(vec![])),
            mime_type: "application/octet-stream".to_string(),
            filesystem_snapshot: Some(name.clone()),
        },
        None,
    )
}

/// The strategy entry of a snapshot-assisted update that selected the record at
/// `snapshot` with the filesystem snapshot `name`.
fn assisted_strategy_record(snapshot: u64, name: Option<&FilesystemSnapshotName>) -> OplogEntry {
    OplogEntry::pending_update(
        UpdateDescription::SnapshotAssistedAutomatic {
            target_revision: ComponentRevision::new(2).unwrap(),
            source_component_revision: ComponentRevision::INITIAL,
            source_revision_start_index: OplogIndex::INITIAL,
            snapshot_index: OplogIndex::from_u64(snapshot),
            snapshot_revision: ComponentRevision::INITIAL,
            filesystem_snapshot: name.cloned(),
        },
        Some(OplogIndex::from_u64(snapshot + 1)),
    )
}

fn automatic_admission_record() -> OplogEntry {
    OplogEntry::pending_update(
        UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(2).unwrap(),
        },
        None,
    )
}

fn assisted_success() -> OplogEntry {
    OplogEntry::successful_update(
        ComponentRevision::new(2).unwrap(),
        10,
        None,
        std::collections::HashSet::new(),
        None,
    )
}

fn region(start: u64, end: u64) -> golem_common::model::regions::OplogRegion {
    golem_common::model::regions::OplogRegion {
        start: OplogIndex::from_u64(start),
        end: OplogIndex::from_u64(end),
    }
}

fn oplog(records: Vec<(u64, OplogEntry)>) -> std::collections::BTreeMap<OplogIndex, OplogEntry> {
    records
        .into_iter()
        .map(|(index, entry)| (OplogIndex::from_u64(index), entry))
        .collect()
}

#[test]
fn names_inside_the_dropped_region_are_collected_newest_first() {
    let [p1, p2, p3] = [(); 3].map(|()| FilesystemSnapshotName::periodic());
    let entries = oplog(vec![
        (2, snapshot_record(Some(&p1))),
        (5, snapshot_record(Some(&p2))),
        (6, snapshot_record(None)),
        (8, snapshot_record(Some(&p3))),
    ]);

    assert_eq!(
        super::reverted_snapshot_names(
            &entries,
            &region(4, 9),
            &golem_common::model::regions::DeletedRegions::new()
        ),
        Box::from([p3, p2])
    );
}

#[test]
fn a_snapshot_based_update_record_in_the_region_gives_its_name() {
    let u1 = FilesystemSnapshotName::update();
    let entries = oplog(vec![(5, update_record(&u1))]);

    assert_eq!(
        super::reverted_snapshot_names(
            &entries,
            &region(4, 9),
            &golem_common::model::regions::DeletedRegions::new()
        ),
        Box::from([u1])
    );
}

#[test]
fn a_name_that_a_record_outside_the_region_uses_stays() {
    let [p1, p2] = [(); 2].map(|()| FilesystemSnapshotName::periodic());
    let entries = oplog(vec![
        (2, snapshot_record(Some(&p1))),
        (5, snapshot_record(Some(&p1))),
        (6, snapshot_record(Some(&p2))),
        (7, snapshot_record(Some(&p2))),
    ]);

    assert_eq!(
        super::reverted_snapshot_names(
            &entries,
            &region(4, 9),
            &golem_common::model::regions::DeletedRegions::new()
        ),
        Box::from([p2])
    );
}

#[test]
fn records_in_an_earlier_deleted_region_do_not_keep_a_name() {
    let p1 = FilesystemSnapshotName::periodic();
    let entries = oplog(vec![
        (2, snapshot_record(Some(&p1))),
        (6, snapshot_record(Some(&p1))),
    ]);

    assert_eq!(
        super::reverted_snapshot_names(
            &entries,
            &region(5, 9),
            &golem_common::model::regions::DeletedRegions::from_regions([region(1, 3)])
        ),
        Box::from([p1])
    );
}

#[test]
fn the_strategy_entry_of_an_assisted_update_holds_the_name_of_its_record() {
    let p1 = FilesystemSnapshotName::periodic();

    assert_eq!(
        [
            super::record_name(&assisted_strategy_record(2, Some(&p1))),
            super::record_name(&assisted_strategy_record(2, None)),
        ],
        [Some(&p1), None]
    );
}

#[test]
fn a_revert_of_an_assisted_update_deletes_the_name_of_its_record_only_with_the_record() {
    let [p1, p2] = [(); 2].map(|()| FilesystemSnapshotName::periodic());
    let entries = oplog(vec![
        (2, snapshot_record(Some(&p1))),
        (4, snapshot_record(Some(&p2))),
        (5, automatic_admission_record()),
        (6, assisted_strategy_record(4, Some(&p2))),
        (7, assisted_success()),
    ]);
    let none = golem_common::model::regions::DeletedRegions::new();

    assert_eq!(
        [
            super::reverted_snapshot_names(&entries, &region(8, 9), &none),
            super::reverted_snapshot_names(&entries, &region(7, 9), &none),
            super::reverted_snapshot_names(&entries, &region(6, 9), &none),
            super::reverted_snapshot_names(&entries, &region(3, 9), &none),
            super::reverted_snapshot_names(&entries, &region(2, 9), &none),
        ],
        [
            Box::from([]),
            Box::from([]),
            Box::from([]),
            Box::from([p2.clone()]),
            Box::from([p2, p1]),
        ]
    );
}
