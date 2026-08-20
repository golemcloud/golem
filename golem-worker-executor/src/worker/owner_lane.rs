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

use golem_common::model::OwnedAgentId;
pub use golem_common::model::entity::EntityCallMode;
use golem_common::model::entity::{EntityInvocationId, FilesystemCapability};
use golem_common::model::oplog::OplogIndex;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, watch};

/// One invocation participating in an owner's causal execution graph.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum OwnerInvocationId {
    Agent(OplogIndex),
    Entity(EntityInvocationId),
}

impl OwnerInvocationId {
    pub fn start_index(&self) -> OplogIndex {
        match self {
            Self::Agent(start_index) => *start_index,
            Self::Entity(invocation_id) => invocation_id.start_index(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum OwnerLaneError {
    InactiveInvocation(OwnerInvocationId),
    DuplicateInvocation(OwnerInvocationId),
    WrongOwner(EntityInvocationId, OwnedAgentId),
    InvalidStartIndex,
    WouldDeadlock { caller: OwnerInvocationId },
}

impl Display for OwnerLaneError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InactiveInvocation(invocation) => {
                write!(f, "owner invocation {invocation:?} is not active")
            }
            Self::DuplicateInvocation(invocation) => {
                write!(f, "owner invocation {invocation:?} is already active")
            }
            Self::WrongOwner(invocation, owner) => {
                write!(
                    f,
                    "entity invocation {invocation} does not belong to owner {owner}"
                )
            }
            Self::InvalidStartIndex => f.write_str("owner invocation Start index cannot be zero"),
            Self::WouldDeadlock { caller } => write!(
                f,
                "synchronous call from entity invocation {caller:?} back into its blocked owner would deadlock"
            ),
        }
    }
}

impl std::error::Error for OwnerLaneError {}

/// Serializes filesystem-capable guest bodies at causal invocation boundaries.
///
/// Filesystem-incapable invocations receive an immediate off-lane permit. Capable asynchronous
/// invocations remain queued until their caller awaits them or their caller ends. The graph of
/// synchronous and awaited calls lets the lane move through off-lane callers without relying on
/// task scheduling order.
#[derive(Clone)]
pub struct OwnerLane {
    inner: Arc<OwnerLaneInner>,
}

struct OwnerLaneInner {
    owner_id: OwnedAgentId,
    state: Mutex<OwnerLaneState>,
    changed: watch::Sender<u64>,
}

#[derive(Default)]
struct OwnerLaneState {
    invocations: HashMap<OwnerInvocationId, LaneInvocation>,
    holder: Option<OwnerInvocationId>,
    exclusive: bool,
}

struct LaneInvocation {
    parent: Option<OwnerInvocationId>,
    lineage: BTreeSet<OwnerInvocationId>,
    filesystem: FilesystemCapability,
    eligible: bool,
    running: bool,
    blocked_on: BTreeSet<OwnerInvocationId>,
    grant: Option<oneshot::Sender<OwnerInvocationPermit>>,
}

impl OwnerLane {
    pub fn new(owner_id: OwnedAgentId) -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            inner: Arc::new(OwnerLaneInner {
                owner_id,
                state: Mutex::new(OwnerLaneState::default()),
                changed,
            }),
        }
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        &self.inner.owner_id
    }

    /// Enters a primary invocation, which is always filesystem-capable.
    pub fn enter_primary(
        &self,
        start_index: OplogIndex,
    ) -> Result<OwnerInvocationTicket, OwnerLaneError> {
        if start_index == OplogIndex::NONE {
            return Err(OwnerLaneError::InvalidStartIndex);
        }
        self.register(
            OwnerInvocationId::Agent(start_index),
            None,
            EntityCallMode::Synchronous,
            FilesystemCapability::Capable,
            true,
        )
    }

    pub fn register_entity(
        &self,
        parent: OwnerInvocationId,
        invocation_id: EntityInvocationId,
        mode: EntityCallMode,
        filesystem: FilesystemCapability,
    ) -> Result<OwnerInvocationTicket, OwnerLaneError> {
        if invocation_id.owner_id() != &self.inner.owner_id {
            return Err(OwnerLaneError::WrongOwner(
                invocation_id,
                self.inner.owner_id.clone(),
            ));
        }
        let invocation = OwnerInvocationId::Entity(invocation_id);
        self.register(
            invocation,
            Some(parent),
            mode,
            filesystem,
            mode == EntityCallMode::Synchronous,
        )
    }

    fn register(
        &self,
        invocation: OwnerInvocationId,
        parent: Option<OwnerInvocationId>,
        mode: EntityCallMode,
        filesystem: FilesystemCapability,
        eligible: bool,
    ) -> Result<OwnerInvocationTicket, OwnerLaneError> {
        let (grant_tx, grant_rx) = oneshot::channel();
        {
            let mut state = self.inner.state.lock().unwrap();
            if state.invocations.contains_key(&invocation) {
                return Err(OwnerLaneError::DuplicateInvocation(invocation));
            }
            let lineage = if let Some(parent) = &parent {
                let mut lineage = state
                    .invocations
                    .get(parent)
                    .ok_or_else(|| OwnerLaneError::InactiveInvocation(parent.clone()))?
                    .lineage
                    .clone();
                lineage.insert(parent.clone());
                lineage
            } else {
                BTreeSet::new()
            };
            if let Some(parent) = &parent {
                let parent_state = state
                    .invocations
                    .get_mut(parent)
                    .expect("parent existence was validated above");
                if mode == EntityCallMode::Synchronous {
                    parent_state.blocked_on.insert(invocation.clone());
                }
            }

            state.invocations.insert(
                invocation.clone(),
                LaneInvocation {
                    parent,
                    lineage,
                    filesystem,
                    eligible,
                    running: filesystem == FilesystemCapability::Incapable,
                    blocked_on: BTreeSet::new(),
                    grant: Some(grant_tx),
                },
            );
        }

        let ticket = OwnerInvocationTicket {
            lane: self.clone(),
            invocation: invocation.clone(),
            grant: Some(grant_rx),
            acquired: false,
        };
        if filesystem == FilesystemCapability::Incapable {
            self.inner.send_off_lane_grant(&invocation);
        } else {
            self.inner.try_grant();
        }
        Ok(ticket)
    }

    /// Marks one or more initiated invocations as causally awaited by `caller`.
    ///
    /// A batch models a poll over several futures. All become eligible at the same point and the
    /// lane selects them by durable Start index rather than registration or scheduler order.
    pub fn await_invocations(
        &self,
        caller: &OwnerInvocationId,
        invocations: impl IntoIterator<Item = OwnerInvocationId>,
    ) -> Result<OwnerLaneWait, OwnerLaneError> {
        let mut pending_roots = BTreeSet::new();
        let return_holder;
        {
            let mut state = self.inner.state.lock().unwrap();
            if !state.invocations.contains_key(caller) {
                return Err(OwnerLaneError::InactiveInvocation(caller.clone()));
            }
            let invocations = invocations.into_iter().collect::<Vec<_>>();
            for invocation in &invocations {
                if !state.invocations.contains_key(invocation) {
                    return Err(OwnerLaneError::InactiveInvocation(invocation.clone()));
                }
            }
            for invocation in invocations {
                if state.invocations[&invocation].filesystem == FilesystemCapability::Capable {
                    pending_roots.insert(invocation.clone());
                }
                state.invocations.get_mut(&invocation).unwrap().eligible = true;
                state
                    .invocations
                    .get_mut(caller)
                    .unwrap()
                    .blocked_on
                    .insert(invocation);
            }
            return_holder = state.holder.clone().filter(|holder| {
                pending_roots
                    .iter()
                    .any(|invocation| state.blocking_reaches(holder, invocation))
            });
        }
        self.inner.try_grant();
        Ok(OwnerLaneWait {
            lane: self.inner.clone(),
            return_holder,
            pending_roots,
        })
    }

    /// Rejects only the owner reentrancy shape whose primary invocation is transitively blocked on
    /// `caller`. Entity-to-entity self calls remain ordinary graph nodes.
    pub fn ensure_synchronous_owner_call(
        &self,
        caller: &OwnerInvocationId,
    ) -> Result<(), OwnerLaneError> {
        let state = self.inner.state.lock().unwrap();
        if !state.invocations.contains_key(caller) {
            return Err(OwnerLaneError::InactiveInvocation(caller.clone()));
        }

        let mut ancestor = Some(caller.clone());
        while let Some(current) = ancestor {
            let Some(invocation) = state.invocations.get(&current) else {
                break;
            };
            if matches!(current, OwnerInvocationId::Agent(_))
                && state.blocking_reaches(&current, caller)
            {
                return Err(OwnerLaneError::WouldDeadlock {
                    caller: caller.clone(),
                });
            }
            ancestor = invocation.parent.clone();
        }
        Ok(())
    }

    pub fn holder(&self) -> Option<OwnerInvocationId> {
        self.inner.state.lock().unwrap().holder.clone()
    }

    /// Waits until no filesystem-capable body owns the lane, then prevents a new body from being
    /// granted until the returned guard is dropped. Executor-side filesystem inspection and root
    /// generation changes use this instead of a second mutex, so causal lane transfer through a
    /// blocking entity chain remains deadlock-free.
    pub async fn acquire_exclusive(&self) -> OwnerLaneExclusiveGuard {
        let mut changed = self.inner.changed.subscribe();
        loop {
            let acquired = {
                let mut state = self.inner.state.lock().unwrap();
                if state.holder.is_none() && !state.exclusive {
                    state.exclusive = true;
                    true
                } else {
                    false
                }
            };
            if acquired {
                return OwnerLaneExclusiveGuard {
                    lane: self.inner.clone(),
                };
            }
            if changed.changed().await.is_err() {
                continue;
            }
        }
    }
}

