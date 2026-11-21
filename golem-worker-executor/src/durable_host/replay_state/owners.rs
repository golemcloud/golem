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

use golem_common::model::oplog::{OplogEntry, OplogIndex};
use std::collections::BTreeMap;

use super::cursor::{is_custom_invocation_start, is_entity_invocation_start};

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

/// A committed entry that identities and references can name.
#[derive(Debug, Clone, Copy)]
enum AnchorKind {
    /// An `AgentInvocationStarted`.
    Invocation,
    /// The `Start` of an entity invocation. `cancelled` is set once its recorded `Cancelled`
    /// terminal is committed.
    Entity { owner: RecordOwner, cancelled: bool },
    /// The root `Start` of a custom durable invocation.
    Custom { owner: RecordOwner },
}

/// The owners of the committed `Start`s, and the invocations that closed.
#[derive(Debug, Default)]
pub(super) struct OwnerMap {
    /// Every committed `AgentInvocationStarted`, entity `Start` and custom root `Start`.
    identity_anchors: BTreeMap<OplogIndex, AnchorKind>,
    /// The owners of the committed `Start`s of open invocations, and of retained `Start`s.
    owners: BTreeMap<OplogIndex, RecordOwner>,
    /// The invocations whose close the cursor reached, by anchor, with the index of the entry that
    /// closed them.
    closed: BTreeMap<OplogIndex, OplogIndex>,
    /// The committed `Start`s whose owners a close dropped, with the entry that closed their
    /// invocation and the owner it dropped.
    pruned: BTreeMap<OplogIndex, (OplogIndex, RecordOwner)>,
}

