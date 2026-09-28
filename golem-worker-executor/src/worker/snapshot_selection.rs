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
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentStatusRecord, PendingUpdateKind, UsableAutomaticSnapshot};
use std::collections::HashSet;

/// What a start excludes when it selects an automatic snapshot entry.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AutomaticSnapshotFilter<'a> {
    /// Whether an update is pending. A pending update ignores the automatic snapshot entries.
    pub(crate) has_pending_update: bool,
    /// The entries whose application snapshot did not load or whose replay diverged.
    pub(crate) rejected: &'a HashSet<OplogIndex>,
    /// The entries whose payload or filesystem snapshot this start could not get.
    pub(crate) unavailable: &'a HashSet<OplogIndex>,
    /// Whether this executor restores filesystem snapshots. Without it, an entry with a
    /// filesystem snapshot name is not usable.
    pub(crate) filesystem_snapshots_enabled: bool,
}

/// Gives the automatic snapshot entry that a start uses as its baseline, or `None` when the start
/// uses the manual-update baseline or a full replay.
pub(crate) fn select_automatic_snapshot(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> Option<UsableAutomaticSnapshot> {
    let last = status
        .last_automatic_snapshot_index
        .zip(status.last_automatic_snapshot_component_revision)
        .filter(|_| {
            status.last_automatic_snapshot_confirmed
                || status.last_automatic_snapshot_filesystem_snapshot.is_none()
        })
        .map(|(index, component_revision)| UsableAutomaticSnapshot {
            index,
            component_revision,
            filesystem_snapshot: status.last_automatic_snapshot_filesystem_snapshot.clone(),
        });
    last.into_iter()
        .chain(status.previous_usable_automatic_snapshot.clone())
        .find(|snapshot| {
            passes(
                status,
                filter,
                snapshot.index,
                snapshot.component_revision,
                snapshot.filesystem_snapshot.is_some(),
            )
        })
}

/// Whether a start would select the last automatic snapshot record if a confirmation record
/// confirmed it.
pub(crate) fn selects_the_last_record_once_confirmed(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> bool {
    status
        .last_automatic_snapshot_index
        .zip(status.last_automatic_snapshot_component_revision)
        .is_some_and(|(index, component_revision)| {
            passes(
                status,
                filter,
                index,
                component_revision,
                status.last_automatic_snapshot_filesystem_snapshot.is_some(),
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
        && !filter.unavailable.contains(&index)
        && (filter.filesystem_snapshots_enabled || !has_filesystem_snapshot)
}

/// Gives the component revision at the start of the replay: the revision of the selected
/// automatic snapshot entry, else the target of a pending snapshot-based update, else the revision
/// of the manual-update baseline.
pub(crate) fn component_revision_for_replay(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> ComponentRevision {
    select_automatic_snapshot(status, filter).map_or_else(
        || {
            status
                .pending_updates
                .front()
                .and_then(|update| match update.kind {
                    PendingUpdateKind::SnapshotBased => Some(update.target_revision),
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
            last_automatic_snapshot_index: Some(OplogIndex::from_u64(10)),
            last_automatic_snapshot_component_revision: Some(revision(2)),
            last_automatic_snapshot_filesystem_snapshot: last,
            last_automatic_snapshot_confirmed: confirmed,
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
            unavailable,
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
        let older_revision = AgentStatusRecord {
            last_automatic_snapshot_component_revision: Some(revision(1)),
            ..status.clone()
        };

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
            kind: PendingUpdateKind::SnapshotBased,
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
}
