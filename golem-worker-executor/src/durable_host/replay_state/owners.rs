//! The owners of the `Start`s a replay cursor committed.
//!
//! Every committed `Start` gets its owner from one function of its recorded fields and of the
//! owners of earlier entries ([`OwnerMap::record_owner`]):
//!
//! 1. its `driver`, the invocation whose settlement waits for the call, when it records one;
//! 2. else the anchor of its invocation id, resolved through entity and custom `Start`s to an
//!    invocation;
//! 3. else its `parent_start_index` or `observational_owner`;
//! 4. else [`InvocationOwner::AgentLevel`].
//!
//! A reference that does not resolve to an earlier committed entry is an [`Orphan`]: committing a
//! `Start` whose owner is an orphan is a divergence, with or without a driver. A parent or
//! observational owner that resolves to no owner counts as pruned only when it is a committed
//! `Start` whose owner a close dropped; any other one is an orphan. A pruned reference gives way
//! to a resolved invocation id, or, when the `Start` records no invocation id, to its driver. It
//! belongs to a closed invocation, so every entity body on its chain ended, and a driven `Start`
//! under it has no enclosing body. The map is resident state derived from the recorded prefix,
//! rebuilt by every new cursor.
//!
//! A close drops the owners of its invocation's `Start`s except retained ones, and keeps no record
//! of what it dropped: whether a reference that resolves to no owner is pruned is a question about
//! the recorded prefix (is the entry a committed `Start`?), so the cursor answers it from the oplog
//! ([`OwnerMap::unresolved_reference`]) and passes the answer in. The entity and custom anchors
//! stay for the life of the cursor, one per entity or custom invocation: an invocation id or a
//! reference can name them after their invocation closed, and their recorded owner and
//! cancellation still decide such a `Start`.

use golem_common::model::InvocationId;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::{BTreeMap, BTreeSet};

use crate::durable_host::attribution::ExportReturned;

use super::cursor::{boundary_divergence, is_custom_invocation_start, is_entity_invocation_start};

/// The invocation that owns a committed record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum InvocationOwner {
    /// The invocation whose `AgentInvocationStarted` is at this index.
    Anchor(OplogIndex),
    /// Root work that no invocation drives: recorded with no driver, invocation id, parent or
    /// observational owner.
    AgentLevel,
}

/// The owner of a committed `Start`: its invocation, and the nearest entity `Start` that
/// encloses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RecordOwner {
    pub(super) invocation: InvocationOwner,
    pub(super) body: Option<OplogIndex>,
}

impl RecordOwner {
    const AGENT_LEVEL: Self = Self {
        invocation: InvocationOwner::AgentLevel,
        body: None,
    };

    fn invocation(anchor: OplogIndex) -> Self {
        Self {
            invocation: InvocationOwner::Anchor(anchor),
            body: None,
        }
    }
}

/// A recorded reference that resolves to no earlier committed entry. Always a divergence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Orphan(pub(super) String);

/// An invocation whose `AgentInvocationStarted` the cursor committed and whose close it has not
/// reached.
#[derive(Debug)]
pub(super) struct OpenInvocation {
    pub(super) id: InvocationId,
    /// The return signal of an invocation that records its result at the export's return, set
    /// by `ReplayState::open_invocation_anchor`. Such an invocation closes at its
    /// `AgentInvocationSettled` or at the replay tail; the others close at their terminal.
    pub(super) export_returned: Option<ExportReturned>,
}

impl OpenInvocation {
    /// Whether the invocation records its result at the export's return.
    pub(super) fn records_result_at_return(&self) -> bool {
        self.export_returned.is_some()
    }

    /// Whether the invocation records its result at the export's return, and its export
    /// returned.
    pub(super) fn has_returned(&self) -> bool {
        self.export_returned
            .as_ref()
            .is_some_and(ExportReturned::is_marked)
    }
}

/// A committed `Start` that identities and references can name besides an invocation anchor.
#[derive(Debug, Clone, Copy)]
enum AnchorKind {
    /// The `Start` of an entity invocation. `cancelled` is set once its recorded `Cancelled`
    /// terminal is committed.
    Entity { owner: RecordOwner, cancelled: bool },
    /// The root `Start` of a custom durable invocation.
    Custom { owner: RecordOwner },
}