impl OwnerLaneInner {
    fn send_off_lane_grant(self: &Arc<Self>, invocation: &OwnerInvocationId) {
        let grant = self
            .state
            .lock()
            .unwrap()
            .invocations
            .get_mut(invocation)
            .and_then(|invocation| invocation.grant.take());
        if let Some(grant) = grant
            && grant
                .send(OwnerInvocationPermit {
                    lane: self.clone(),
                    invocation: invocation.clone(),
                    filesystem: FilesystemCapability::Incapable,
                    completed: false,
                })
                .is_err()
        {
            self.complete(invocation);
        }
    }

    fn try_grant(self: &Arc<Self>) {
        loop {
            let grant = {
                let mut state = self.state.lock().unwrap();
                if state.exclusive {
                    return;
                }
                let candidate = state.next_capable_candidate();
                candidate.and_then(|candidate| {
                    state.holder = Some(candidate.clone());
                    let invocation = state.invocations.get_mut(&candidate).unwrap();
                    invocation.running = true;
                    invocation.grant.take().map(|grant| (candidate, grant))
                })
            };

            if grant.is_some() {
                self.signal_change();
            }

            let Some((invocation, grant)) = grant else {
                return;
            };
            let permit = OwnerInvocationPermit {
                lane: self.clone(),
                invocation: invocation.clone(),
                filesystem: FilesystemCapability::Capable,
                completed: false,
            };
            if grant.send(permit).is_ok() {
                return;
            }
            self.complete(&invocation);
        }
    }

