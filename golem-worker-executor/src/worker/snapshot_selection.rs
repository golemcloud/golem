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

//! The start decision of an agent: what a start does with the head of its update queue, and the
//! record whose filesystem snapshot and application snapshot the start uses as its baseline.
//!
//! The status keeps two automatic snapshot records: the last one, and the newest usable one
//! before it. A record is usable when a `SnapshotConfirmed` entry confirms its filesystem
//! snapshot, or when it has no filesystem snapshot name. A start without a pending update takes
//! the first of the two that is usable, of the current component revision, not rejected, and not
//! unavailable for this start. An automatic update at the head of the queue that has no strategy
//! yet takes a record by the same rules, and its strategy entry freezes that choice. When no
//! record is taken, the start uses the authoritative baseline or a full replay.

use crate::worker::start_outcome::BaselineRole;
use crate::worker::status::update_queue::is_unselected_automatic;
use golem_common::model::component::ComponentRevision;
use golem_common::model::oplog::{
    FilesystemSnapshotName, OplogIndex, SnapshotFault, UpdateDescription,
};
use golem_common::model::{
    AgentStatusRecord, AssistedSelection, AuthoritativeSnapshot, AuthoritativeSnapshotKind,
    AutomaticSnapshot, PendingUpdateKind, PendingUpdateRef, SnapshotFiles, UsableAutomaticSnapshot,
};
use std::collections::{BTreeSet, HashSet};

/// The automatic snapshot entries that the starts of one agent exclude.
#[derive(Clone, Debug, Default)]
pub(crate) struct SnapshotExclusions {
    /// The entries whose application snapshot did not load or whose replay diverged, or whose
    /// filesystem snapshot the store lost in a snapshot-assisted update. A start never selects
    /// them. The start that rejects one persists it for the incarnation after its fallback
    /// succeeds. Each change keeps only the entries that [`kept_rejections`] keeps, so the set
    /// holds at most the two candidates of a start and the entry just rejected.
    rejected: HashSet<OplogIndex>,
    /// The entries whose payload or filesystem snapshot a start could not get. The starts skip
    /// them until a start prepares the agent with success, or until a new startup attempt
    /// begins. Either clears them.
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
            queue: QueueFilter::of(status),
            rejected: &self.rejected,
            unavailable: unavailable.then_some(&self.unavailable),
            filesystem_snapshots_enabled: enabled,
        }
    }
}

/// What a start does first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartDecision {
    /// The head of the queue is an automatic update without a strategy. The start writes
    /// `PendingUpdate { description, update_attempt_index: Some(admission_index) }`, reads the
    /// status again and decides again.
    PersistStrategy {
        description: UpdateDescription,
        admission_index: OplogIndex,
    },
    /// The head of the queue is a snapshot-assisted update whose source does not hold any more:
    /// `found` is what the status has. The start fails the update from `role`, the baseline of
    /// that update, and decides again.
    FailHead {
        role: BaselineRole,
        found: SourceFound,
    },
    /// The start uses this selection.
    Start(StartSelection),
}

/// The source revision of an agent as its status has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SourceFound {
    pub(crate) revision: ComponentRevision,
    pub(crate) start_index: OplogIndex,
}

/// The decision of a start of `status` under `exclusions`. `enabled` tells whether this executor
/// keeps filesystem snapshots.
///
/// A head with a frozen snapshot-assisted selection reads only its kind and the source of the
/// status: the exclusions, the candidates and `enabled` do not change it.
pub(crate) fn decide_start(
    status: &AgentStatusRecord,
    exclusions: &SnapshotExclusions,
    enabled: bool,
) -> StartDecision {
    match Head::of(status) {
        Head::UnselectedAutomatic(head) => StartDecision::PersistStrategy {
            description: strategy(
                status,
                head,
                select_automatic_snapshot(status, exclusions.filter(status, enabled, true)),
            ),
            admission_index: head.admission_index,
        },
        Head::SelectedAssisted(head, _) => match stale_assisted_head(status, head) {
            Some(found) => StartDecision::FailHead {
                role: BaselineRole::AssistedPending(Box::new(head.clone())),
                found,
            },
            None => StartDecision::Start(StartSelection::of(status, exclusions, enabled)),
        },
        Head::None | Head::Other(_) => {
            StartDecision::Start(StartSelection::of(status, exclusions, enabled))
        }
    }
}

/// What the status has when `head` is a snapshot-assisted update whose source does not hold in
/// `status` any more: the revision or the start of the revision changed, or the target is not
/// newer than the source. `None` for any other head.
pub(crate) fn stale_assisted_head(
    status: &AgentStatusRecord,
    head: &PendingUpdateRef,
) -> Option<SourceFound> {
    let PendingUpdateKind::SnapshotAssistedAutomatic(selection) = &head.kind else {
        return None;
    };
    (status.component_revision != selection.snapshot.component_revision
        || status.component_revision_start_index != selection.source_revision_start_index
        || head.target_revision <= selection.snapshot.component_revision)
        .then_some(SourceFound {
            revision: status.component_revision,
            start_index: status.component_revision_start_index,
        })
}

/// The pending update whose target a start of `status` instantiates: the first update of the
/// queue whose source holds. A start fails a snapshot-assisted head whose source does not hold
/// before it instantiates anything, and the update after it becomes the head.
pub(crate) fn active_head(status: &AgentStatusRecord) -> Option<&PendingUpdateRef> {
    status
        .pending_updates
        .iter()
        .find(|update| stale_assisted_head(status, update).is_none())
}