/// The lifecycle of every invocation the cursor started, and the owners of the committed
/// `Start`s.
#[derive(Debug, Default)]
pub(super) struct OwnerMap {
    /// The invocations whose `AgentInvocationStarted` the cursor committed and whose close it did
    /// not reach, by anchor. Every committed invocation is in this map or in
    /// [`Self::closed_invocations`].
    open_invocations: BTreeMap<OplogIndex, OpenInvocation>,
    /// The invocations whose close the cursor reached: the entry that closed each, by anchor.
    closed_invocations: BTreeMap<OplogIndex, OplogIndex>,
    /// Every committed entity `Start` and custom root `Start`, one per entity or custom
    /// invocation, for the life of the cursor: invocation ids and references name them after their
    /// invocation closed.
    identity_anchors: BTreeMap<OplogIndex, AnchorKind>,
    /// The owners of the committed `Start`s of open invocations and of agent-level work, and of
    /// the `Start`s of closed invocations that a close kept because they were retained (see
    /// [`Self::forget_released`]).
    owners: BTreeMap<OplogIndex, RecordOwner>,
}

impl OwnerMap {
    /// The owner of the `Start` `entry` at `index`, from its recorded fields and the owners of
    /// the entries committed before it. A parent or observational owner that resolves to no owner
    /// gives way to a resolved invocation id, or with no invocation id to a valid driver, when it
    /// is `pruned`: a committed `Start` whose owner a close dropped. Any other one, such as a
    /// later entry, an entry in a deleted region or an earlier entry that is no `Start`, is an
    /// [`Orphan`].
    pub(super) fn record_owner(
        &self,
        index: OplogIndex,
        entry: &OplogEntry,
        pruned: bool,
    ) -> Result<RecordOwner, Orphan> {
        let OplogEntry::Start {
            driver,
            invocation_id,
            parent_start_index,
            observational_owner,
            ..
        } = entry
        else {
            return Err(Orphan(format!("the entry at {index} is not a Start")));
        };
        let reference = parent_start_index.or(*observational_owner);
        let recorded = match invocation_id {
            Some(id) => match (self.resolve(index, id.anchor, true), reference) {
                (Err(orphan), _) => Err(orphan),
                (Ok(by_id), None) => Ok(by_id),
                (Ok(by_id), Some(reference)) => match self.resolve(index, reference, false) {
                    Ok(by_reference) if by_reference.invocation == by_id.invocation => {
                        Ok(RecordOwner {
                            invocation: by_id.invocation,
                            body: by_id.body.max(by_reference.body),
                        })
                    }
                    Err(orphan) if !pruned => Err(orphan),
                    _ => Ok(by_id),
                },
            },
            None => match reference {
                Some(reference) => self.resolve(index, reference, false),
                None => Ok(RecordOwner::AGENT_LEVEL),
            },
        };
        match driver {
            None => recorded,
            Some(driver) if *driver < index && self.is_invocation(*driver) => {
                // With no invocation id, a reference that resolves to no owner gives way to the
                // driver when it is pruned. Its invocation closed, so every body on the
                // reference's chain ended: no body encloses the Start.
                let body = match recorded {
                    Ok(recorded) => recorded.body,
                    Err(_) if invocation_id.is_none() && pruned => None,
                    Err(orphan) => return Err(orphan),
                };
                Ok(RecordOwner {
                    invocation: InvocationOwner::Anchor(*driver),
                    body,
                })
            }
            Some(driver) => Err(Orphan(format!(
                "the Start at {index} records the driver {driver}, which is no earlier committed \
                 AgentInvocationStarted"
            ))),
        }
    }

    /// Resolves `reference`, recorded by the `Start` at `index` as an invocation id anchor
    /// (`by_id`) or as a parent or observational owner.
    fn resolve(
        &self,
        index: OplogIndex,
        reference: OplogIndex,
        by_id: bool,
    ) -> Result<RecordOwner, Orphan> {
        if reference >= index {
            return Err(Orphan(format!(
                "the Start at {index} references the later entry {reference}"
            )));
        }
        if self.is_invocation(reference) {
            return Ok(RecordOwner::invocation(reference));
        }
        match self.identity_anchors.get(&reference) {
            Some(AnchorKind::Entity { owner, .. }) => Ok(RecordOwner {
                invocation: owner.invocation,
                body: Some(reference),
            }),
            Some(AnchorKind::Custom { owner }) => Ok(*owner),
            None if by_id => Err(Orphan(format!(
                "the invocation id of the Start at {index} is anchored at {reference}, which is \
                 no committed invocation, entity or custom Start"
            ))),
            None => self.owners.get(&reference).copied().ok_or_else(|| {
                Orphan(format!(
                    "the Start at {index} references {reference}, which is no committed Start of \
                     an open invocation"
                ))
            }),
        }
    }