    fn cancel_queued(self: &Arc<Self>, invocation: &OwnerInvocationId) {
        let queued = self
            .state
            .lock()
            .unwrap()
            .invocations
            .get(invocation)
            .is_some_and(|invocation| !invocation.running);
        if queued {
            self.complete(invocation);
        }
    }

    fn complete(self: &Arc<Self>, invocation: &OwnerInvocationId) {
        let _ = self.complete_with_wait(invocation);
    }

    fn complete_with_wait(
        self: &Arc<Self>,
        invocation: &OwnerInvocationId,
    ) -> Option<LaneCompletionWait> {
        let wait = {
            let mut state = self.state.lock().unwrap();
            let completed = state.invocations.remove(invocation)?;

            let pending_capable_children = state
                .invocations
                .iter()
                .filter(|&(_id, child)| {
                    child.parent.as_ref() == Some(invocation)
                        && child.filesystem == FilesystemCapability::Capable
                        && !child.running
                })
                .map(|(id, _child)| id.clone())
                .collect::<BTreeSet<_>>();
            for child in state.invocations.values_mut() {
                if child.parent.as_ref() == Some(invocation) {
                    child.eligible = true;
                    child.parent = completed.parent.clone();
                }
            }

            let return_holder = state.holder.as_ref().and_then(|holder| {
                if holder == invocation {
                    completed
                        .parent
                        .as_ref()
                        .and_then(|parent| state.nearest_capable_ancestor(parent))
                } else if state.blocking_reaches(holder, invocation) {
                    Some(holder.clone())
                } else {
                    None
                }
            });
            for active in state.invocations.values_mut() {
                active.blocked_on.remove(invocation);
            }

            if state.holder.as_ref() == Some(invocation) {
                state.holder = return_holder.clone();
            }
            if let Some(return_holder) = &return_holder
                && !pending_capable_children.is_empty()
            {
                state
                    .invocations
                    .get_mut(return_holder)
                    .expect("return holder must remain active")
                    .blocked_on
                    .extend(pending_capable_children.iter().cloned());
            }

            return_holder
                .filter(|_| !pending_capable_children.is_empty())
                .map(|return_holder| LaneCompletionWait {
                    return_holder,
                    pending_children: pending_capable_children,
                })
        };
        self.signal_change();
        self.try_grant();
        wait
    }

