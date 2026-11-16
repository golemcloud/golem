use super::{QuotaClass, ReplayEvent, RetainedStart};
use crate::durable_host::concurrent::{
    HistoricalReconstruction, ReconstructionClaimState, ReplayableOneshot, Resolution,
    ResolutionOutcome,
};
use crate::worker::owner_wallet::OwnerWallet;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Weak};
use tokio::sync::oneshot;

/// Per-Start replay obligations, mutated only under the replay cursor lock.
#[derive(Debug, Default)]
pub(crate) struct ReplayStarts {
    entries: BTreeMap<OplogIndex, ReplayStart>,
    /// Derived index containing only the keys of `Retained` entries.
    retained_indices: BTreeSet<OplogIndex>,
    retained_summary: RetainedSummary,
    reconstruction_claims: Arc<ReconstructionClaimState>,
}

/// Derived from retained payloads at registry mutations, never from active claims.
#[derive(Debug, Default)]
pub(super) struct RetainedSummary {
    pub(super) total: usize,
    pub(super) http: usize,
    pub(super) rpc: usize,
    pub(super) entities: HashMap<OplogIndex, usize>,
}

impl RetainedSummary {
    fn quota_count(&mut self, retained: &RetainedStart) -> Option<&mut usize> {
        let OplogEntry::Start { function_name, .. } = &retained.entry else {
            unreachable!("only Start entries are retained");
        };
        match QuotaClass::of_recorded_call(function_name) {
            Some(QuotaClass::Http) => Some(&mut self.http),
            Some(QuotaClass::Rpc) => Some(&mut self.rpc),
            None => None,
        }
    }

    fn add(&mut self, retained: &RetainedStart) {
        self.total += 1;
        if let Some(count) = self.quota_count(retained) {
            *count += 1;
        }
        for entity in &retained.enclosing_entities {
            *self.entities.entry(*entity).or_default() += 1;
        }
    }

    fn remove(&mut self, retained: &RetainedStart) {
        self.total -= 1;
        if let Some(count) = self.quota_count(retained) {
            *count -= 1;
        }
        for entity in &retained.enclosing_entities {
            let count = self.entities.get_mut(entity).unwrap();
            *count -= 1;
            if *count == 0 {
                self.entities.remove(entity);
            }
        }
    }
}

#[derive(Debug)]
enum ReplayStart {
    // Retained history is much larger than a sender; do not charge every active claim for it.
    Retained(Box<RetainedStart>),
    Claimed {
        position: ClaimPosition,
        terminal: ClaimTerminal,
    },
    /// An ahead claim whose registration failed or whose sender was explicitly unregistered.
    UnregisteredAhead,
}

#[derive(Debug, Clone, Copy)]
enum ClaimPosition {
    AheadOfCursor,
    Consumed,
}

#[derive(Debug)]
enum ClaimTerminal {
    Waiting(PendingClaim),
    Prefetched { terminal_index: OplogIndex },
}

#[derive(Debug)]
struct PendingClaim {
    tx: ReplayableOneshot<ResolutionOutcome>,
    parent_start_index: Option<OplogIndex>,
}

impl ReplayStarts {
    pub(crate) fn claim_ahead(&mut self, index: OplogIndex) -> Result<(), WorkerExecutorError> {
        match self.entries.entry(index) {
            Entry::Vacant(entry) => {
                entry.insert(ReplayStart::UnregisteredAhead);
                Ok(())
            }
            Entry::Occupied(_) => Err(Self::duplicate_claim(index)),
        }
    }

    fn duplicate_claim(index: OplogIndex) -> WorkerExecutorError {
        WorkerExecutorError::runtime(format!("duplicate replay claim for Start at {index}"))
    }

    pub(crate) fn is_claimed_ahead(&self, index: &OplogIndex) -> bool {
        matches!(
            self.entries.get(index),
            Some(ReplayStart::UnregisteredAhead)
                | Some(ReplayStart::Claimed {
                    position: ClaimPosition::AheadOfCursor,
                    ..
                })
        )
    }