    /// The parent or observational owner of the `Start` `entry` at `index` when it is an earlier
    /// entry that resolves to no owner. It is either an orphan or a committed `Start` whose owner a
    /// close dropped; only the recorded prefix tells them apart, so the caller looks the entry up
    /// and passes the answer to [`Self::validate_start`].
    pub(super) fn unresolved_reference(
        &self,
        index: OplogIndex,
        entry: &OplogEntry,
    ) -> Option<OplogIndex> {
        let OplogEntry::Start {
            parent_start_index,
            observational_owner,
            ..
        } = entry
        else {
            return None;
        };
        let reference = parent_start_index.or(*observational_owner)?;
        (reference < index && self.resolve(index, reference, false).is_err()).then_some(reference)
    }

    /// Records the owner of the `Start` committed at `index`, whose references are no pruned
    /// `Start`s: [`Self::validate_start`], then [`Self::record_start`].
    #[cfg(test)]
    pub(super) fn commit_start(
        &mut self,
        index: OplogIndex,
        entry: &OplogEntry,
    ) -> Result<(), String> {
        let owner = self.validate_start(index, entry, false)?;
        self.record_start(index, entry, owner);
        Ok(())
    }

    /// The owner of the `Start` at `index`, without changing the map, where `pruned` tells
    /// whether its [`Self::unresolved_reference`] is a committed `Start`. Returns the divergence
    /// of an [`Orphan`], of a `Start` recorded without a driver after its invocation closed, of a
    /// root `Start` with no driver or invocation id recorded while an invocation is open, and of
    /// a `Start` recorded after the recorded `Cancelled` of its enclosing entity body.
    pub(super) fn validate_start(
        &self,
        index: OplogIndex,
        entry: &OplogEntry,
        pruned: bool,
    ) -> Result<RecordOwner, String> {
        let owner = self
            .record_owner(index, entry, pruned)
            .map_err(|Orphan(reason)| reason)?;
        if self.has_open_invocation() && owner == RecordOwner::AGENT_LEVEL {
            return Err(format!(
                "the Start at {index} records no driver, invocation id, parent or observational \
                 owner, but an invocation is open"
            ));
        }
        if let InvocationOwner::Anchor(anchor) = owner.invocation
            && matches!(entry, OplogEntry::Start { driver: None, .. })
            && self.is_closed(anchor)
        {
            return Err(format!(
                "the Start at {index} belongs to the invocation started at {anchor}, which closed \
                 before it, and records no driver"
            ));
        }
        if let Some(body) = owner.body
            && self.is_cancelled_entity(body)
        {
            return Err(format!(
                "the Start at {index} was recorded after the recorded cancellation of its entity \
                 body at {body}"
            ));
        }
        Ok(owner)
    }

    /// Records `owner`, which [`Self::validate_start`] returned, for the `Start` committed at
    /// `index`.
    pub(super) fn record_start(
        &mut self,
        index: OplogIndex,
        entry: &OplogEntry,
        owner: RecordOwner,
    ) {
        if is_entity_invocation_start(entry) {
            self.identity_anchors.insert(
                index,
                AnchorKind::Entity {
                    owner,
                    cancelled: false,
                },
            );
        } else if is_custom_invocation_start(entry) {
            self.identity_anchors
                .insert(index, AnchorKind::Custom { owner });
        }
        self.owners.insert(index, owner);
    }

    /// Records the `AgentInvocationStarted` of the invocation `id` committed at `index`: the
    /// invocation is open until [`Self::close`].
    pub(super) fn commit_invocation_started(&mut self, index: OplogIndex, id: InvocationId) {
        self.open_invocations.insert(
            index,
            OpenInvocation {
                id,
                export_returned: None,
            },
        );
    }

    /// Records a committed terminal of the `Start` at `start_index`.
    pub(super) fn commit_terminal(&mut self, start_index: OplogIndex, terminal: &OplogEntry) {
        if let Some(AnchorKind::Entity { cancelled, .. }) =
            self.identity_anchors.get_mut(&start_index)
        {
            *cancelled = matches!(terminal, OplogEntry::Cancelled { .. });
        }
    }