    async fn wait_for_completion_return(&self, wait: OwnerLaneWaitState) {
        let mut changed = self.changed.subscribe();
        loop {
            let ready = {
                let state = self.state.lock().unwrap();
                !state.invocations.contains_key(&wait.return_holder)
                    || (state.holder.as_ref() == Some(&wait.return_holder)
                        && wait.pending_roots.iter().all(|child| {
                            !state.invocations.iter().any(|(active_id, active)| {
                                active_id == child || active.lineage.contains(child)
                            })
                        }))
            };
            if ready {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    fn signal_change(&self) {
        self.changed.send_modify(|generation| {
            *generation = generation.wrapping_add(1);
        });
    }
}

/// Exclusive executor-side access to the owner filesystem lane.
pub struct OwnerLaneExclusiveGuard {
    lane: Arc<OwnerLaneInner>,
}

impl Drop for OwnerLaneExclusiveGuard {
    fn drop(&mut self) {
        {
            let mut state = self.lane.state.lock().unwrap();
            debug_assert!(state.exclusive, "owner lane exclusive guard was not active");
            state.exclusive = false;
        }
        self.lane.signal_change();
        self.lane.try_grant();
    }
}

struct LaneCompletionWait {
    return_holder: OwnerInvocationId,
    pending_children: BTreeSet<OwnerInvocationId>,
}

struct OwnerLaneWaitState {
    return_holder: OwnerInvocationId,
    pending_roots: BTreeSet<OwnerInvocationId>,
}

/// Causal barrier returned when a set of filesystem-capable invocations becomes eligible.
/// Polling code waits on this after processing ready results, before returning control to a guest
/// that previously held the lane.
pub struct OwnerLaneWait {
    lane: Arc<OwnerLaneInner>,
    return_holder: Option<OwnerInvocationId>,
    pending_roots: BTreeSet<OwnerInvocationId>,
}

impl OwnerLaneWait {
    pub async fn wait(self) {
        if let Some(return_holder) = self.return_holder
            && !self.pending_roots.is_empty()
        {
            self.lane
                .wait_for_completion_return(OwnerLaneWaitState {
                    return_holder,
                    pending_roots: self.pending_roots,
                })
                .await;
        }
    }
}

impl OwnerLaneState {
    fn next_capable_candidate(&self) -> Option<OwnerInvocationId> {
        let candidates = if let Some(holder) = &self.holder {
            let holder_state = self.invocations.get(holder)?;
            if holder_state.blocked_on.is_empty() {
                return None;
            }
            let mut reachable = HashSet::new();
            self.collect_blocked_descendants(holder, &mut reachable);
            reachable
        } else {
            self.invocations.keys().cloned().collect()
        };

        candidates
            .into_iter()
            .filter(|candidate| {
                self.invocations.get(candidate).is_some_and(|invocation| {
                    invocation.filesystem == FilesystemCapability::Capable
                        && invocation.eligible
                        && !invocation.running
                })
            })
            .min_by_key(|candidate| (candidate.start_index(), candidate.clone()))
    }

    fn collect_blocked_descendants(
        &self,
        invocation: &OwnerInvocationId,
        result: &mut HashSet<OwnerInvocationId>,
    ) {
        let Some(invocation) = self.invocations.get(invocation) else {
            return;
        };
        for blocked in &invocation.blocked_on {
            if result.insert(blocked.clone())
                && self
                    .invocations
                    .get(blocked)
                    .is_some_and(|blocked| blocked.running)
            {
                self.collect_blocked_descendants(blocked, result);
            }
        }
    }

    fn blocking_reaches(&self, from: &OwnerInvocationId, target: &OwnerInvocationId) -> bool {
        let mut reachable = HashSet::new();
        self.collect_blocked_descendants(from, &mut reachable);
        reachable.contains(target)
    }

    fn nearest_capable_ancestor(
        &self,
        invocation: &OwnerInvocationId,
    ) -> Option<OwnerInvocationId> {
        let mut current = Some(invocation.clone());
        while let Some(candidate) = current {
            let active = self.invocations.get(&candidate)?;
            if active.filesystem == FilesystemCapability::Capable && active.running {
                return Some(candidate);
            }
            current = active.parent.clone();
        }
        None
    }
}

pub struct OwnerInvocationTicket {
    lane: OwnerLane,
    invocation: OwnerInvocationId,
    grant: Option<oneshot::Receiver<OwnerInvocationPermit>>,
    acquired: bool,
}

impl OwnerInvocationTicket {
    pub fn invocation(&self) -> &OwnerInvocationId {
        &self.invocation
    }

    pub async fn acquire(mut self) -> Result<OwnerInvocationPermit, OwnerLaneError> {
        let grant = self
            .grant
            .take()
            .expect("owner invocation ticket can only be acquired once")
            .await
            .map_err(|_| OwnerLaneError::InactiveInvocation(self.invocation.clone()))?;
        self.acquired = true;
        Ok(grant)
    }
}

impl Drop for OwnerInvocationTicket {
    fn drop(&mut self) {
        if !self.acquired {
            self.lane.inner.cancel_queued(&self.invocation);
        }
    }
}

/// Lifetime guard for an active invocation. For a capable invocation it also represents exclusive
/// ownership of the filesystem lane; for an incapable invocation it only closes graph ancestry at
/// the body terminal.
pub struct OwnerInvocationPermit {
    lane: Arc<OwnerLaneInner>,
    invocation: OwnerInvocationId,
    filesystem: FilesystemCapability,
    completed: bool,
}

impl OwnerInvocationPermit {
    pub fn invocation(&self) -> &OwnerInvocationId {
        &self.invocation
    }

    pub fn filesystem(&self) -> FilesystemCapability {
        self.filesystem
    }

    pub fn complete(mut self) {
        self.lane.complete(&self.invocation);
        self.completed = true;
    }

    pub async fn complete_and_wait(mut self) {
        let wait = self.lane.complete_with_wait(&self.invocation);
        self.completed = true;
        if let Some(wait) = wait {
            self.lane
                .wait_for_completion_return(OwnerLaneWaitState {
                    return_holder: wait.return_holder,
                    pending_roots: wait.pending_children,
                })
                .await;
        }
    }
}

impl Drop for OwnerInvocationPermit {
    fn drop(&mut self) {
        if !self.completed {
            self.lane.complete(&self.invocation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::AgentId;
    use golem_common::model::component::ComponentId;
    use golem_common::model::entity::{AgentEntity, OwnedAgentEntityId};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::tool::ToolName;
    use test_r::test;

    fn lane() -> OwnerLane {
        OwnerLane::new(OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: "owner".to_string(),
            },
        ))
    }

    fn entity(lane: &OwnerLane, name: &str, start: u64) -> EntityInvocationId {
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: lane.owner_id().clone(),
                entity: AgentEntity::Tool(ToolName::try_from(name).unwrap()),
            },
            OplogIndex::from_u64(start),
        )
        .unwrap()
    }

    #[test]
    async fn incapable_invocations_run_off_lane_while_primary_holds_it() {
        let lane = lane();
        let primary_id = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let _primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let first = lane
            .register_entity(
                primary_id.clone(),
                entity(&lane, "first", 2),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Incapable,
            )
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let second = lane
            .register_entity(
                primary_id.clone(),
                entity(&lane, "second", 3),
                EntityCallMode::FireAndForget,
                FilesystemCapability::Incapable,
            )
            .unwrap()
            .acquire()
            .await
            .unwrap();

        assert_eq!(lane.holder(), Some(primary_id));
        assert_eq!(first.filesystem(), FilesystemCapability::Incapable);
        assert_eq!(second.filesystem(), FilesystemCapability::Incapable);
    }

    #[test]
    async fn capable_async_body_waits_for_its_causal_await() {
        let lane = lane();
        let primary_id = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let _primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let invocation_id = entity(&lane, "async-tool", 2);
        let ticket = lane
            .register_entity(
                primary_id.clone(),
                invocation_id.clone(),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Capable,
            )
            .unwrap();
        let mut acquire = Box::pin(ticket.acquire());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut acquire)
                .await
                .is_err()
        );

        lane.await_invocations(
            &primary_id,
            [OwnerInvocationId::Entity(invocation_id.clone())],
        )
        .unwrap();
        let entity = acquire.await.unwrap();
        assert_eq!(
            lane.holder(),
            Some(OwnerInvocationId::Entity(invocation_id))
        );
        drop(entity);
        assert_eq!(lane.holder(), Some(primary_id));
    }