/// The filesystem snapshot whose upload a loaded agent waits for before it ends its generation
/// to start the automatic update at the head of its queue: the candidate of a start of `status`
/// under the exclusions that `exclusions` gives, while that head has no strategy entry. A start
/// of an unloaded agent waits for the same upload before it takes its permits. `enabled` tells
/// whether this executor keeps filesystem snapshots. The call reads the exclusions only when the
/// head has no strategy entry and the last automatic snapshot record is not confirmed.
pub(crate) fn upload_before_an_automatic_update<Exclusions>(
    status: &AgentStatusRecord,
    enabled: bool,
    exclusions: impl FnOnce() -> Exclusions,
) -> Option<FilesystemSnapshotName>
where
    Exclusions: std::ops::Deref<Target = SnapshotExclusions>,
{
    let Head::UnselectedAutomatic(_) = Head::of(status) else {
        return None;
    };
    let name = status
        .last_automatic_snapshot
        .as_ref()
        .and_then(|last| match &last.files {
            SnapshotFiles::Unconfirmed(name) => Some(name),
            SnapshotFiles::Unnamed | SnapshotFiles::Confirmed(_) => None,
        })?;
    selects_the_last_record_once_confirmed(status, exclusions().filter(status, enabled, true))
        .then(|| name.clone())
}

/// The strategy entry of the unselected automatic update `head`: a snapshot-assisted update from
/// `selected`, or a plain automatic update, which replays the whole history on the target.
fn strategy(
    status: &AgentStatusRecord,
    head: &PendingUpdateRef,
    selected: Option<UsableAutomaticSnapshot>,
) -> UpdateDescription {
    match selected {
        Some(snapshot) => UpdateDescription::SnapshotAssistedAutomatic {
            target_revision: head.target_revision,
            source_component_revision: status.component_revision,
            source_revision_start_index: status.component_revision_start_index,
            snapshot_index: snapshot.index,
            snapshot_revision: snapshot.component_revision,
            filesystem_snapshot: snapshot.filesystem_snapshot,
        },
        None => UpdateDescription::Automatic {
            target_revision: head.target_revision,
        },
    }
}

/// What a start selects from the status of an agent, under the exclusions of the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartSelection {
    /// The record that gives the start its filesystem and its application snapshot.
    pub(crate) baseline: SelectedBaseline,
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
    /// executor keeps filesystem snapshots; without them an entry with a name is not usable.
    ///
    /// For an automatic update at the head of the queue without a strategy, the selection is the
    /// one that its strategy entry freezes.
    pub(crate) fn of(
        status: &AgentStatusRecord,
        exclusions: &SnapshotExclusions,
        enabled: bool,
    ) -> Self {
        let filter = exclusions.filter(status, enabled, true);
        let baseline = selected_baseline(status, filter);
        Self {
            replay_revision: replay_revision(status, &baseline),
            replay_revision_without_unavailable: replay_revision(
                status,
                &selected_baseline(status, exclusions.filter(status, enabled, false)),
            ),
            baseline,
            candidate: start_candidate(status, filter),
        }
    }
}

/// The record whose filesystem snapshot and application snapshot a start uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectedBaseline {
    /// A periodic record, for a start without a pending update.
    Periodic(UsableAutomaticSnapshot),
    /// The record that the snapshot-assisted update at the head of the queue selected.
    AssistedPending {
        snapshot: UsableAutomaticSnapshot,
        head: Box<PendingUpdateRef>,
    },
    /// The authoritative baseline of a successful snapshot-assisted update: the `Snapshot` entry
    /// at `index` and its filesystem snapshot. Its replay revision is the source revision of the
    /// update.
    AssistedPromoted {
        index: OplogIndex,
        name: Option<FilesystemSnapshotName>,
    },
    /// The snapshot-based manual update at the head of the queue, whose record is its
    /// `PendingUpdate` entry. `previous` is the authoritative baseline before it.
    ManualPending {
        head: Box<PendingUpdateRef>,
        previous: Option<AuthoritativeSnapshot>,
    },
    /// The authoritative baseline of a successful snapshot-based manual update: its
    /// `PendingUpdate` entry at `index`.
    ManualPromoted { index: OplogIndex },
    /// No record: the initial files of the replay revision.
    InitialFiles,
}

impl SelectedBaseline {
    /// The periodic record of the baseline, when it is one.
    pub(crate) fn periodic(&self) -> Option<&UsableAutomaticSnapshot> {
        match self {
            Self::Periodic(snapshot) => Some(snapshot),
            _ => None,
        }
    }

    /// The column of the start outcome table of a start from this baseline.
    pub(crate) fn role(&self) -> BaselineRole {
        match self {
            Self::Periodic(snapshot) => BaselineRole::Periodic(snapshot.index),
            Self::AssistedPending { head, .. } => BaselineRole::AssistedPending(head.clone()),
            Self::AssistedPromoted { .. } => BaselineRole::AssistedPromoted,
            Self::ManualPending { head, .. } => BaselineRole::ManualPending(head.clone()),
            Self::ManualPromoted { .. } => BaselineRole::ManualPromoted,
            Self::InitialFiles => BaselineRole::InitialFiles,
        }
    }
}

