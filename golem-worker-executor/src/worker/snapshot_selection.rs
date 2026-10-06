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

//! The choice of the automatic snapshot entry that a start uses as its baseline.
//!
//! The status keeps two automatic snapshot entries: the last one, and the newest usable one
//! before it. An entry is usable when a `SnapshotConfirmed` entry confirms its filesystem
//! snapshot, or when it has no filesystem snapshot name. A start takes the first of the two that
//! is usable, of the current component revision, not rejected, and not unavailable for this
//! start. When neither is, the start uses the manual-update baseline or a
//! full replay.

use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{FilesystemSnapshotName, OplogIndex};
use golem_common::model::{
    AgentStatusRecord, AutomaticSnapshot, PendingUpdateKind, SnapshotFiles, UsableAutomaticSnapshot,
};
use std::collections::{BTreeSet, HashSet};

/// The automatic snapshot entries that the starts of one agent exclude.
#[derive(Clone, Debug, Default)]
pub(crate) struct SnapshotExclusions {
    /// The entries whose application snapshot did not load or whose replay diverged. A start
    /// never selects them. The start that rejects one persists it for the incarnation after its
    /// fallback succeeds. Each change keeps only the entries that [`kept_rejections`] keeps, so
    /// the set holds at most the two candidates of a start and the entry just rejected.
    rejected: HashSet<OplogIndex>,
    /// The entries whose payload or filesystem snapshot a start could not get. The starts skip
    /// them until a start prepares the agent with success, which clears them.
    unavailable: HashSet<OplogIndex>,
}

impl SnapshotExclusions {
    /// The exclusions with the entry at `index` rejected. The other rejected entries are kept
    /// only while they are candidates of a start of `status`. The entry at `index` stays even
    /// when `status` does not name it, so a start whose status differs cannot select it again
    /// before its next selection prunes the set.
    pub(crate) fn rejecting(self, index: OplogIndex, status: &AgentStatusRecord) -> Self {
        let mut rejected: HashSet<OplogIndex> =
            kept_rejections(status, self.rejected).into_iter().collect();
        rejected.insert(index);
        Self { rejected, ..self }
    }

    /// The exclusions with the rejected entries that storage keeps for the incarnation, for a
    /// start of `status`: only the rejected entries that are candidates of that start stay. A
    /// start of `status` never selects another entry.
    pub(crate) fn with_persisted(
        self,
        persisted: impl IntoIterator<Item = OplogIndex>,
        status: &AgentStatusRecord,
    ) -> Self {
        let rejected = kept_rejections(status, self.rejected.into_iter().chain(persisted))
            .into_iter()
            .collect();
        Self { rejected, ..self }
    }

    /// The exclusions with the entry at `index` unavailable for the current start attempt.
    pub(crate) fn with_unavailable(mut self, index: OplogIndex) -> Self {
        self.unavailable.insert(index);
        self
    }

    /// The exclusions without unavailable entries.
    pub(crate) fn without_unavailable(mut self) -> Self {
        self.unavailable.clear();
        self
    }

    /// The rejected entries to persist, or `None` when no entry is rejected.
    pub(crate) fn persisted_rejections(&self) -> Option<HashSet<OplogIndex>> {
        (!self.rejected.is_empty()).then(|| self.rejected.clone())
    }

    /// The filter of a start of `status`. It excludes the rejected entries, and the unavailable
    /// entries when `unavailable` is true.
    fn filter(
        &self,
        status: &AgentStatusRecord,
        enabled: bool,
        unavailable: bool,
    ) -> AutomaticSnapshotFilter<'_> {
        AutomaticSnapshotFilter {
            has_pending_update: !status.pending_updates.is_empty(),
            rejected: &self.rejected,
            unavailable: unavailable.then_some(&self.unavailable),
            filesystem_snapshots_enabled: enabled,
        }
    }
}