    #[test]
    async fn simultaneous_eligibility_uses_ascending_start_order() {
        let lane = lane();
        let primary_id = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let later_id = entity(&lane, "later", 30);
        let earlier_id = entity(&lane, "earlier", 20);
        let later = lane
            .register_entity(
                primary_id.clone(),
                later_id.clone(),
                EntityCallMode::FireAndForget,
                FilesystemCapability::Capable,
            )
            .unwrap();
        let earlier = lane
            .register_entity(
                primary_id,
                earlier_id.clone(),
                EntityCallMode::FireAndForget,
                FilesystemCapability::Capable,
            )
            .unwrap();

        drop(primary);
        let earlier = tokio::time::timeout(std::time::Duration::from_secs(1), earlier.acquire())
            .await
            .unwrap()
            .unwrap();
        let mut later = Box::pin(later.acquire());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut later)
                .await
                .is_err()
        );
        assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(earlier_id)));
        drop(earlier);
        let later = later.await.unwrap();
        assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(later_id)));
        drop(later);
        assert_eq!(lane.holder(), None);
    }

    #[test]
    async fn batched_await_regains_lane_after_all_capable_roots_but_not_off_lane_roots() {
        let lane = lane();
        let primary_id = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let _primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let first_id = entity(&lane, "first-batch", 2);
        let first = lane
            .register_entity(
                primary_id.clone(),
                first_id.clone(),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Capable,
            )
            .unwrap();
        let second_id = entity(&lane, "second-batch", 3);
        let second = lane
            .register_entity(
                primary_id.clone(),
                second_id.clone(),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Capable,
            )
            .unwrap();
        let off_lane_id = entity(&lane, "off-lane-batch", 4);
        let off_lane = lane
            .register_entity(
                primary_id.clone(),
                off_lane_id.clone(),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Incapable,
            )
            .unwrap()
            .acquire()
            .await
            .unwrap();

        let wait = lane
            .await_invocations(
                &primary_id,
                [
                    OwnerInvocationId::Entity(second_id),
                    OwnerInvocationId::Entity(off_lane_id),
                    OwnerInvocationId::Entity(first_id),
                ],
            )
            .unwrap();
        let mut wait = Box::pin(wait.wait());
        let first = first.acquire().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut wait)
                .await
                .is_err()
        );
        first.complete();
        let second = second.acquire().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut wait)
                .await
                .is_err(),
            "the caller must not resume when only the first result in a batch is ready"
        );
        second.complete();
        wait.await;

        assert_eq!(lane.holder(), Some(primary_id));
        assert_eq!(off_lane.filesystem(), FilesystemCapability::Incapable);
        drop(off_lane);
    }

    #[test]
    async fn lane_inherits_through_a_blocked_off_lane_caller() {
        let lane = lane();
        let primary_id = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let _primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let off_lane_id = entity(&lane, "outer", 2);
        let off_lane = lane
            .register_entity(
                primary_id.clone(),
                off_lane_id.clone(),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Incapable,
            )
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let capable_id = entity(&lane, "inner", 3);
        let capable = lane
            .register_entity(
                OwnerInvocationId::Entity(off_lane_id.clone()),
                capable_id.clone(),
                EntityCallMode::Synchronous,
                FilesystemCapability::Capable,
            )
            .unwrap();
        let mut acquire = Box::pin(capable.acquire());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut acquire)
                .await
                .is_err(),
            "an unrelated off-lane chain must not take the lane"
        );

        lane.await_invocations(&primary_id, [OwnerInvocationId::Entity(off_lane_id)])
            .unwrap();
        let capable = acquire.await.unwrap();
        assert_eq!(lane.holder(), Some(OwnerInvocationId::Entity(capable_id)));
        drop(capable);
        assert_eq!(lane.holder(), Some(primary_id));
        drop(off_lane);
    }

    #[test]
    async fn synchronous_caller_resumes_after_children_eligible_at_body_end_finish() {
        let lane = lane();
        let primary_id = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let _primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let body_id = entity(&lane, "body", 2);
        let body = lane
            .register_entity(
                primary_id.clone(),
                body_id.clone(),
                EntityCallMode::Synchronous,
                FilesystemCapability::Capable,
            )
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let child_id = entity(&lane, "child", 3);
        let child = lane
            .register_entity(
                OwnerInvocationId::Entity(body_id),
                child_id.clone(),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Capable,
            )
            .unwrap();

        let mut body_completion = Box::pin(body.complete_and_wait());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut body_completion)
                .await
                .is_err()
        );
        let child = child.acquire().await.unwrap();
        assert_eq!(
            lane.holder(),
            Some(OwnerInvocationId::Entity(child_id.clone()))
        );
        let grandchild_id = entity(&lane, "grandchild", 4);
        let grandchild = lane
            .register_entity(
                OwnerInvocationId::Entity(child_id),
                grandchild_id.clone(),
                EntityCallMode::Asynchronous,
                FilesystemCapability::Capable,
            )
            .unwrap();
        let mut child_completion = Box::pin(child.complete_and_wait());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut child_completion)
                .await
                .is_err()
        );
        let grandchild = grandchild.acquire().await.unwrap();
        assert_eq!(
            lane.holder(),
            Some(OwnerInvocationId::Entity(grandchild_id))
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut body_completion)
                .await
                .is_err(),
            "the synchronous caller must remain blocked while the child chain is active"
        );

        grandchild.complete();
        child_completion.await;
        body_completion.await;
        assert_eq!(lane.holder(), Some(primary_id));
    }

    #[test]
    async fn blocked_owner_reentrancy_is_rejected_but_entity_self_call_is_not() {
        let lane = lane();
        let primary_id = OwnerInvocationId::Agent(OplogIndex::from_u64(1));
        let _primary = lane
            .enter_primary(OplogIndex::from_u64(1))
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let entity_id = entity(&lane, "recursive", 2);
        let entity_permit = lane
            .register_entity(
                primary_id,
                entity_id.clone(),
                EntityCallMode::Synchronous,
                FilesystemCapability::Capable,
            )
            .unwrap()
            .acquire()
            .await
            .unwrap();
        let caller = OwnerInvocationId::Entity(entity_id.clone());
        assert!(matches!(
            lane.ensure_synchronous_owner_call(&caller),
            Err(OwnerLaneError::WouldDeadlock { .. })
        ));

        let nested = lane
            .register_entity(
                caller,
                entity(&lane, "recursive", 3),
                EntityCallMode::Synchronous,
                FilesystemCapability::Capable,
            )
            .unwrap()
            .acquire()
            .await
            .unwrap();
        drop(nested);
        drop(entity_permit);
    }
}