/// The head of the update queue of a status, as a start sees it.
#[derive(Clone, Copy, Debug)]
enum Head<'a> {
    None,
    /// An automatic update without a strategy entry.
    UnselectedAutomatic(&'a PendingUpdateRef),
    /// A snapshot-assisted update with its frozen selection.
    SelectedAssisted(&'a PendingUpdateRef, &'a AssistedSelection),
    /// A plain automatic update with its strategy entry, or a snapshot-based manual update.
    Other(&'a PendingUpdateRef),
}

impl<'a> Head<'a> {
    fn of(status: &'a AgentStatusRecord) -> Self {
        match status.pending_updates.front() {
            None => Self::None,
            Some(head) if is_unselected_automatic(head) => Self::UnselectedAutomatic(head),
            Some(head) => match &head.kind {
                PendingUpdateKind::SnapshotAssistedAutomatic(selection) => {
                    Self::SelectedAssisted(head, selection)
                }
                PendingUpdateKind::Automatic | PendingUpdateKind::SnapshotBased { .. } => {
                    Self::Other(head)
                }
            },
        }
    }
}

/// Which records the update queue of a status allows a start to select.
#[derive(Clone, Copy, Debug)]
enum QueueFilter {
    /// No pending update: the records of the current revision.
    Empty,
    /// An automatic update without a strategy at the head: the records of the current revision
    /// when `target` is newer than it, and only the records before `before`, the `PendingUpdate`
    /// entry of the first snapshot-based manual update in the queue.
    UnselectedAutomatic {
        target: ComponentRevision,
        before: Option<OplogIndex>,
    },
    /// Any other head, or an automatic update whose earlier snapshot-assisted attempt with the
    /// same target from the same source could not use its record: no record.
    Closed,
}

impl QueueFilter {
    fn of(status: &AgentStatusRecord) -> Self {
        match Head::of(status) {
            Head::None => Self::Empty,
            Head::UnselectedAutomatic(head)
                if !incompatible_before(status, head.target_revision) =>
            {
                Self::UnselectedAutomatic {
                    target: head.target_revision,
                    before: status
                        .pending_updates
                        .iter()
                        .filter(|update| {
                            matches!(update.kind, PendingUpdateKind::SnapshotBased { .. })
                        })
                        .map(|update| update.oplog_index)
                        .min(),
                }
            }
            Head::UnselectedAutomatic(_) | Head::SelectedAssisted(..) | Head::Other(_) => {
                Self::Closed
            }
        }
    }
}

/// Whether a live failed update of `target` from the current source of `status` could not load
/// its selected record or diverged after it. A request for the same target from the same source
/// then replays the full history.
fn incompatible_before(status: &AgentStatusRecord, target: ComponentRevision) -> bool {
    status.failed_updates.iter().any(|failed| {
        failed.target_revision == target
            && failed.snapshot_fault == Some(SnapshotFault::Incompatible)
            && failed
                .snapshot_assisted_details
                .as_ref()
                .is_some_and(|details| {
                    details.source_component_revision == status.component_revision
                        && details.source_revision_start_index
                            == status.component_revision_start_index
                })
    })
}

/// What a start excludes when it selects an automatic snapshot entry.
#[derive(Clone, Copy, Debug)]
struct AutomaticSnapshotFilter<'a> {
    /// What the update queue allows.
    queue: QueueFilter,
    /// The entries whose application snapshot did not load or whose replay diverged.
    rejected: &'a HashSet<OplogIndex>,
    /// The entries whose payload or filesystem snapshot this start could not get, when the filter
    /// excludes them.
    unavailable: Option<&'a HashSet<OplogIndex>>,
    /// Whether this executor restores filesystem snapshots. Without it, an entry with a
    /// filesystem snapshot name is not usable.
    filesystem_snapshots_enabled: bool,
}

/// The baseline of a start of `status` under `filter`: the record of a snapshot-assisted head,
/// else the record of a snapshot-based head, else a periodic record that passes, else the
/// authoritative baseline, else the initial files.
fn selected_baseline(
    status: &AgentStatusRecord,
    filter: AutomaticSnapshotFilter<'_>,
) -> SelectedBaseline {
    match Head::of(status) {
        Head::SelectedAssisted(head, selection) => SelectedBaseline::AssistedPending {
            snapshot: selection.snapshot.clone(),
            head: Box::new(head.clone()),
        },
        Head::Other(
            head @ PendingUpdateRef {
                kind: PendingUpdateKind::SnapshotBased { .. },
                ..
            },
        ) => SelectedBaseline::ManualPending {
            head: Box::new(head.clone()),
            previous: status.authoritative_snapshot.clone(),
        },
        Head::UnselectedAutomatic(head) => select_automatic_snapshot(status, filter).map_or_else(
            || authoritative_baseline(status),
            |snapshot| SelectedBaseline::AssistedPending {
                snapshot,
                head: Box::new(head.clone()),
            },
        ),
        Head::None | Head::Other(_) => select_automatic_snapshot(status, filter).map_or_else(
            || authoritative_baseline(status),
            SelectedBaseline::Periodic,
        ),
    }
}

/// The authoritative baseline of `status`, or the initial files when it has none.
fn authoritative_baseline(status: &AgentStatusRecord) -> SelectedBaseline {
    match &status.authoritative_snapshot {
        Some(AuthoritativeSnapshot {
            index,
            kind: AuthoritativeSnapshotKind::ManualUpdate,
        }) => SelectedBaseline::ManualPromoted { index: *index },
        Some(AuthoritativeSnapshot {
            index,
            kind:
                AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                    filesystem_snapshot,
                },
        }) => SelectedBaseline::AssistedPromoted {
            index: *index,
            name: filesystem_snapshot.clone(),
        },
        None => SelectedBaseline::InitialFiles,
    }
}