    /// The recorded owner of the committed `Start` at `index`, while the map holds it.
    pub(super) fn owner(&self, index: OplogIndex) -> Option<RecordOwner> {
        self.owners.get(&index).copied()
    }

    /// The nearest entity body that encloses the committed `Start` at `index`, while the map
    /// holds its owner.
    pub(super) fn enclosing_body(&self, index: OplogIndex) -> Option<OplogIndex> {
        self.owner(index).and_then(|owner| owner.body)
    }

    /// The owner of the entity or custom `Start` at `index`.
    pub(super) fn anchor_owner(&self, index: OplogIndex) -> Option<RecordOwner> {
        match self.identity_anchors.get(&index)? {
            AnchorKind::Entity { owner, .. } | AnchorKind::Custom { owner } => Some(*owner),
        }
    }

    /// Whether `index` is a committed entity `Start`.
    pub(super) fn is_entity(&self, index: OplogIndex) -> bool {
        matches!(
            self.identity_anchors.get(&index),
            Some(AnchorKind::Entity { .. })
        )
    }

    /// Whether `index` is a committed entity `Start` closed by a recorded `Cancelled`.
    pub(super) fn is_cancelled_entity(&self, index: OplogIndex) -> bool {
        matches!(
            self.identity_anchors.get(&index),
            Some(AnchorKind::Entity {
                cancelled: true,
                ..
            })
        )
    }

    /// Whether an invocation started at `anchor`.
    fn is_invocation(&self, anchor: OplogIndex) -> bool {
        self.open_invocations.contains_key(&anchor) || self.closed_invocations.contains_key(&anchor)
    }

    /// Whether the invocation started at `anchor` closed.
    pub(super) fn is_closed(&self, anchor: OplogIndex) -> bool {
        self.closed_invocations.contains_key(&anchor)
    }

    /// Whether the invocation started at `anchor` is open: it started and did not close.
    pub(super) fn is_open(&self, anchor: OplogIndex) -> bool {
        self.open_invocations.contains_key(&anchor)
    }

    /// Whether an invocation is open.
    pub(super) fn has_open_invocation(&self) -> bool {
        !self.open_invocations.is_empty()
    }

    /// The open invocation `id`, with its anchor.
    pub(super) fn open_invocation(
        &self,
        id: &InvocationId,
    ) -> Option<(OplogIndex, &OpenInvocation)> {
        self.open_invocations
            .iter()
            .find(|(_, open)| open.id == *id)
            .map(|(anchor, open)| (*anchor, open))
    }

    /// The open invocation started at `anchor`.
    pub(super) fn open_invocation_at(&self, anchor: OplogIndex) -> Option<&OpenInvocation> {
        self.open_invocations.get(&anchor)
    }

    /// The open invocation started at `anchor`, to change.
    pub(super) fn open_invocation_at_mut(
        &mut self,
        anchor: OplogIndex,
    ) -> Option<&mut OpenInvocation> {
        self.open_invocations.get_mut(&anchor)
    }

    /// The anchor of the last invocation the cursor started.
    pub(super) fn last_invocation(&self) -> Option<OplogIndex> {
        let open = self
            .open_invocations
            .last_key_value()
            .map(|(anchor, _)| *anchor);
        let closed = self
            .closed_invocations
            .last_key_value()
            .map(|(anchor, _)| *anchor);
        open.max(closed)
    }

    /// Closes the open invocation started at `anchor` at the entry at `at`, drops the owners of
    /// its `Start`s that `retained` does not hold, then [`Self::forget_released`]. Every
    /// invocation closes once.
    pub(super) fn close(
        &mut self,
        anchor: OplogIndex,
        at: OplogIndex,
        retained: impl Fn(OplogIndex) -> bool,
    ) {
        if self.open_invocations.remove(&anchor).is_some() {
            self.closed_invocations.insert(anchor, at);
            self.owners.retain(|index, owner| {
                owner.invocation != InvocationOwner::Anchor(anchor) || retained(*index)
            });
            self.forget_released(retained);
        }
    }

