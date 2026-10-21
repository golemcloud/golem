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
//! snapshot, or when it has no filesystem snapshot name. An executor without filesystem snapshots
//! uses only a record without a name. A start without a pending update takes the first of the two
//! that is usable, of the current component revision, not rejected, not lost in a failed update,
//! and not unavailable for this start. A start with a plain automatic update that has its strategy
//! entry takes no record: that update replays the full history after the authoritative baseline.
//! An automatic update at the head of the queue that has no strategy yet takes a record by the
//! same rules, only when its target is newer than the current revision and only from before the
//! first snapshot-based manual update in the queue. It takes no record when an earlier
//! snapshot-assisted attempt with the same target from the same source could not load its record
//! or diverged after it. Its strategy entry freezes that choice. When no record is taken, the
//! start uses the authoritative baseline, else the initial files with a full replay.

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
use std::sync::Arc;

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
                role: BaselineRole::AssistedPending(Arc::new(head.clone())),
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

/// The periodic record that a start of `status` selects under `exclusions`, which is the periodic
/// baseline of [`StartSelection::of`]: only a start without a pending update selects one. A start
/// with a plain automatic update that has its strategy entry selects none. `enabled` tells whether
/// this executor keeps filesystem snapshots.
pub(crate) fn periodic_baseline(
    status: &AgentStatusRecord,
    exclusions: &SnapshotExclusions,
    enabled: bool,
) -> Option<UsableAutomaticSnapshot> {
    match Head::of(status) {
        Head::None => select_automatic_snapshot(status, exclusions.filter(status, enabled, true)),
        Head::UnselectedAutomatic(_) | Head::SelectedAssisted(..) | Head::Other(_) => None,
    }
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
        let (baseline, replay_revision_without_unavailable) = match frozen_baseline(status) {
            Ok(baseline) => (baseline, None),
            Err(head) => (
                filtered_baseline(status, head, filter),
                Some(replay_revision(
                    status,
                    &filtered_baseline(status, head, exclusions.filter(status, enabled, false)),
                )),
            ),
        };
        let replay_revision = replay_revision(status, &baseline);
        Self {
            replay_revision,
            replay_revision_without_unavailable: replay_revision_without_unavailable
                .unwrap_or(replay_revision),
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
        head: Arc<PendingUpdateRef>,
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
        head: Arc<PendingUpdateRef>,
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
    #[cfg(test)]
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
    /// same target from the same source could not load its record or diverged after it: no
    /// record.
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
    /// The entries that [`SnapshotExclusions`] rejects.
    rejected: &'a HashSet<OplogIndex>,
    /// The entries whose payload or filesystem snapshot this start could not get, when the filter
    /// excludes them.
    unavailable: Option<&'a HashSet<OplogIndex>>,
    /// Whether this executor restores filesystem snapshots. Without it, an entry with a
    /// filesystem snapshot name is not usable.
    filesystem_snapshots_enabled: bool,
}

/// A head of the queue that froze no baseline.
#[derive(Clone, Copy, Debug)]
enum OpenHead<'a> {
    /// No pending update, or a plain automatic update with its strategy entry: a start selects
    /// a periodic record.
    Periodic,
    /// An automatic update without a strategy entry: a start selects the record of its
    /// snapshot-assisted strategy.
    UnselectedAutomatic(&'a PendingUpdateRef),
}

/// The baseline that the head of the queue of `status` froze, which no filter changes: the
/// record of a snapshot-assisted head, or the record of a snapshot-based head. Any other head
/// froze none, and the error gives it.
fn frozen_baseline(status: &AgentStatusRecord) -> Result<SelectedBaseline, OpenHead<'_>> {
    match Head::of(status) {
        Head::SelectedAssisted(head, selection) => Ok(SelectedBaseline::AssistedPending {
            snapshot: selection.snapshot.clone(),
            head: Arc::new(head.clone()),
        }),
        Head::Other(
            head @ PendingUpdateRef {
                kind: PendingUpdateKind::SnapshotBased { .. },
                ..
            },
        ) => Ok(SelectedBaseline::ManualPending {
            head: Arc::new(head.clone()),
            previous: status.authoritative_snapshot.clone(),
        }),
        Head::UnselectedAutomatic(head) => Err(OpenHead::UnselectedAutomatic(head)),
        Head::None | Head::Other(_) => Err(OpenHead::Periodic),
    }
}