    pub(crate) fn consume_claimed_start(&mut self, index: &OplogIndex) -> bool {
        match self.entries.get_mut(index) {
            Some(ReplayStart::Claimed {
                position: position @ ClaimPosition::AheadOfCursor,
                ..
            }) => {
                *position = ClaimPosition::Consumed;
                true
            }
            Some(ReplayStart::UnregisteredAhead) => {
                self.entries.remove(index);
                true
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn has_claimed_ahead(&self) -> bool {
        self.entries
            .keys()
            .any(|index| self.is_claimed_ahead(index))
    }

    pub(crate) fn register(
        &mut self,
        start_idx: OplogIndex,
        parent_start_index: Option<OplogIndex>,
    ) -> Result<oneshot::Receiver<ResolutionOutcome>, WorkerExecutorError> {
        let position = match self.entries.get(&start_idx) {
            Some(ReplayStart::UnregisteredAhead) => ClaimPosition::AheadOfCursor,
            None => ClaimPosition::Consumed,
            Some(_) => return Err(Self::duplicate_claim(start_idx)),
        };
        let (tx, rx) = oneshot::channel();
        self.entries.insert(
            start_idx,
            ReplayStart::Claimed {
                position,
                terminal: ClaimTerminal::Waiting(PendingClaim {
                    tx,
                    parent_start_index,
                }),
            },
        );
        Ok(rx)
    }

    fn waiting(&self, start_idx: OplogIndex) -> Option<&PendingClaim> {
        match self.entries.get(&start_idx) {
            Some(ReplayStart::Claimed {
                terminal: ClaimTerminal::Waiting(claim),
                ..
            }) => Some(claim),
            _ => None,
        }
    }

    pub(crate) fn resolve_if_pending(
        &mut self,
        start_idx: OplogIndex,
        terminal_idx: OplogIndex,
        resolution: Resolution,
    ) -> bool {
        if !self.owns_terminal(start_idx, terminal_idx) {
            return false;
        }
        if let Some(ReplayStart::Claimed {
            terminal: ClaimTerminal::Waiting(claim),
            ..
        }) = self.entries.remove(&start_idx)
        {
            let _ = claim.tx.send(ResolutionOutcome::Resolved(resolution));
        }
        true
    }

    pub(crate) fn resolve_prefetched(
        &mut self,
        start_idx: OplogIndex,
        terminal_idx: OplogIndex,
        resolution: Resolution,
    ) {
        let Some(ReplayStart::Claimed { terminal, .. }) = self.entries.get_mut(&start_idx) else {
            panic!("a completion is prefetched only while registering its claimed Start");
        };
        assert!(matches!(terminal, ClaimTerminal::Waiting(_)));
        let ClaimTerminal::Waiting(claim) = std::mem::replace(
            terminal,
            ClaimTerminal::Prefetched {
                terminal_index: terminal_idx,
            },
        ) else {
            unreachable!();
        };
        let _ = claim.tx.send(ResolutionOutcome::Resolved(resolution));
    }

    /// Explicit finalization only. Natural cursor exhaustion must not discard obligations.
    pub(crate) fn fail_all_pending_incomplete(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        for (index, entry) in entries {
            match entry {
                ReplayStart::Retained(retained) => {
                    self.entries.insert(index, ReplayStart::Retained(retained));
                }
                ReplayStart::Claimed {
                    terminal: ClaimTerminal::Waiting(claim),
                    ..
                } => {
                    self.reconstruction_claims.release_incomplete(index);
                    let _ = claim.tx.send(ResolutionOutcome::Incomplete);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn unregister(&mut self, start_idx: OplogIndex) {
        if let Some(ReplayStart::Claimed {
            position,
            terminal: ClaimTerminal::Waiting(_),
            ..
        }) = self.entries.get(&start_idx)
        {
            match position {
                ClaimPosition::AheadOfCursor => {
                    self.entries
                        .insert(start_idx, ReplayStart::UnregisteredAhead);
                }
                ClaimPosition::Consumed => {
                    self.entries.remove(&start_idx);
                }
            }
        }
    }

    pub(crate) fn unregister_incomplete(&mut self, start_idx: OplogIndex) {
        self.unregister(start_idx);
        self.reconstruction_claims.release_incomplete(start_idx);
    }

    pub(crate) fn owns_terminal(&self, start_idx: OplogIndex, terminal_idx: OplogIndex) -> bool {
        match self.entries.get(&start_idx) {
            Some(ReplayStart::Claimed {
                terminal: ClaimTerminal::Waiting(_),
                ..
            }) => true,
            Some(ReplayStart::Claimed {
                terminal: ClaimTerminal::Prefetched { terminal_index },
                ..
            }) => *terminal_index == terminal_idx,
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn is_pending(&self, start_idx: OplogIndex) -> bool {
        self.waiting(start_idx).is_some()
    }

    pub(crate) fn claimed_parent(&self, start_idx: OplogIndex) -> Option<OplogIndex> {
        self.waiting(start_idx)
            .and_then(|claim| claim.parent_start_index)
    }

    pub(crate) fn has_claim(&self, start_idx: OplogIndex) -> bool {
        matches!(
            self.entries.get(&start_idx),
            Some(ReplayStart::Claimed { .. })
        )
    }

    pub(crate) fn is_awaited(&self, start_idx: OplogIndex) -> bool {
        self.waiting(start_idx)
            .is_some_and(|claim| !claim.tx.is_closed())
    }

    pub(crate) fn reconstruction_claims(&self) -> Arc<ReconstructionClaimState> {
        self.reconstruction_claims.clone()
    }

    pub(crate) fn register_reconstruction(
        &self,
        start_index: OplogIndex,
        wallet: Option<Weak<OwnerWallet>>,
    ) -> HistoricalReconstruction {
        self.reconstruction_claims.register(start_index, wallet)
    }

    pub(crate) fn pending_reconstruction_starts(&self) -> HashSet<OplogIndex> {
        self.reconstruction_claims
            .subscribe_fences()
            .borrow()
            .iter()
            .filter(|index| self.waiting(**index).is_some())
            .copied()
            .collect()
    }

    pub(crate) fn only_pending_reconstruction_fences_remain(
        &self,
        incomplete: &HashSet<OplogIndex>,
    ) -> bool {
        self.reconstruction_claims
            .subscribe_fences()
            .borrow()
            .iter()
            .all(|index| incomplete.contains(index) && self.waiting(*index).is_some())
    }

    pub(super) fn retained_iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (&OplogIndex, &RetainedStart)> {
        self.retained_indices
            .iter()
            .map(|index| (index, self.get_retained(index).unwrap()))
    }

    pub(super) fn retained_keys(&self) -> impl DoubleEndedIterator<Item = &OplogIndex> {
        self.retained_indices.iter()
    }

    pub(super) fn retained_values(&self) -> impl DoubleEndedIterator<Item = &RetainedStart> {
        self.retained_iter().map(|(_, retained)| retained)
    }

    pub(super) fn retained_len(&self) -> usize {
        self.retained_indices.len()
    }

    pub(super) fn retained_summary(&self) -> &RetainedSummary {
        &self.retained_summary
    }

    pub(super) fn get_retained(&self, index: &OplogIndex) -> Option<&RetainedStart> {
        match self.entries.get(index) {
            Some(ReplayStart::Retained(retained)) => Some(retained),
            _ => None,
        }
    }

    pub(super) fn get_retained_terminal_mut(
        &mut self,
        index: &OplogIndex,
    ) -> Option<&mut Option<(OplogIndex, OplogEntry)>> {
        match self.entries.get_mut(index) {
            Some(ReplayStart::Retained(retained)) => Some(&mut retained.terminal),
            _ => None,
        }
    }

    pub(super) fn get_retained_deferred_events_mut(
        &mut self,
        index: &OplogIndex,
    ) -> Option<&mut Vec<(OplogIndex, ReplayEvent)>> {
        match self.entries.get_mut(index) {
            Some(ReplayStart::Retained(retained)) => Some(&mut retained.deferred_events),
            _ => None,
        }
    }

    pub(super) fn insert_retained(&mut self, index: OplogIndex, retained: RetainedStart) {
        assert!(
            !self.entries.contains_key(&index),
            "retained Start at {index} already exists"
        );
        self.retained_summary.add(&retained);
        self.entries
            .insert(index, ReplayStart::Retained(Box::new(retained)));
        self.retained_indices.insert(index);
    }

    pub(super) fn remove_retained(&mut self, index: &OplogIndex) -> Option<RetainedStart> {
        if !self.retained_indices.remove(index) {
            return None;
        }
        let Some(ReplayStart::Retained(retained)) = self.entries.remove(index) else {
            unreachable!("retained index contains only Retained keys");
        };
        self.retained_summary.remove(&retained);
        Some(*retained)
    }

    pub(super) fn retain_retained(
        &mut self,
        mut keep: impl FnMut(&OplogIndex, &mut RetainedStart) -> bool,
    ) {
        let indices: Vec<_> = self.retained_indices.iter().copied().collect();
        for index in indices {
            let Some(ReplayStart::Retained(retained)) = self.entries.get_mut(&index) else {
                unreachable!("retained index contains only Retained keys");
            };
            self.retained_summary.remove(retained);
            if keep(&index, retained) {
                self.retained_summary.add(retained);
            } else {
                self.retained_indices.remove(&index);
                self.entries.remove(&index);
            }
        }
    }

    pub(super) fn take_retained(&mut self) -> BTreeMap<OplogIndex, RetainedStart> {
        let indices = std::mem::take(&mut self.retained_indices);
        self.retained_summary = RetainedSummary::default();
        indices
            .into_iter()
            .map(|index| {
                let Some(ReplayStart::Retained(retained)) = self.entries.remove(&index) else {
                    unreachable!("retained index contains only Retained keys");
                };
                (index, *retained)
            })
            .collect()
    }
}