/// The automatic snapshot entry that a start of `status` selects under `exclusions`, as
/// [`StartSelection::of`] gives it, without the rest of the selection. `enabled` tells whether
/// this executor keeps filesystem snapshots.
pub(crate) fn selected_automatic_snapshot(
    status: &AgentStatusRecord,
    exclusions: &SnapshotExclusions,
    enabled: bool,
) -> Option<UsableAutomaticSnapshot> {
    select_automatic_snapshot(status, exclusions.filter(status, enabled, true))
}

/// What a start selects from the status of an agent, under the exclusions of the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartSelection {
    /// The automatic snapshot entry that the start uses as its baseline, or `None` when it uses
    /// the manual-update baseline or a full replay.
    pub(crate) automatic: Option<UsableAutomaticSnapshot>,
    /// The component revision at the start of the replay.
    pub(crate) replay_revision: ComponentRevision,
    /// The component revision at the start of the replay when no entry is unavailable. A check
    /// before a start uses it, because the unavailable entries belong to the starts that could
    /// not get them.
    pub(crate) replay_revision_without_unavailable: ComponentRevision,
    /// The filesystem snapshot name of the last automatic snapshot entry when the entry is not
    /// confirmed and the start would select it if a confirmation record confirmed it. The start
    /// can confirm that name itself when the snapshot is whole in the store.
    pub(crate) candidate: Option<FilesystemSnapshotName>,
}

impl StartSelection {
    /// The selection of a start of `status` under `exclusions`. `enabled` tells whether this
    /// executor keeps filesystem snapshots; without them an entry with a name is not usable. A
    /// pending update ignores the automatic snapshot entries.
    pub(crate) fn of(
        status: &AgentStatusRecord,
        exclusions: &SnapshotExclusions,
        enabled: bool,
    ) -> Self {
        let filter = exclusions.filter(status, enabled, true);
        Self {
            automatic: select_automatic_snapshot(status, filter),
            replay_revision: component_revision_for_replay(status, filter),
            replay_revision_without_unavailable: component_revision_for_replay(
                status,
                exclusions.filter(status, enabled, false),
            ),
            candidate: start_candidate(status, filter),
        }
    }
}

/// What a start excludes when it selects an automatic snapshot entry.
#[derive(Clone, Copy, Debug)]
struct AutomaticSnapshotFilter<'a> {
    /// Whether an update is pending. A pending update ignores the automatic snapshot entries.
    has_pending_update: bool,
    /// The entries whose application snapshot did not load or whose replay diverged.
    rejected: &'a HashSet<OplogIndex>,
    /// The entries whose payload or filesystem snapshot this start could not get, when the filter
    /// excludes them.
    unavailable: Option<&'a HashSet<OplogIndex>>,
    /// Whether this executor restores filesystem snapshots. Without it, an entry with a
    /// filesystem snapshot name is not usable.
    filesystem_snapshots_enabled: bool,
}