    /// Drops the owners that closes kept for retained `Start`s that `retained` no longer holds,
    /// when no entity body encloses the `Start`. Without a body, a later `Start` that names such a
    /// `Start` as its parent or observational owner resolves the same way whether it counts as
    /// pruned or finds the owner. With a body, the owner keeps deciding it: the body encloses the
    /// later `Start`, whose record after the body's recorded `Cancelled` is a divergence.
    pub(super) fn forget_released(&mut self, retained: impl Fn(OplogIndex) -> bool) {
        let closed_invocations = &self.closed_invocations;
        self.owners.retain(|index, owner| {
            !matches!(
                owner.invocation,
                InvocationOwner::Anchor(anchor) if closed_invocations.contains_key(&anchor)
            ) || owner.body.is_some()
                || retained(*index)
        });
    }

    /// The number of owners the map holds.
    #[cfg(test)]
    pub(super) fn owner_count(&self) -> usize {
        self.owners.len()
    }

    /// The number of entity and custom anchors the map holds.
    #[cfg(test)]
    pub(super) fn identity_anchor_count(&self) -> usize {
        self.identity_anchors.len()
    }

    /// The number of invocations whose close the map recorded.
    #[cfg(test)]
    pub(super) fn closed_invocation_count(&self) -> usize {
        self.closed_invocations.len()
    }

    /// Drops the owners and anchors of the `Start`s committed beyond `target`. Recovery sets the
    /// target before replay commits anything, so no invocation started or closed beyond it, and
    /// none is reopened.
    pub(super) fn truncate_after(&mut self, target: OplogIndex) {
        let from = target.next();
        debug_assert!(
            self.open_invocations.range(from..).next().is_none()
                && self.closed_invocations.values().all(|at| *at <= target),
            "the replay target was lowered below a committed invocation boundary"
        );
        self.identity_anchors.split_off(&from);
        self.owners.split_off(&from);
    }
}

/// What a close does with a retained `Start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    /// Another open invocation owns it: it stays retained for that owner.
    Keep,
    /// The closing invocation owns it, and an entity body that can still consume it encloses it:
    /// the close waits for that body.
    Wait,
    /// The closing invocation owns it, and nothing can claim it anymore.
    Own,
}

impl super::CursorState {
    /// Whether the entity body started at `body` can still consume its records. It can while it
    /// replays or its `Start` is claimed ahead (active), and while its `Start` is retained and
    /// its dispatcher can still run (claimable). The dispatcher is the nearest body enclosing
    /// the entity `Start`, checked the same way, or else the primary Store of the owning
    /// invocation while that invocation is open and is not `exited`, the invocation whose
    /// primary guest tasks have exited.
    pub(super) fn body_can_still_consume(
        &self,
        reconstruction_claims: &crate::durable_host::concurrent::ReconstructionClaimState,
        body: OplogIndex,
        exited: Option<OplogIndex>,
    ) -> bool {
        let mut outermost = body;
        for body in self.enclosing_bodies(body) {
            if reconstruction_claims.is_body_active(body) || self.starts.is_claimed_ahead(&body) {
                return true;
            }
            if self.starts.get_retained(&body).is_none() {
                return false;
            }
            outermost = body;
        }
        match self.owners.anchor_owner(outermost) {
            Some(RecordOwner {
                invocation: InvocationOwner::Anchor(invocation),
                body: None,
            }) => self.owners.is_open(invocation) && exited != Some(invocation),
            Some(RecordOwner {
                invocation: InvocationOwner::AgentLevel,
                body: None,
            }) => exited.is_none(),
            _ => false,
        }
    }