/// The component revision at the start of the replay from `baseline`: the revision of the
/// selected record, the target of a pending snapshot-based update, else the replay revision of
/// the status, which is the source revision after a snapshot-assisted update.
fn replay_revision(status: &AgentStatusRecord, baseline: &SelectedBaseline) -> ComponentRevision {
    match baseline {
        SelectedBaseline::Periodic(snapshot)
        | SelectedBaseline::AssistedPending { snapshot, .. } => snapshot.component_revision,
        SelectedBaseline::ManualPending { head, .. } => head.target_revision,
        SelectedBaseline::AssistedPromoted { .. }
        | SelectedBaseline::ManualPromoted { .. }
        | SelectedBaseline::InitialFiles => status.component_revision_for_replay,
    }
}

/// Gives the automatic snapshot entry that passes `filter`: the last usable entry, else the
/// previous usable entry.
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
    let queue_allows = match filter.queue {
        QueueFilter::Empty => true,
        QueueFilter::UnselectedAutomatic { target, before } => {
            target > status.component_revision && before.is_none_or(|before| index < before)
        }
        QueueFilter::Closed => false,
    };
    queue_allows
        && component_revision == status.component_revision
        && !filter.rejected.contains(&index)
        && !lost_in_failed_update(status, index)
        && !filter
            .unavailable
            .is_some_and(|unavailable| unavailable.contains(&index))
        && (filter.filesystem_snapshots_enabled || !has_filesystem_snapshot)
}