impl OwnerMap {
    /// The owner of the `Start` `entry` at `index`, from its recorded fields and the owners of
    /// the entries committed before it. A parent or observational owner that resolves to no owner
    /// gives way to a resolved invocation id, or with no invocation id to a valid driver, when the
    /// map pruned it at its invocation's close; any other one, such as a later entry, an entry in
    /// a deleted region or an earlier entry that is no `Start`, is an [`Orphan`].
    pub(super) fn record_owner(
        &self,
        index: OplogIndex,
        entry: &OplogEntry,
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
                    Err(orphan) if !self.pruned.contains_key(&reference) => Err(orphan),
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
            Some(driver)
                if *driver < index
                    && matches!(
                        self.identity_anchors.get(driver),
                        Some(AnchorKind::Invocation)
                    ) =>
            {
                // With no invocation id, a reference that resolves to no owner gives way to the
                // driver when the map pruned it at its invocation's close. That invocation closed,
                // so every body on the reference's chain ended: no body encloses the Start.
                let body = match recorded {
                    Ok(recorded) => recorded.body,
                    Err(_)
                        if invocation_id.is_none()
                            && reference
                                .is_some_and(|reference| self.pruned.contains_key(&reference)) =>
                    {
                        None
                    }
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
        match self.identity_anchors.get(&reference) {
            Some(AnchorKind::Invocation) => Ok(RecordOwner::invocation(reference)),
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

    /// Records the owner of the `Start` committed at `index`: [`Self::validate_start`], then
    /// [`Self::record_start`].
    #[cfg(test)]
    pub(super) fn commit_start(
        &mut self,
        index: OplogIndex,
        entry: &OplogEntry,
        invocation_open: bool,
    ) -> Result<(), String> {
        let owner = self.validate_start(index, entry, invocation_open)?;
        self.record_start(index, entry, owner);
        Ok(())
    }

    /// The owner of the `Start` at `index`, without changing the map. Returns the divergence of
    /// an [`Orphan`], of a `Start` recorded without a driver after its invocation closed, of a
    /// root `Start` with no driver or invocation id recorded while `invocation_open`, and of a
    /// `Start` recorded after the recorded `Cancelled` of its enclosing entity body.
    pub(super) fn validate_start(
        &self,
        index: OplogIndex,
        entry: &OplogEntry,
        invocation_open: bool,
    ) -> Result<RecordOwner, String> {
        let owner = self
            .record_owner(index, entry)
            .map_err(|Orphan(reason)| reason)?;
        if invocation_open && owner == RecordOwner::AGENT_LEVEL {
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

    /// Records the `AgentInvocationStarted` committed at `index`.
    pub(super) fn commit_invocation_started(&mut self, index: OplogIndex) {
        self.identity_anchors.insert(index, AnchorKind::Invocation);
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

    /// The owner of the entity or custom `Start` at `index`.
    pub(super) fn anchor_owner(&self, index: OplogIndex) -> Option<RecordOwner> {
        match self.identity_anchors.get(&index)? {
            AnchorKind::Invocation => None,
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

    /// Whether the invocation started at `anchor` closed.
    pub(super) fn is_closed(&self, anchor: OplogIndex) -> bool {
        self.closed.contains_key(&anchor)
    }

    /// Closes the invocation started at `anchor` at the entry at `at`, and drops the owners of its
    /// `Start`s except the ones `retained` still holds.
    pub(super) fn close(
        &mut self,
        anchor: OplogIndex,
        at: OplogIndex,
        retained: impl Fn(OplogIndex) -> bool,
    ) {
        self.closed.insert(anchor, at);
        let pruned = &mut self.pruned;
        self.owners.retain(|index, owner| {
            let keep = owner.invocation != InvocationOwner::Anchor(anchor) || retained(*index);
            if !keep {
                pruned.insert(*index, (at, *owner));
            }
            keep
        });
    }

    /// Drops everything recorded beyond `target`, including the closes at entries beyond it. The
    /// owners such a close pruned are owners again.
    pub(super) fn truncate_after(&mut self, target: OplogIndex) {
        let from = target.next();
        self.identity_anchors.split_off(&from);
        self.owners.split_off(&from);
        self.closed.retain(|_, at| *at <= target);
        self.pruned.split_off(&from);
        let owners = &mut self.owners;
        self.pruned.retain(|index, (at, owner)| {
            let undone = *at > target;
            if undone {
                owners.insert(*index, *owner);
            }
            !undone
        });
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
    /// Whether the invocation started at `anchor` is open: an open invocation anchor, or the
    /// invocation of the last consumed `AgentInvocationStarted` before its close.
    pub(super) fn is_open_invocation_anchor(&self, anchor: OplogIndex) -> bool {
        self.open_anchors.contains_key(&anchor)
            || (self
                .last_started_invocation
                .is_some_and(|(started, _)| started == anchor)
                && !self.owners.is_closed(anchor))
    }

    /// Whether an invocation is open.
    pub(super) fn has_open_invocation(&self) -> bool {
        !self.open_anchors.is_empty()
            || self
                .last_started_invocation
                .is_some_and(|(started, _)| !self.owners.is_closed(started))
    }

    /// Whether the entity body started at `body` can still consume its records. It can while it
    /// replays or its `Start` is claimed ahead (active), and while its `Start` is retained and
    /// its dispatcher can still run (claimable). The dispatcher is the nearest body enclosing
    /// the entity `Start`, checked the same way, or else the primary Store of the owning
    /// invocation while that invocation is open and is not `exited`, the invocation whose
    /// primary guest tasks have exited.
    pub(super) fn body_can_still_consume(
        &self,
        reconstruction_claims: &crate::durable_host::concurrent::ReconstructionClaimState,
        mut body: OplogIndex,
        exited: Option<OplogIndex>,
    ) -> bool {
        loop {
            if reconstruction_claims.is_body_active(body) || self.starts.is_claimed_ahead(&body) {
                return true;
            }
            if self.starts.get_retained(&body).is_none() {
                return false;
            }
            match self.owners.anchor_owner(body) {
                Some(RecordOwner {
                    body: Some(dispatcher),
                    ..
                }) if dispatcher < body => body = dispatcher,
                Some(RecordOwner {
                    invocation: InvocationOwner::Anchor(invocation),
                    body: None,
                }) => {
                    return self.is_open_invocation_anchor(invocation)
                        && exited != Some(invocation);
                }
                Some(RecordOwner {
                    invocation: InvocationOwner::AgentLevel,
                    body: None,
                }) => return exited.is_none(),
                _ => return false,
            }
        }
    }

    /// Classifies the retained `Start` at `index` for the close of the invocation started at
    /// `closing`; `exited` is the invocation whose primary guest tasks have exited. An `Err` is
    /// the divergence the `Start` is.
    pub(super) fn classify(
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
                if self.is_open_invocation_anchor(other) {
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
                let mut body = owner.body;
                while let Some(enclosing) = body {
                    if self.body_can_still_consume(reconstruction_claims, enclosing, exited) {
                        return Ok(Class::Wait);
                    }
                    body = self
                        .owners
                        .anchor_owner(enclosing)
                        .and_then(|owner| owner.body)
                        .filter(|dispatcher| *dispatcher < enclosing);
                }
                Ok(Class::Own)
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
        let body_of = |reference: OplogIndex| {
            if self.owners.is_entity(reference) {
                return Some(reference);
            }
            self.owners
                .owner(reference)
                .map(|owner| owner.body)
                .unwrap_or_else(|| {
                    self.owners
                        .anchor_owner(reference)
                        .and_then(|owner| owner.body)
                })
        };
        match parent {
            Some(parent) => body_of(parent),
            None => invocation_id.and_then(|id| body_of(id.anchor)),
        }
    }

    /// The first retained `Start` whose nearest entity body is `body`.
    pub(super) fn retained_record_of_body(&self, body: OplogIndex) -> Option<OplogIndex> {
        self.starts.retained_keys().copied().find(|index| {
            self.owners
                .owner(*index)
                .is_some_and(|owner| owner.body == Some(body))
        })
    }
}