    /// `body` and the entity bodies that enclose its entity `Start`, nearest first. Each body's
    /// dispatcher is the nearest body enclosing its `Start`; the walk ends at a body whose `Start`
    /// no earlier body encloses.
    fn enclosing_bodies(&self, body: OplogIndex) -> impl Iterator<Item = OplogIndex> + '_ {
        std::iter::successors(Some(body), move |body| {
            self.owners
                .anchor_owner(*body)
                .and_then(|owner| owner.body)
                .filter(|dispatcher| dispatcher < body)
        })
    }

    /// The entity body that a record under `reference` belongs to: `reference` itself when it is
    /// an entity `Start`, else the nearest body that encloses the committed `Start` at
    /// `reference`.
    pub(super) fn body_of(&self, reference: OplogIndex) -> Option<OplogIndex> {
        if self.owners.is_entity(reference) {
            return Some(reference);
        }
        self.owners
            .owner(reference)
            .or_else(|| self.owners.anchor_owner(reference))
            .and_then(|owner| owner.body)
    }

    /// Closes the open invocation started at `anchor` at the entry at `at`; the owners of its
    /// retained `Start`s stay ([`OwnerMap::close`]).
    pub(super) fn close_invocation(&mut self, anchor: OplogIndex, at: OplogIndex) {
        let starts = &self.starts;
        self.owners
            .close(anchor, at, |index| starts.get_retained(&index).is_some());
    }

    /// Drops the owners closes kept for `Start`s that are no longer retained
    /// ([`OwnerMap::forget_released`]).
    pub(super) fn forget_released(&mut self) {
        let starts = &self.starts;
        self.owners
            .forget_released(|index| starts.get_retained(&index).is_some());
    }

    /// Classifies the retained `Start`s recorded before `end` (all of them without `end`) for the
    /// close of the invocation started at `closing`, whose primary guest tasks have exited, and
    /// returns the invocation's own `Start`s. With no `closing` invocation every retained `Start`
    /// is its own. `on_wait` decides each `Start` under a body that can still claim it, and a
    /// `Start` that classifies as a divergence is reported as one. Every close of an invocation
    /// classifies its retained `Start`s here.
    pub(super) fn classify_retained(
        &self,
        reconstruction_claims: &crate::durable_host::concurrent::ReconstructionClaimState,
        closing: Option<OplogIndex>,
        end: Option<OplogIndex>,
        mut on_wait: impl FnMut(OplogIndex) -> Result<(), WorkerExecutorError>,
    ) -> Result<BTreeSet<OplogIndex>, WorkerExecutorError> {
        let mut own = BTreeSet::new();
        let below: Vec<OplogIndex> = self
            .starts
            .retained_keys()
            .copied()
            .filter(|index| end.is_none_or(|end| *index < end))
            .collect();
        for index in below {
            let class = match closing {
                Some(closing) => {
                    self.classify(reconstruction_claims, index, closing, Some(closing))
                }
                None => Ok(Class::Own),
            };
            match class {
                Ok(Class::Own) => {
                    own.insert(index);
                }
                Ok(Class::Keep) => {}
                Ok(Class::Wait) => on_wait(index)?,
                Err(divergence) => return Err(boundary_divergence(index, divergence)),
            }
        }
        Ok(own)
    }

    /// Classifies the retained `Start` at `index` for the close of the invocation started at
    /// `closing`; `exited` is the invocation whose primary guest tasks have exited. An `Err` is
    /// the divergence the `Start` is.
    fn classify(
        &self,
        reconstruction_claims: &crate::durable_host::concurrent::ReconstructionClaimState,
        index: OplogIndex,
        closing: OplogIndex,
        exited: Option<OplogIndex>,
    ) -> Result<Class, String> {
        let Some(owner) = self.owners.owner(index) else {
            return Err(format!("the Start at {index} has no recorded owner"));
        };
        match owner.invocation {
            InvocationOwner::Anchor(other) if other != closing => {
                if self.owners.is_open(other) {
                    Ok(Class::Keep)
                } else {
                    Err(format!(
                        "the Start at {index} belongs to the invocation started at {other}, which \
                         closed without claiming it"
                    ))
                }
            }
            InvocationOwner::AgentLevel if index > closing => Err(format!(
                "the Start at {index} records no invocation, but was recorded after the \
                 invocation started at {closing}"
            )),
            _ => {
                let waits = owner
                    .body
                    .into_iter()
                    .flat_map(|body| self.enclosing_bodies(body))
                    .any(|enclosing| {
                        self.body_can_still_consume(reconstruction_claims, enclosing, exited)
                    });
                Ok(if waits { Class::Wait } else { Class::Own })
            }
        }
    }
}

impl super::CursorState {
    /// The nearest entity body of a call claimed under `parent` or numbered under the anchor of
    /// `invocation_id`.
    pub(super) fn claim_nearest_body(
        &self,
        parent: Option<OplogIndex>,
        invocation_id: Option<golem_common::model::oplog::DurableInvocationId>,
    ) -> Option<OplogIndex> {
        match parent {
            Some(parent) => self.body_of(parent),
            None => invocation_id.and_then(|id| self.body_of(id.anchor)),
        }
    }

    /// The first retained `Start` whose nearest entity body is `body`.
    pub(super) fn retained_record_of_body(&self, body: OplogIndex) -> Option<OplogIndex> {
        self.starts
            .retained_keys()
            .copied()
            .find(|index| self.owners.enclosing_body(*index) == Some(body))
    }
}