/// The baseline of a start of `status` with the open `head` under `filter`: a record that
/// passes, else the authoritative baseline, else the initial files.
fn filtered_baseline(
    status: &AgentStatusRecord,
    head: OpenHead<'_>,
    filter: AutomaticSnapshotFilter<'_>,
) -> SelectedBaseline {
    let selected = select_automatic_snapshot(status, filter);
    match head {
        OpenHead::UnselectedAutomatic(head) => selected.map_or_else(
            || authoritative_baseline(status),
            |snapshot| SelectedBaseline::AssistedPending {
                snapshot,
                head: Arc::new(head.clone()),
            },
        ),
        OpenHead::Periodic => selected.map_or_else(
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
/// records that a start can select, of the successful updates, of the pending updates, and of the
/// authoritative snapshot-assisted baseline. A revert rebuilds the status, so the names of updates
/// in its dropped region are not in it. Each name is given once, in the order of its text.
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
    let mut names = start_candidates(status)
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
        .collect::<Vec<_>>();
    // A sort and a dedup take O(k log k) for k names, and need no hash set.
    names.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
    names.dedup();
    names.into_iter().cloned().collect()
}

#[cfg(test)]
mod tests;

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Whether this executor restores filesystem snapshots.
    pub(in crate::worker) fn filesystem_snapshots_enabled(&self) -> bool {
        self.agent_filesystem_snapshots().is_enabled()
    }
    /// The selection of a start of `status` under the exclusions that this incarnation holds in
    /// memory now. It reads no storage, so it does not see the rejections that another executor
    /// stored since the last [`Worker::select_start`]. A start selects with
    /// [`Worker::select_start`].
    pub(in crate::worker) fn selection_in_memory(
        &self,
        status: &AgentStatusRecord,
    ) -> StartSelection {
        let enabled = self.filesystem_snapshots_enabled();
        self.read_exclusions(|exclusions| StartSelection::of(status, exclusions, enabled))
    }
    /// Selects the start of `status`, as [`Worker::decide_start`] does, without the decision on
    /// the head of the queue.
    pub(in crate::worker) async fn select_start(
        &self,
        status: &AgentStatusRecord,
    ) -> Result<StartSelection, WorkerExecutorError> {
        let enabled = self.filesystem_snapshots_enabled();
        self.with_persisted_exclusions(status, |exclusions| {
            StartSelection::of(status, exclusions, enabled)
        })
        .await
    }
    /// Decides the start of `status`, as [`snapshot_selection::decide_start`] does, under the
    /// exclusions of the agent with the rejected entries that storage keeps for the incarnation.
    pub(in crate::worker) async fn decide_start(
        &self,
        status: &AgentStatusRecord,
    ) -> Result<StartDecision, WorkerExecutorError> {
        let enabled = self.filesystem_snapshots_enabled();
        self.with_persisted_exclusions(status, |exclusions| {
            snapshot_selection::decide_start(status, exclusions, enabled)
        })
        .await
    }
    /// Decides the start of `status` as [`Worker::decide_start`] does, under the exclusions that
    /// this incarnation holds in memory, without a read of storage. A start decides with it again
    /// after it wrote a strategy entry: its first decision already added the stored rejections.
    pub(in crate::worker) fn decide_start_in_memory(
        &self,
        status: &AgentStatusRecord,
    ) -> StartDecision {
        let enabled = self.filesystem_snapshots_enabled();
        self.read_exclusions(|exclusions| {
            snapshot_selection::decide_start(status, exclusions, enabled)
        })
    }
    /// Loads the rejected entries that storage keeps for the incarnation, adds them to the
    /// exclusions of the agent for a start of `status`, and gives `read` of the exclusions.
    pub(in crate::worker) async fn with_persisted_exclusions<T>(
        &self,
        status: &AgentStatusRecord,
        read: impl FnOnce(&SnapshotExclusions) -> T,
    ) -> Result<T, WorkerExecutorError> {
        let persisted = self
            .worker_service()
            .get_rejected_periodic_snapshots(
                &self.owned_agent_id,
                self.initial_worker_metadata.fingerprint,
            )
            .await?;
        Ok(self.update_exclusions(
            |exclusions| exclusions.with_persisted(persisted, status),
            read,
        ))
    }
    /// Settles the exclusions after the agent prepared with success: it stores the rejected
    /// entries for the incarnation, when there are any, and ends the skips of unavailable
    /// entries.
    pub(crate) async fn settle_exclusions_after_prepare(&self) -> Result<(), WorkerExecutorError> {
        if let Some(rejected) = self.read_exclusions(SnapshotExclusions::persisted_rejections) {
            let status = self.get_last_known_status().await;
            self.worker_service()
                .reject_periodic_snapshots(
                    &self.owned_agent_id,
                    self.initial_worker_metadata.fingerprint,
                    &status,
                    &rejected,
                )
                .await?;
        }
        self.clear_unavailable_periodic();
        Ok(())
    }
    /// Rejects the automatic snapshot entry at `index` for the starts of this incarnation: its
    /// application snapshot did not load, or the replay after it diverged.
    pub(crate) fn reject_periodic(&self, index: OplogIndex) {
        let status = self.last_known_status.load();
        self.update_exclusions(|exclusions| exclusions.rejecting(index, &status), |_| ());
    }
    /// Skips the automatic snapshot entry at `index` until a start prepares the agent with
    /// success, or until a new startup attempt begins: its payload or its filesystem snapshot
    /// could not be read.
    pub(crate) fn mark_periodic_unavailable(&self, index: OplogIndex) {
        self.update_exclusions(|exclusions| exclusions.with_unavailable(index), |_| ());
    }
    /// Ends the skips of unavailable automatic snapshot entries.
    pub(in crate::worker) fn clear_unavailable_periodic(&self) {
        self.update_exclusions(SnapshotExclusions::without_unavailable, |_| ());
    }
    /// Gives what `read` gives for the exclusions of the agent now, under their lock.
    pub(in crate::worker) fn read_exclusions<T>(
        &self,
        read: impl FnOnce(&SnapshotExclusions) -> T,
    ) -> T {
        read(
            &self
                .snapshot_exclusions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
    /// Replaces the exclusions of the agent with what `next` gives for them, and gives what
    /// `read` gives for the new exclusions, both under one hold of their lock.
    pub(in crate::worker) fn update_exclusions<T>(
        &self,
        next: impl FnOnce(SnapshotExclusions) -> SnapshotExclusions,
        read: impl FnOnce(&SnapshotExclusions) -> T,
    ) -> T {
        let mut exclusions = self
            .snapshot_exclusions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *exclusions = next(std::mem::take(&mut *exclusions));
        read(&exclusions)
    }
}