/// Whether a live failed update says that the store lost the filesystem snapshot of the record at
/// `index`: the snapshot-assisted attempt selected that record, and its restore found no snapshot.
/// The status keeps the failure, so no start selects the record again, also after a restart that
/// lost the rejection in memory. A revert that drops the failure drops the rule.
fn lost_in_failed_update(status: &AgentStatusRecord, index: OplogIndex) -> bool {
    status.failed_updates.iter().any(|failed| {
        failed.snapshot_fault == Some(SnapshotFault::Unavailable)
            && failed
                .snapshot_assisted_details
                .as_ref()
                .is_some_and(|details| details.snapshot_index == index)
    })
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

/// The filesystem snapshot names that retention keeps whatever their age: the names of the two
/// records that a start can select, the last record first, of the successful updates, of the
/// pending updates, and of the authoritative snapshot-assisted baseline. A revert rebuilds the
/// status, so the names of updates in its dropped region are not in it. Each name is given once.
pub(crate) fn names_in_use(status: &AgentStatusRecord) -> Box<[FilesystemSnapshotName]> {
    let authoritative = status
        .authoritative_snapshot
        .as_ref()
        .and_then(|snapshot| match &snapshot.kind {
            AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                filesystem_snapshot,
            } => filesystem_snapshot.as_ref(),
            AuthoritativeSnapshotKind::ManualUpdate => None,
        });
    start_candidates(status)
        .into_iter()
        .flatten()
        .filter_map(|(_, name)| name)
        .chain(
            status
                .successful_updates
                .iter()
                .filter_map(|update| update.filesystem_snapshot.as_ref()),
        )
        .chain(
            status
                .pending_updates
                .iter()
                .filter_map(|update| update.kind.filesystem_snapshot()),
        )
        .chain(authoritative)
        .fold(
            (HashSet::new(), Vec::new()),
            |(mut seen, mut names), name| {
                if seen.insert(name) {
                    names.push(name.clone());
                }
                (seen, names)
            },
        )
        .1
        .into_boxed_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
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
            queue: QueueFilter::Empty,
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
            replay_revision(
                &status,
                &selected_baseline(&status, filter(&HashSet::new()))
            ),
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
            replay_revision(&status, &selected_baseline(&status, rejecting(&both))),
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
                    queue: QueueFilter::Closed,
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
            admission_index: OplogIndex::from_u64(11),
            target_revision: revision(4),
            kind: PendingUpdateKind::SnapshotBased {
                filesystem_snapshot: None,
            },
        });
        assert_eq!(
            StartSelection::of(&status, &SnapshotExclusions::default(), true).replay_revision,
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
            pending_update: None,
            snapshot_assisted_details: None,
        }
    }

    fn pending_update(index: u64, kind: PendingUpdateKind) -> PendingUpdateRef {
        PendingUpdateRef {
            timestamp: Timestamp::from(1_000),
            oplog_index: OplogIndex::from_u64(index),
            admission_index: OplogIndex::from_u64(index),
            target_revision: revision(3),
            kind,
        }
    }

    #[test]
    fn the_names_in_use_are_the_candidate_successful_pending_and_authoritative_names() {
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

        let assisted = FilesystemSnapshotName::periodic();
        let last = FilesystemSnapshotName::periodic();
        let with_assisted = AgentStatusRecord {
            authoritative_snapshot: Some(AuthoritativeSnapshot {
                index: OplogIndex::from_u64(2),
                kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                    filesystem_snapshot: Some(assisted.clone()),
                },
            }),
            last_automatic_snapshot: Some(AutomaticSnapshot {
                index: OplogIndex::from_u64(13),
                timestamp: Timestamp::from(2_000),
                component_revision: revision(2),
                files: SnapshotFiles::Confirmed(last.clone()),
            }),
            ..status.clone()
        };
        let mut repeated = with_assisted.clone();
        repeated
            .successful_updates
            .push(successful_update(14, Some(assisted.clone())));

        assert_eq!(
            [
                names_in_use(&status),
                names_in_use(&AgentStatusRecord::default()),
                names_in_use(&with_assisted),
                names_in_use(&repeated),
            ],
            [
                Box::from([first.clone(), second.clone(), pending.clone()]),
                Box::from([]),
                Box::from([
                    last.clone(),
                    first.clone(),
                    second.clone(),
                    pending.clone(),
                    assisted.clone()
                ]),
                Box::from([last, first, second, assisted, pending]),
            ]
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

        let selected = StartSelection::of(&reused, &rejected, true)
            .baseline
            .periodic()
            .cloned();

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
            .baseline
            .periodic()
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
            oplog_index: OplogIndex::from_u64(12),
            admission_index: OplogIndex::from_u64(11),
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

    #[test]
    fn a_loaded_agent_waits_for_the_upload_of_the_newest_record_only_before_an_unselected_automatic_update()
     {
        let name = FilesystemSnapshotName::periodic();
        let with_head = |record: &AgentStatusRecord, head: PendingUpdateRef| {
            let mut status = record.clone();
            status.pending_updates.push_back(head);
            status
        };
        let unconfirmed = status(Some(name.clone()), false, None);
        let confirmed = status(Some(name.clone()), true, None);
        let strategy = PendingUpdateRef {
            oplog_index: OplogIndex::from_u64(13),
            ..unselected(12, 4)
        };
        let cases = [
            with_head(&unconfirmed, unselected(12, 4)),
            unconfirmed.clone(),
            with_head(&unconfirmed, strategy),
            with_head(&confirmed, unselected(12, 4)),
            with_head(&unconfirmed, unselected(12, 0)),
        ];
        let none = SnapshotExclusions::default();
        let reads = std::cell::Cell::new(0);

        assert_eq!(
            cases.each_ref().map(|status| {
                upload_before_an_automatic_update(status, true, || {
                    reads.set(reads.get() + 1);
                    &none
                })
            }),
            [Some(name), None, None, None, None]
        );
        assert_eq!(
            cases
                .each_ref()
                .map(|status| StartSelection::of(status, &none, true).candidate.is_some()),
            [true, true, false, false, false]
        );
        assert_eq!(reads.get(), 2);
    }

    fn unselected(admission: u64, target: u64) -> PendingUpdateRef {
        PendingUpdateRef {
            timestamp: Timestamp::from(1_000),
            oplog_index: OplogIndex::from_u64(admission),
            admission_index: OplogIndex::from_u64(admission),
            target_revision: revision(target),
            kind: PendingUpdateKind::Automatic,
        }
    }

    fn assisted_head(
        snapshot: UsableAutomaticSnapshot,
        source_revision_start_index: u64,
        target: u64,
    ) -> PendingUpdateRef {
        PendingUpdateRef {
            timestamp: Timestamp::from(1_000),
            oplog_index: OplogIndex::from_u64(12),
            admission_index: OplogIndex::from_u64(6),
            target_revision: revision(target),
            kind: PendingUpdateKind::SnapshotAssistedAutomatic(Box::new(AssistedSelection {
                source_revision_start_index: OplogIndex::from_u64(source_revision_start_index),
                snapshot,
            })),
        }
    }

    /// A status of source revision 2 that started at index 4, whose last record is at index 10
    /// and whose previous usable record is at index 5, with `head` at the head of its queue.
    fn with_head(
        last: Option<FilesystemSnapshotName>,
        confirmed: bool,
        previous: Option<Option<FilesystemSnapshotName>>,
        head: PendingUpdateRef,
    ) -> AgentStatusRecord {
        let mut status = status(last, confirmed, previous);
        status.component_revision_start_index = OplogIndex::from_u64(4);
        status.pending_updates.push_back(head);
        status
    }

    fn assisted_strategy(
        snapshot_index: u64,
        name: Option<FilesystemSnapshotName>,
    ) -> StartDecision {
        StartDecision::PersistStrategy {
            description: UpdateDescription::SnapshotAssistedAutomatic {
                target_revision: revision(3),
                source_component_revision: revision(2),
                source_revision_start_index: OplogIndex::from_u64(4),
                snapshot_index: OplogIndex::from_u64(snapshot_index),
                snapshot_revision: revision(2),
                filesystem_snapshot: name,
            },
            admission_index: OplogIndex::from_u64(6),
        }
    }

    fn automatic_strategy() -> StartDecision {
        StartDecision::PersistStrategy {
            description: UpdateDescription::Automatic {
                target_revision: revision(3),
            },
            admission_index: OplogIndex::from_u64(6),
        }
    }

    /// An automatic update that has no strategy yet selects the record that a start would
    /// select, also a record that came after its admission, and freezes it with its name.
    #[test]
    fn an_assisted_strategy_selects_a_snapshot_taken_after_the_admission() {
        let name = FilesystemSnapshotName::periodic();
        let status = with_head(Some(name.clone()), true, Some(None), unselected(6, 3));
        let none = SnapshotExclusions::default();

        assert_eq!(
            decide_start(&status, &none, true),
            assisted_strategy(10, Some(name))
        );
    }

    #[test]
    fn an_assisted_strategy_falls_back_to_the_previous_usable_record() {
        let name = FilesystemSnapshotName::periodic();
        let previous = FilesystemSnapshotName::periodic();
        let unconfirmed = with_head(
            Some(name.clone()),
            false,
            Some(Some(previous.clone())),
            unselected(6, 3),
        );
        let confirmed = with_head(
            Some(name),
            true,
            Some(Some(previous.clone())),
            unselected(6, 3),
        );
        let rejected =
            SnapshotExclusions::default().rejecting(OplogIndex::from_u64(10), &confirmed);
        let unavailable = SnapshotExclusions::default().with_unavailable(OplogIndex::from_u64(10));
        let none = SnapshotExclusions::default();

        assert_eq!(
            [
                decide_start(&unconfirmed, &none, true),
                decide_start(&confirmed, &rejected, true),
                decide_start(&confirmed, &unavailable, true),
            ],
            [
                assisted_strategy(5, Some(previous.clone())),
                assisted_strategy(5, Some(previous.clone())),
                assisted_strategy(5, Some(previous)),
            ]
        );
    }

    #[test]
    fn a_plain_automatic_strategy_when_no_record_passes_or_the_update_is_not_an_upgrade() {
        let name = FilesystemSnapshotName::periodic();
        let none = SnapshotExclusions::default();
        let named_only = with_head(Some(name.clone()), true, None, unselected(6, 3));
        let unnamed = with_head(None, true, None, unselected(6, 3));
        let mut downgrade = with_head(Some(name.clone()), true, None, unselected(6, 1));
        downgrade.pending_updates[0].target_revision = revision(1);
        let mut same = with_head(Some(name.clone()), true, None, unselected(6, 2));
        same.pending_updates[0].target_revision = revision(2);
        let rejected =
            SnapshotExclusions::default().rejecting(OplogIndex::from_u64(10), &named_only);

        assert_eq!(
            [
                decide_start(&named_only, &none, false),
                decide_start(&named_only, &rejected, true),
                decide_start(&unnamed, &none, false),
            ],
            [
                automatic_strategy(),
                automatic_strategy(),
                assisted_strategy(10, None),
            ]
        );
        assert!(matches!(
            decide_start(&downgrade, &none, true),
            StartDecision::PersistStrategy {
                description: UpdateDescription::Automatic { .. },
                ..
            }
        ));
        assert!(matches!(
            decide_start(&same, &none, true),
            StartDecision::PersistStrategy {
                description: UpdateDescription::Automatic { .. },
                ..
            }
        ));
    }

    fn failed(
        target: u64,
        source_revision_start_index: u64,
        fault: Option<SnapshotFault>,
    ) -> golem_common::model::FailedUpdateRecord {
        golem_common::model::FailedUpdateRecord {
            timestamp: Timestamp::from(1_000),
            target_revision: revision(target),
            details: None,
            pending_update: None,
            snapshot_assisted_details: Some(
                golem_common::model::oplog::FailedSnapshotAssistedUpdateDetails {
                    pending_update_index: OplogIndex::from_u64(8),
                    source_component_revision: revision(2),
                    source_revision_start_index: OplogIndex::from_u64(source_revision_start_index),
                    snapshot_index: OplogIndex::from_u64(10),
                },
            ),
            snapshot_fault: fault,
        }
    }

    /// After a failed snapshot-assisted update whose target could not use its record, the next
    /// request for the same target from the same source replays the full history. Another
    /// target, another source and a failure without a fault select as usual; a lost record is
    /// never selected again (see the next test).
    #[test]
    fn an_incompatible_assisted_failure_makes_the_next_request_of_the_same_update_a_full_replay() {
        let name = FilesystemSnapshotName::periodic();
        let none = SnapshotExclusions::default();
        let with_failure = |failure| {
            let mut status = with_head(Some(name.clone()), true, None, unselected(6, 3));
            status.failed_updates.push(failure);
            status
        };

        assert_eq!(
            [
                decide_start(
                    &with_failure(failed(3, 4, Some(SnapshotFault::Incompatible))),
                    &none,
                    true
                ),
                decide_start(
                    &with_failure(failed(4, 4, Some(SnapshotFault::Incompatible))),
                    &none,
                    true
                ),
                decide_start(
                    &with_failure(failed(3, 1, Some(SnapshotFault::Incompatible))),
                    &none,
                    true
                ),
                decide_start(
                    &with_failure(failed(3, 4, Some(SnapshotFault::Unavailable))),
                    &none,
                    true
                ),
                decide_start(&with_failure(failed(3, 4, None)), &none, true),
                decide_start(
                    &with_head(Some(name.clone()), true, None, unselected(6, 3)),
                    &none,
                    true
                ),
            ],
            [
                automatic_strategy(),
                assisted_strategy(10, Some(name.clone())),
                assisted_strategy(10, Some(name.clone())),
                automatic_strategy(),
                assisted_strategy(10, Some(name.clone())),
                assisted_strategy(10, Some(name)),
            ]
        );
    }

    /// A live failed update whose snapshot-assisted attempt lost the filesystem snapshot of its
    /// record excludes that record from every start: the next request of any target takes the
    /// previous usable record, a periodic start falls back the same way, and the record is not a
    /// start candidate. No exclusion in memory is needed, so a restart between the failure and
    /// the persisted rejection cannot select the record again.
    #[test]
    fn a_record_whose_snapshot_a_failed_update_lost_is_never_selected_again() {
        let name = FilesystemSnapshotName::periodic();
        let none = SnapshotExclusions::default();
        let lost = |mut status: AgentStatusRecord| {
            status
                .failed_updates
                .push(failed(4, 4, Some(SnapshotFault::Unavailable)));
            status
        };
        let request = lost(with_head(
            Some(name.clone()),
            true,
            Some(None),
            unselected(6, 3),
        ));
        let periodic = lost(status(Some(name.clone()), true, Some(None)));
        let unconfirmed = lost(status(Some(name.clone()), false, Some(None)));
        let previous = UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(5),
            component_revision: revision(2),
            filesystem_snapshot: None,
        };

        assert_eq!(
            decide_start(&request, &none, true),
            assisted_strategy(5, None)
        );
        assert_eq!(
            StartSelection::of(&periodic, &none, true).baseline,
            SelectedBaseline::Periodic(previous)
        );
        assert_eq!(
            StartSelection::of(&unconfirmed, &none, true).candidate,
            None
        );
    }

    /// The selection of a snapshot-assisted update is frozen: the exclusions, the candidates and
    /// the filesystem snapshot setting of the executor do not change it.
    #[test]
    fn a_frozen_assisted_selection_ignores_exclusions_candidates_and_the_executor_setting() {
        let frozen = record_at(7, Some(FilesystemSnapshotName::periodic()));
        let head = assisted_head(frozen.clone(), 4, 3);
        let status = with_head(
            Some(FilesystemSnapshotName::periodic()),
            true,
            Some(None),
            head.clone(),
        );
        let mut without_candidates = status.clone();
        without_candidates.last_automatic_snapshot = None;
        without_candidates.previous_usable_automatic_snapshot = None;
        let rejected = SnapshotExclusions::default()
            .rejecting(OplogIndex::from_u64(7), &status)
            .with_unavailable(OplogIndex::from_u64(7));
        let expected = SelectedBaseline::AssistedPending {
            snapshot: frozen,
            head: Box::new(head),
        };
        let baseline = |decision| match decision {
            StartDecision::Start(selection) => {
                Some((selection.baseline, selection.replay_revision))
            }
            _ => None,
        };

        assert!(
            [
                baseline(decide_start(&status, &SnapshotExclusions::default(), true)),
                baseline(decide_start(&status, &rejected, false)),
                baseline(decide_start(&without_candidates, &rejected, true)),
            ]
            .into_iter()
            .all(|selected| selected == Some((expected.clone(), revision(2))))
        );
    }

    fn record_at(index: u64, name: Option<FilesystemSnapshotName>) -> UsableAutomaticSnapshot {
        UsableAutomaticSnapshot {
            index: OplogIndex::from_u64(index),
            component_revision: revision(2),
            filesystem_snapshot: name,
        }
    }

    /// A frozen assisted head fails when its source revision, the start of that revision or the
    /// upgrade does not hold; only a snapshot-assisted head can fail this way.
    #[test]
    fn a_stale_or_downgrading_assisted_head_fails_and_no_other_head_does() {
        let valid = assisted_head(record_at(7, None), 4, 3);
        let status = with_head(None, true, None, valid.clone());
        let mut aba = status.clone();
        aba.component_revision_start_index = OplogIndex::from_u64(11);
        let mut moved = status.clone();
        moved.component_revision = revision(3);
        let downgrade = with_head(None, true, None, assisted_head(record_at(7, None), 4, 1));
        let found = |revision_value: u64, start: u64| SourceFound {
            revision: revision(revision_value),
            start_index: OplogIndex::from_u64(start),
        };
        let none = SnapshotExclusions::default();

        assert!(matches!(
            decide_start(&status, &none, true),
            StartDecision::Start(_)
        ));
        assert_eq!(
            [
                decide_start(&aba, &none, true),
                decide_start(&moved, &none, true),
                decide_start(&downgrade, &none, true),
            ],
            [
                StartDecision::FailHead {
                    role: BaselineRole::AssistedPending(Box::new(valid.clone())),
                    found: found(2, 11),
                },
                StartDecision::FailHead {
                    role: BaselineRole::AssistedPending(Box::new(valid)),
                    found: found(3, 4),
                },
                StartDecision::FailHead {
                    role: BaselineRole::AssistedPending(Box::new(assisted_head(
                        record_at(7, None),
                        4,
                        1
                    ))),
                    found: found(2, 4),
                },
            ]
        );
        let other_heads = [
            unselected(6, 3),
            PendingUpdateRef {
                oplog_index: OplogIndex::from_u64(12),
                ..unselected(6, 3)
            },
            PendingUpdateRef {
                kind: PendingUpdateKind::SnapshotBased {
                    filesystem_snapshot: None,
                },
                ..unselected(6, 3)
            },
        ];
        assert!(
            other_heads
                .iter()
                .all(|head| stale_assisted_head(&aba, head).is_none())
        );
    }

    /// A start instantiates the first queued update whose source holds: a stale assisted head is
    /// passed over, a valid one is the head, and an empty queue has none.
    #[test]
    fn the_active_head_is_the_first_update_whose_source_holds() {
        let valid = assisted_head(record_at(7, None), 4, 3);
        let stale = assisted_head(record_at(7, None), 4, 1);
        let behind = unselected(13, 4);
        let queued = |head: PendingUpdateRef| {
            let mut status = with_head(None, true, None, head);
            status.pending_updates.push_back(behind.clone());
            status
        };

        assert_eq!(active_head(&queued(valid.clone())), Some(&valid));
        assert_eq!(active_head(&queued(stale)), Some(&behind));
        assert_eq!(active_head(&status(None, true, None)), None);
    }

    /// The strategy entry refines the head, so a start after it never writes a strategy again.
    #[test]
    fn a_start_after_the_strategy_entry_never_persists_a_strategy() {
        let name = FilesystemSnapshotName::periodic();
        let none = SnapshotExclusions::default();
        let plain = with_head(
            Some(name.clone()),
            true,
            None,
            PendingUpdateRef {
                oplog_index: OplogIndex::from_u64(12),
                ..unselected(6, 3)
            },
        );
        let assisted = with_head(
            Some(name),
            true,
            None,
            assisted_head(record_at(10, None), 4, 3),
        );

        assert!(
            [
                decide_start(&plain, &none, true),
                decide_start(&assisted, &none, true)
            ]
            .iter()
            .all(|decision| matches!(decision, StartDecision::Start(_)))
        );
    }

    /// A start with an automatic update that has no strategy waits for the unconfirmed newest
    /// record, as a start without a pending update does, and its selection is the one that the
    /// strategy entry then freezes.
    #[test]
    fn an_unselected_head_waits_for_the_newest_record_and_selects_what_its_strategy_freezes() {
        let name = FilesystemSnapshotName::periodic();
        let none = SnapshotExclusions::default();
        let unconfirmed = with_head(Some(name.clone()), false, Some(None), unselected(6, 3));
        let confirmed = with_head(Some(name.clone()), true, Some(None), unselected(6, 3));
        let after_strategy = {
            let mut status = confirmed.clone();
            status.pending_updates[0] = PendingUpdateRef {
                oplog_index: OplogIndex::from_u64(12),
                ..assisted_head(record_at(10, Some(name.clone())), 4, 3)
            };
            status
        };
        let selected_record =
            |status: &AgentStatusRecord| match StartSelection::of(status, &none, true).baseline {
                SelectedBaseline::AssistedPending { snapshot, .. } => Some(snapshot),
                _ => None,
            };

        assert_eq!(
            StartSelection::of(&unconfirmed, &none, true).candidate,
            Some(name.clone())
        );
        assert_eq!(StartSelection::of(&confirmed, &none, true).candidate, None);
        assert_eq!(
            selected_record(&confirmed),
            selected_record(&after_strategy)
        );
        assert_eq!(selected_record(&confirmed), Some(record_at(10, Some(name))));
        assert_eq!(
            StartSelection::of(&confirmed, &none, true).replay_revision,
            StartSelection::of(&after_strategy, &none, true).replay_revision
        );
    }

    /// A record after the `PendingUpdate` entry of a manual update queued behind the head is not
    /// selected: its skipped region would reach past the manual snapshot.
    #[test]
    fn an_unselected_head_does_not_select_a_record_after_a_queued_manual_update() {
        let none = SnapshotExclusions::default();
        let manual_at = |index: u64| PendingUpdateRef {
            timestamp: Timestamp::from(1_000),
            oplog_index: OplogIndex::from_u64(index),
            admission_index: OplogIndex::from_u64(index),
            target_revision: revision(4),
            kind: PendingUpdateKind::SnapshotBased {
                filesystem_snapshot: None,
            },
        };
        let queued = |index| {
            let mut status = with_head(None, true, Some(None), unselected(6, 3));
            status.pending_updates.push_back(manual_at(index));
            status
        };

        assert_eq!(
            [
                decide_start(&queued(8), &none, true),
                decide_start(&queued(11), &none, true),
            ],
            [assisted_strategy(5, None), assisted_strategy(10, None)]
        );
        assert_eq!(decide_start(&queued(4), &none, true), automatic_strategy());
    }

    /// After a snapshot-assisted update the authoritative baseline is its record, which replays
    /// from the source revision; a rejected newer periodic record falls back to it, never to a
    /// full replay.
    #[test]
    fn a_rejected_periodic_record_after_an_assisted_update_uses_the_promoted_record_on_the_source_revision()
     {
        let name = FilesystemSnapshotName::periodic();
        let mut status = status(None, true, None);
        status.component_revision = revision(3);
        status.component_revision_for_replay = revision(2);
        status.authoritative_snapshot = Some(AuthoritativeSnapshot {
            index: OplogIndex::from_u64(7),
            kind: AuthoritativeSnapshotKind::SnapshotAssistedAutomatic {
                filesystem_snapshot: Some(name.clone()),
            },
        });
        if let Some(last) = status.last_automatic_snapshot.as_mut() {
            last.component_revision = revision(3);
        }
        let rejected = SnapshotExclusions::default().rejecting(OplogIndex::from_u64(10), &status);
        let selection = StartSelection::of(&status, &rejected, true);
        let usable = StartSelection::of(&status, &SnapshotExclusions::default(), true);

        assert_eq!(
            (selection.baseline, selection.replay_revision),
            (
                SelectedBaseline::AssistedPromoted {
                    index: OplogIndex::from_u64(7),
                    name: Some(name),
                },
                revision(2)
            )
        );
        assert_eq!(
            (
                usable.baseline.periodic().map(|record| record.index),
                usable.replay_revision
            ),
            (Some(OplogIndex::from_u64(10)), revision(3))
        );
    }

    #[test]
    fn a_selection_follows_the_priority_of_the_queue_head_then_the_records_then_the_baseline() {
        let none = SnapshotExclusions::default();
        let manual_head = PendingUpdateRef {
            timestamp: Timestamp::from(1_000),
            oplog_index: OplogIndex::from_u64(11),
            admission_index: OplogIndex::from_u64(9),
            target_revision: revision(4),
            kind: PendingUpdateKind::SnapshotBased {
                filesystem_snapshot: None,
            },
        };
        let authoritative = AuthoritativeSnapshot {
            index: OplogIndex::from_u64(3),
            kind: AuthoritativeSnapshotKind::ManualUpdate,
        };
        let mut manual = with_head(None, true, None, manual_head.clone());
        manual.authoritative_snapshot = Some(authoritative.clone());
        let mut promoted = status(None, false, None);
        promoted.last_automatic_snapshot = None;
        promoted.authoritative_snapshot = Some(authoritative.clone());
        let mut initial = promoted.clone();
        initial.authoritative_snapshot = None;
        let selection = |status: &AgentStatusRecord| {
            let selection = StartSelection::of(status, &none, true);
            (selection.baseline, selection.replay_revision)
        };

        assert_eq!(
            [
                selection(&manual),
                selection(&promoted),
                selection(&initial)
            ],
            [
                (
                    SelectedBaseline::ManualPending {
                        head: Box::new(manual_head),
                        previous: Some(authoritative),
                    },
                    revision(4)
                ),
                (
                    SelectedBaseline::ManualPromoted {
                        index: OplogIndex::from_u64(3)
                    },
                    revision(1)
                ),
                (SelectedBaseline::InitialFiles, revision(1)),
            ]
        );
    }
}