/// Gives the automatic snapshot entry that a start uses as its baseline, or `None` when the start
/// uses the manual-update baseline or a full replay.
fn select_automatic_snapshot(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> Option<UsableAutomaticSnapshot> {
    let usable_passes = |usable: &UsableAutomaticSnapshot| {
        passes(
            status,
            filter,
            usable.index,
            usable.component_revision,
            usable.filesystem_snapshot.is_some(),
        )
    };
    status
        .last_automatic_snapshot
        .clone()
        .and_then(AutomaticSnapshot::into_usable)
        .filter(usable_passes)
        .or_else(|| {
            status
                .previous_usable_automatic_snapshot
                .as_ref()
                .filter(|previous| usable_passes(previous))
                .cloned()
        })
}

/// Gives the filesystem snapshot name of the last automatic snapshot record when the record is
/// not confirmed and a start would select it if a confirmation record confirmed it. A start can
/// confirm that name itself when the snapshot is whole in the store.
fn start_candidate(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> Option<FilesystemSnapshotName> {
    status
        .last_automatic_snapshot
        .as_ref()
        .and_then(|last| match &last.files {
            SnapshotFiles::Unconfirmed(name) => Some(name.clone()),
            SnapshotFiles::Unnamed | SnapshotFiles::Confirmed(_) => None,
        })
        .filter(|_| selects_the_last_record_once_confirmed(status, filter))
}

/// Whether a start would select the last automatic snapshot record if a confirmation record
/// confirmed it.
fn selects_the_last_record_once_confirmed(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> bool {
    status.last_automatic_snapshot.as_ref().is_some_and(|last| {
        passes(
            status,
            filter,
            last.index,
            last.component_revision,
            last.files.name().is_some(),
        )
    })
}

/// Whether the usable record at `index` passes `filter` for a start of `status`.
fn passes(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
    index: OplogIndex,
    component_revision: ComponentRevision,
    has_filesystem_snapshot: bool,
) -> bool {
    !filter.has_pending_update
        && component_revision == status.component_revision
        && !filter.rejected.contains(&index)
        && !filter
            .unavailable
            .is_some_and(|unavailable| unavailable.contains(&index))
        && (filter.filesystem_snapshots_enabled || !has_filesystem_snapshot)
}

/// The rejected automatic snapshot entries of `rejected` that are one of the two candidates of a
/// start of `status`, `last_automatic_snapshot` and `previous_usable_automatic_snapshot`. A
/// rejection of any other entry is dropped.
///
/// A revert can make an older entry a candidate again: it drops the region of the newer entries,
/// and the fold from an earlier baseline gives the older entry back. When the rejection of that
/// entry was dropped, a start after a restart of the executor selects it again, fails to load it
/// or diverges from it again, rejects it again, and falls back. The cost is one more failed start
/// attempt, and never a wrong tree or a wrong result, because both causes of a rejection are found
/// at run time. The bound accepts that cost so that the stored set holds at most two entries and
/// does not grow with the life of the agent.
pub(crate) fn kept_rejections(
    status: &AgentStatusRecord,
    rejected: impl IntoIterator<Item = OplogIndex>,
) -> BTreeSet<OplogIndex> {
    let candidates = start_candidates(status).map(|candidate| candidate.map(|(index, _)| index));
    rejected
        .into_iter()
        .filter(|index| candidates.contains(&Some(*index)))
        .collect()
}

/// The two automatic snapshot records that a start can select: the last one and the newest
/// usable one before it, with their filesystem snapshot names. The result is not filtered by
/// revision, by pending update or by exclusion. [`kept_rejections`] uses it too.
pub(crate) fn start_candidates(
    status: &AgentStatusRecord,
) -> [Option<(OplogIndex, Option<&FilesystemSnapshotName>)>; 2] {
    [
        status
            .last_automatic_snapshot
            .as_ref()
            .map(|last| (last.index, last.files.name())),
        status
            .previous_usable_automatic_snapshot
            .as_ref()
            .map(|previous| (previous.index, previous.filesystem_snapshot.as_ref())),
    ]
}

/// The filesystem snapshot names of the two automatic snapshot records that a start can select,
/// as [`start_candidates`] gives them, the last record first. Retention keeps them whatever their
/// age.
pub(crate) fn selectable_names(status: &AgentStatusRecord) -> Box<[FilesystemSnapshotName]> {
    start_candidates(status)
        .into_iter()
        .flatten()
        .filter_map(|(_, name)| name.cloned())
        .collect()
}

/// The update snapshot names that a valid cut of the agent can still make a baseline: the names
/// of the successful updates and of the pending updates in the status. A revert rebuilds the
/// status, so the names of updates in its dropped region are not in it.
pub(crate) fn update_names_in_use(status: &AgentStatusRecord) -> Box<[FilesystemSnapshotName]> {
    status
        .successful_updates
        .iter()
        .filter_map(|update| update.filesystem_snapshot.clone())
        .chain(
            status
                .pending_updates
                .iter()
                .filter_map(|update| update.kind.filesystem_snapshot().cloned()),
        )
        .collect()
}

/// Gives the component revision at the start of the replay: the revision of the selected
/// automatic snapshot entry, else the target of a pending snapshot-based update, else the revision
/// of the manual-update baseline.
fn component_revision_for_replay(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> ComponentRevision {
    select_automatic_snapshot(status, filter).map_or_else(
        || {
            status
                .pending_updates
                .front()
                .and_then(|update| match update.kind {
                    PendingUpdateKind::SnapshotBased { .. } => Some(update.target_revision),
                    PendingUpdateKind::Automatic => None,
                })
                .unwrap_or(status.component_revision_for_replay)
        },
        |snapshot| snapshot.component_revision,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::PendingUpdateRef;
    use golem_common::model::SuccessfulUpdateRecord;
    use golem_common::model::Timestamp;
    use golem_common::model::oplog::FilesystemSnapshotName;
    use test_r::test;

    fn revision(value: u64) -> ComponentRevision {
        ComponentRevision::new(value).unwrap()
    }

    /// A status whose last automatic snapshot entry is at index 10 with `last`, confirmed when
    /// `confirmed`, and whose previous usable entry is at index 5 with `previous`.
    fn status(
        last: Option<FilesystemSnapshotName>,
        confirmed: bool,
        previous: Option<Option<FilesystemSnapshotName>>,
    ) -> AgentStatusRecord {
        AgentStatusRecord {
            component_revision: revision(2),
            component_revision_for_replay: revision(1),
            last_automatic_snapshot: Some(AutomaticSnapshot {
                index: OplogIndex::from_u64(10),
                timestamp: Timestamp::from(1_000),
                component_revision: revision(2),
                files: match (last, confirmed) {
                    (Some(name), true) => SnapshotFiles::Confirmed(name),
                    (last, _) => SnapshotFiles::named(last),
                },
            }),
            previous_usable_automatic_snapshot: previous.map(|filesystem_snapshot| {
                UsableAutomaticSnapshot {
                    index: OplogIndex::from_u64(5),
                    component_revision: revision(2),
                    filesystem_snapshot,
                }
            }),
            ..Default::default()
        }
    }

    /// A filter without a pending update and with filesystem snapshots on. The empty sets
    /// live in statics, so each test can take one.
    fn filter(unavailable: &HashSet<OplogIndex>) -> AutomaticSnapshotFilter<'_> {
        static NONE_REJECTED: std::sync::LazyLock<HashSet<OplogIndex>> =
            std::sync::LazyLock::new(HashSet::new);
        AutomaticSnapshotFilter {
            has_pending_update: false,
            rejected: &NONE_REJECTED,
            unavailable: Some(unavailable),
            filesystem_snapshots_enabled: true,
        }
    }

    fn selected_index(
        status: &AgentStatusRecord,
        filter: AutomaticSnapshotFilter<'_>,
    ) -> Option<u64> {
        select_automatic_snapshot(status, filter).map(|snapshot| u64::from(snapshot.index))
    }

    #[test]
    fn an_unconfirmed_last_entry_is_selected_once_confirmed_only_when_it_passes_the_filter() {
        let status = status(Some(FilesystemSnapshotName::periodic()), false, Some(None));
        let unavailable = HashSet::from([OplogIndex::from_u64(10)]);
        let mut older_revision = status.clone();
        if let Some(last) = older_revision.last_automatic_snapshot.as_mut() {
            last.component_revision = revision(1);
        }

        let cases = (
            selects_the_last_record_once_confirmed(&status, filter(&HashSet::new())),
            selects_the_last_record_once_confirmed(&status, filter(&unavailable)),
            selects_the_last_record_once_confirmed(&older_revision, filter(&HashSet::new())),
            selects_the_last_record_once_confirmed(
                &AgentStatusRecord::default(),
                filter(&HashSet::new()),
            ),
        );

        assert_eq!(cases, (true, false, false, false));
    }

    #[test]
    fn a_confirmed_last_entry_is_selected() {
        let name = FilesystemSnapshotName::periodic();
        let status = status(Some(name.clone()), true, Some(None));

        let selected = select_automatic_snapshot(&status, filter(&HashSet::new()));

        assert_eq!(
            selected,
            Some(UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(10),
                component_revision: revision(2),
                filesystem_snapshot: Some(name),
            })
        );
    }

    #[test]
    fn a_last_entry_without_a_name_is_usable_without_a_confirmation() {
        let status = status(None, false, None);

        assert_eq!(selected_index(&status, filter(&HashSet::new())), Some(10));
    }

    #[test]
    fn an_unconfirmed_last_entry_falls_back_to_the_previous_usable_entry() {
        let previous = FilesystemSnapshotName::periodic();
        let status = status(
            Some(FilesystemSnapshotName::periodic()),
            false,
            Some(Some(previous.clone())),
        );

        let selected = select_automatic_snapshot(&status, filter(&HashSet::new()));

        assert_eq!(
            selected.map(|snapshot| (u64::from(snapshot.index), snapshot.filesystem_snapshot)),
            Some((5, Some(previous)))
        );
    }

    #[test]
    fn an_unconfirmed_last_entry_without_a_previous_entry_selects_nothing() {
        let status = status(Some(FilesystemSnapshotName::periodic()), false, None);

        assert_eq!(selected_index(&status, filter(&HashSet::new())), None);
        assert_eq!(
            component_revision_for_replay(&status, filter(&HashSet::new())),
            revision(1)
        );
    }

    #[test]
    fn an_unavailable_last_entry_falls_back_to_the_previous_entry_and_then_to_nothing() {
        let status = status(
            Some(FilesystemSnapshotName::periodic()),
            true,
            Some(Some(FilesystemSnapshotName::periodic())),
        );
        let last = HashSet::from([OplogIndex::from_u64(10)]);
        let both = HashSet::from([OplogIndex::from_u64(10), OplogIndex::from_u64(5)]);

        assert_eq!(
            (
                selected_index(&status, filter(&last)),
                selected_index(&status, filter(&both))
            ),
            (Some(5), None)
        );
    }

    #[test]
    fn a_rejected_last_entry_leaves_the_previous_entry_selectable() {
        let status = status(
            Some(FilesystemSnapshotName::periodic()),
            true,
            Some(Some(FilesystemSnapshotName::periodic())),
        );
        let unavailable = HashSet::new();
        let last = HashSet::from([OplogIndex::from_u64(10)]);
        let both = HashSet::from([OplogIndex::from_u64(10), OplogIndex::from_u64(5)]);
        let rejecting = |rejected| AutomaticSnapshotFilter {
            rejected,
            ..filter(&unavailable)
        };

        assert_eq!(
            (
                selected_index(&status, rejecting(&last)),
                selected_index(&status, rejecting(&both)),
            ),
            (Some(5), None)
        );
        assert_eq!(
            component_revision_for_replay(&status, rejecting(&both)),
            revision(1)
        );
    }

    #[test]
    fn an_entry_with_a_name_is_not_usable_when_snapshots_are_disabled() {
        let named = status(
            Some(FilesystemSnapshotName::periodic()),
            true,
            Some(Some(FilesystemSnapshotName::periodic())),
        );
        let named_then_nameless =
            status(Some(FilesystemSnapshotName::periodic()), true, Some(None));
        let unavailable = HashSet::new();
        let disabled = AutomaticSnapshotFilter {
            filesystem_snapshots_enabled: false,
            ..filter(&unavailable)
        };

        assert_eq!(
            (
                selected_index(&named, disabled),
                selected_index(&named_then_nameless, disabled)
            ),
            (None, Some(5))
        );
    }

    #[test]
    fn an_entry_of_another_revision_or_a_pending_update_selects_nothing() {
        let mut old_revision = status(None, false, Some(None));
        old_revision.component_revision = revision(3);
        let pending = status(None, false, None);
        let unavailable = HashSet::new();

        assert_eq!(selected_index(&old_revision, filter(&unavailable)), None);
        assert_eq!(
            selected_index(
                &pending,
                AutomaticSnapshotFilter {
                    has_pending_update: true,
                    ..filter(&unavailable)
                }
            ),
            None
        );
    }

    #[test]
    fn a_pending_snapshot_based_update_gives_its_target_revision_for_replay() {
        let mut status = status(None, false, None);
        status.pending_updates.push_back(PendingUpdateRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(11),
            target_revision: revision(4),
            kind: PendingUpdateKind::SnapshotBased {
                filesystem_snapshot: None,
            },
        });
        let unavailable = HashSet::new();

        assert_eq!(
            component_revision_for_replay(
                &status,
                AutomaticSnapshotFilter {
                    has_pending_update: true,
                    ..filter(&unavailable)
                }
            ),
            revision(4)
        );
    }

    #[test]
    fn a_start_candidate_is_the_unconfirmed_named_last_entry_that_a_start_would_select() {
        let name = FilesystemSnapshotName::periodic();
        let unconfirmed = status(Some(name.clone()), false, Some(None));
        let confirmed = status(Some(name.clone()), true, Some(None));
        let nameless = status(None, false, Some(None));
        let unavailable = HashSet::from([OplogIndex::from_u64(10)]);

        assert_eq!(
            [
                start_candidate(&unconfirmed, filter(&HashSet::new())),
                start_candidate(&confirmed, filter(&HashSet::new())),
                start_candidate(&nameless, filter(&HashSet::new())),
                start_candidate(&unconfirmed, filter(&unavailable)),
            ],
            [Some(name), None, None, None]
        );
    }

    #[test]
    fn the_start_candidates_are_the_last_and_the_previous_usable_record() {
        let last = FilesystemSnapshotName::periodic();
        let previous = FilesystemSnapshotName::periodic();
        let both = status(Some(last.clone()), false, Some(Some(previous.clone())));
        let nameless = status(None, true, Some(None));
        let last_only = status(Some(last.clone()), true, None);

        assert_eq!(
            [
                start_candidates(&both),
                start_candidates(&nameless),
                start_candidates(&last_only),
                start_candidates(&AgentStatusRecord::default()),
            ],
            [
                [
                    Some((OplogIndex::from_u64(10), Some(&last))),
                    Some((OplogIndex::from_u64(5), Some(&previous)))
                ],
                [
                    Some((OplogIndex::from_u64(10), None)),
                    Some((OplogIndex::from_u64(5), None))
                ],
                [Some((OplogIndex::from_u64(10), Some(&last))), None],
                [None, None],
            ]
        );
    }

    fn successful_update(
        index: u64,
        filesystem_snapshot: Option<FilesystemSnapshotName>,
    ) -> SuccessfulUpdateRecord {
        SuccessfulUpdateRecord {
            timestamp: Timestamp::from(1_000),
            target_revision: revision(2),
            oplog_index: OplogIndex::from_u64(index),
            filesystem_snapshot,
        }
    }

    fn pending_update(index: u64, kind: PendingUpdateKind) -> PendingUpdateRef {
        PendingUpdateRef {
            timestamp: Timestamp::from(1_000),
            oplog_index: OplogIndex::from_u64(index),
            target_revision: revision(3),
            kind,
        }
    }

    #[test]
    fn update_names_in_use_are_the_successful_and_pending_update_names() {
        let first = FilesystemSnapshotName::update();
        let second = FilesystemSnapshotName::update();
        let pending = FilesystemSnapshotName::update();
        let status = AgentStatusRecord {
            successful_updates: vec![
                successful_update(3, Some(first.clone())),
                successful_update(5, None),
                successful_update(7, None),
                successful_update(9, Some(second.clone())),
            ],
            pending_updates: [
                pending_update(10, PendingUpdateKind::Automatic),
                pending_update(
                    11,
                    PendingUpdateKind::SnapshotBased {
                        filesystem_snapshot: Some(pending.clone()),
                    },
                ),
                pending_update(
                    12,
                    PendingUpdateKind::SnapshotBased {
                        filesystem_snapshot: None,
                    },
                ),
            ]
            .into(),
            ..Default::default()
        };

        assert_eq!(
            [
                update_names_in_use(&status),
                update_names_in_use(&AgentStatusRecord::default())
            ],
            [Box::from([first, second, pending]), Box::from([])]
        );
    }

    fn indexes(indexes: &[u64]) -> BTreeSet<OplogIndex> {
        indexes.iter().copied().map(OplogIndex::from_u64).collect()
    }

    #[test]
    fn only_the_candidates_of_a_start_stay_rejected() {
        let both = status(Some(FilesystemSnapshotName::periodic()), true, Some(None));
        let last_only = status(Some(FilesystemSnapshotName::periodic()), true, None);

        assert_eq!(
            [
                kept_rejections(&both, indexes(&[3, 5, 10, 11])),
                kept_rejections(&last_only, indexes(&[5, 10])),
                kept_rejections(&AgentStatusRecord::default(), indexes(&[5, 10])),
            ],
            [indexes(&[5, 10]), indexes(&[10]), indexes(&[])]
        );
    }

    #[test]
    fn a_new_snapshot_entry_moves_the_candidates_and_an_older_rejection_drops_out() {
        let name = FilesystemSnapshotName::periodic();
        let before = status(Some(name.clone()), true, Some(None));
        let rejected = kept_rejections(&before, indexes(&[5, 10]));
        let after = AgentStatusRecord {
            last_automatic_snapshot: Some(AutomaticSnapshot {
                index: OplogIndex::from_u64(20),
                timestamp: Timestamp::from(2_000),
                component_revision: revision(2),
                files: SnapshotFiles::Unconfirmed(FilesystemSnapshotName::periodic()),
            }),
            previous_usable_automatic_snapshot: Some(UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(10),
                component_revision: revision(2),
                filesystem_snapshot: Some(name),
            }),
            ..before.clone()
        };

        assert_eq!(rejected, indexes(&[5, 10]));
        assert_eq!(kept_rejections(&after, rejected), indexes(&[10]));
    }

    #[test]
    fn the_rejections_in_memory_keep_at_most_the_candidates_after_a_new_snapshot_entry() {
        let name = FilesystemSnapshotName::periodic();
        let before = status(Some(name.clone()), true, Some(None));
        let after = AgentStatusRecord {
            last_automatic_snapshot: Some(AutomaticSnapshot {
                index: OplogIndex::from_u64(20),
                timestamp: Timestamp::from(2_000),
                component_revision: revision(2),
                files: SnapshotFiles::Unconfirmed(FilesystemSnapshotName::periodic()),
            }),
            previous_usable_automatic_snapshot: Some(UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(10),
                component_revision: revision(2),
                filesystem_snapshot: Some(name),
            }),
            ..before.clone()
        };
        let rejected_before = SnapshotExclusions::default()
            .rejecting(OplogIndex::from_u64(3), &before)
            .rejecting(OplogIndex::from_u64(5), &before)
            .rejecting(OplogIndex::from_u64(10), &before);

        let persisted_after = rejected_before
            .clone()
            .with_persisted([OplogIndex::from_u64(1)], &after);
        let rejected_after = rejected_before.rejecting(OplogIndex::from_u64(20), &after);

        assert_eq!(
            persisted_after.persisted_rejections(),
            Some(HashSet::from([OplogIndex::from_u64(10)]))
        );
        assert_eq!(
            rejected_after.persisted_rejections(),
            Some(HashSet::from([
                OplogIndex::from_u64(10),
                OplogIndex::from_u64(20)
            ]))
        );
    }

    #[test]
    fn a_rejected_record_that_reuses_a_name_falls_back_to_the_record_before_it() {
        let name = FilesystemSnapshotName::periodic();
        let shared = status(Some(name.clone()), true, None);
        let reused = AgentStatusRecord {
            last_automatic_snapshot: Some(AutomaticSnapshot {
                index: OplogIndex::from_u64(20),
                timestamp: Timestamp::from(2_000),
                component_revision: revision(2),
                files: SnapshotFiles::Confirmed(name.clone()),
            }),
            previous_usable_automatic_snapshot: Some(UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(10),
                component_revision: revision(2),
                filesystem_snapshot: Some(name.clone()),
            }),
            ..shared
        };
        let rejected = SnapshotExclusions::default().rejecting(OplogIndex::from_u64(20), &reused);

        let selected = StartSelection::of(&reused, &rejected, true).automatic;

        assert_eq!(
            selected,
            Some(UsableAutomaticSnapshot {
                index: OplogIndex::from_u64(10),
                component_revision: revision(2),
                filesystem_snapshot: Some(name),
            })
        );
    }

    fn selection_index(selection: &StartSelection) -> Option<u64> {
        selection
            .automatic
            .as_ref()
            .map(|snapshot| u64::from(snapshot.index))
    }

    #[test]
    fn a_start_selection_skips_unavailable_entries_only_for_the_start() {
        let mut status = status(
            Some(FilesystemSnapshotName::periodic()),
            true,
            Some(Some(FilesystemSnapshotName::periodic())),
        );
        status.previous_usable_automatic_snapshot = Some(UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(5),
            component_revision: revision(2),
            filesystem_snapshot: Some(FilesystemSnapshotName::periodic()),
        });
        let exclusions = SnapshotExclusions::default();
        assert_eq!(exclusions.persisted_rejections(), None);

        let exclusions = exclusions
            .with_unavailable(OplogIndex::from_u64(10))
            .with_unavailable(OplogIndex::from_u64(5));
        let unavailable = StartSelection::of(&status, &exclusions, true);
        let exclusions = exclusions.without_unavailable();
        let cleared = selection_index(&StartSelection::of(&status, &exclusions, true));
        let exclusions = exclusions
            .rejecting(OplogIndex::from_u64(10), &status)
            .with_persisted([OplogIndex::from_u64(5)], &status);
        let rejected = StartSelection::of(&status, &exclusions, true);

        assert_eq!(
            (
                selection_index(&unavailable),
                unavailable.replay_revision,
                unavailable.replay_revision_without_unavailable
            ),
            (None, revision(1), revision(2))
        );
        assert_eq!(cleared, Some(10));
        assert_eq!(
            (
                selection_index(&rejected),
                rejected.replay_revision,
                rejected.replay_revision_without_unavailable
            ),
            (None, revision(1), revision(1))
        );
        assert_eq!(
            exclusions.persisted_rejections(),
            Some(HashSet::from([
                OplogIndex::from_u64(10),
                OplogIndex::from_u64(5)
            ]))
        );
    }

    #[test]
    fn clearing_the_unavailable_entries_keeps_the_rejected_ones() {
        let status = status(
            Some(FilesystemSnapshotName::periodic()),
            true,
            Some(Some(FilesystemSnapshotName::periodic())),
        );

        let exclusions = SnapshotExclusions::default()
            .rejecting(OplogIndex::from_u64(10), &status)
            .with_unavailable(OplogIndex::from_u64(5))
            .without_unavailable();

        assert_eq!(
            exclusions.persisted_rejections(),
            Some(HashSet::from([OplogIndex::from_u64(10)]))
        );
    }

    #[test]
    fn a_start_selection_follows_a_pending_update_and_the_enabled_flag() {
        let name = FilesystemSnapshotName::periodic();
        let confirmed = status(Some(name.clone()), true, None);
        let unconfirmed = status(Some(name.clone()), false, None);
        let mut pending = confirmed.clone();
        pending.pending_updates.push_back(PendingUpdateRef {
            timestamp: Timestamp::now_utc(),
            oplog_index: OplogIndex::from_u64(11),
            target_revision: revision(4),
            kind: PendingUpdateKind::Automatic,
        });
        let none = SnapshotExclusions::default();

        assert_eq!(
            [
                selection_index(&StartSelection::of(&confirmed, &none, true)),
                selection_index(&StartSelection::of(&confirmed, &none, false)),
                selection_index(&StartSelection::of(&pending, &none, true)),
            ],
            [Some(10), None, None]
        );
        assert_eq!(
            [
                StartSelection::of(&unconfirmed, &none, true).candidate,
                StartSelection::of(&unconfirmed, &none, false).candidate,
                StartSelection::of(&confirmed, &none, true).candidate,
            ],
            [Some(name), None, None]
        );
    }
}
