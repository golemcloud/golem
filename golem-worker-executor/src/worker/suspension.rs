// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use futures::future::Either;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use golem_common::model::entity::EntityInvocationId;
use golem_service_base::error::worker_executor::InterruptKind;
use tokio::sync::Notify;
use wasmtime::component::{
    RuntimeActivityId, RuntimeInvalidation, RuntimeObservation, RuntimeObserver,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ActivityId(u64);

#[derive(Clone, Debug)]
enum ActivityClass {
    Unknown,
    SourceWait {
        store_id: u64,
    },
    Neutral {
        classification: u64,
        dependencies: Vec<ActivityId>,
    },
    Wait {
        wakeup: Wakeup,
        classification: u64,
    },
    RpcWait {
        eligible_at: Instant,
        resume_after: Duration,
        classification: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wakeup {
    At(Instant),
    OnActivation,
}

#[derive(Clone)]
struct Attempt {
    revision: u64,
    wakeup: Wakeup,
    suspend_at: golem_common::model::Timestamp,
}

struct RuntimeActivity {
    activity: ActivityId,
    root: bool,
}

#[derive(Default)]
struct DriverState {
    active_poll: bool,
    generation: u64,
    blocked_generation: Option<u64>,
    runtime_run: Option<RuntimeActivityId>,
    runtime_watermark: usize,
    runtime_candidate: Option<usize>,
    retired_runs: HashSet<RuntimeActivityId>,
}

#[derive(Default)]
struct StoreState {
    driver: DriverState,
    activities: HashMap<RuntimeActivityId, RuntimeActivity>,
    delegated: HashSet<ActivityId>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    next_store_id: u64,
    next_classification: u64,
    revision: u64,
    stopped: Option<Attempt>,
    stores: HashMap<u64, StoreState>,
    activities: HashMap<ActivityId, ActivityClass>,
    external_activities: HashSet<ActivityId>,
    entity_activities: HashMap<EntityInvocationId, ActivityId>,
}

pub(crate) struct OwnerSuspension {
    state: Mutex<State>,
    changed: Notify,
}

impl OwnerSuspension {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            changed: Notify::new(),
        })
    }

    fn invalidate(state: &mut State) {
        state.revision = state.revision.wrapping_add(1);
    }

    fn classify_wait(&self, activity: &ActivityId, wakeup: Wakeup) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if !state.activities.contains_key(activity) || state.stopped.is_some() {
            return None;
        }
        state.next_classification = state.next_classification.wrapping_add(1);
        let classification = state.next_classification;
        state.activities.insert(
            activity.clone(),
            ActivityClass::Wait {
                wakeup,
                classification,
            },
        );
        OwnerSuspension::invalidate(&mut state);
        Some(classification)
    }

    fn revoke_wait(&self, activity: &ActivityId, classification: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if matches!(
            state.activities.get(activity),
            Some(ActivityClass::Wait { classification: current, .. }
                | ActivityClass::RpcWait { classification: current, .. })
                if *current == classification
        ) && state.stopped.is_none()
        {
            state
                .activities
                .insert(activity.clone(), ActivityClass::Unknown);
            Self::invalidate(&mut state);
            true
        } else {
            false
        }
    }

    fn eligible(state: &State, now: Instant, suspend_after: Duration) -> Option<Wakeup> {
        if state.stopped.is_some() || state.stores.is_empty() {
            return None;
        }
        let mut earliest = None;
        let mut eligible_waits = 0;
        for store in state.stores.values() {
            if store.driver.active_poll {
                return None;
            }
            if store.activities.is_empty()
                && store.driver.runtime_run.is_none()
                && store.delegated.is_empty()
            {
                continue;
            }
            if store.driver.blocked_generation != Some(store.driver.generation)
                || !store.activities.values().any(|activity| activity.root)
            {
                return None;
            }
            for activity in store.activities.values() {
                match state.activities.get(&activity.activity) {
                    Some(ActivityClass::Wait {
                        wakeup: Wakeup::At(deadline),
                        ..
                    }) if *deadline > now
                        && deadline.saturating_duration_since(now) >= suspend_after =>
                    {
                        earliest =
                            Some(earliest.map_or(*deadline, |old: Instant| old.min(*deadline)));
                        eligible_waits += 1;
                    }
                    Some(ActivityClass::Wait {
                        wakeup: Wakeup::OnActivation,
                        ..
                    }) => {
                        eligible_waits += 1;
                    }
                    Some(ActivityClass::RpcWait {
                        eligible_at,
                        resume_after,
                        ..
                    }) if now >= *eligible_at => {
                        let deadline = now + *resume_after;
                        earliest =
                            Some(earliest.map_or(deadline, |old: Instant| old.min(deadline)));
                        eligible_waits += 1;
                    }
                    Some(ActivityClass::Neutral { dependencies, .. })
                        if !dependencies.is_empty()
                            && dependencies
                                .iter()
                                .all(|id| state.activities.contains_key(id)) => {}
                    Some(ActivityClass::Unknown) if activity.root => {}
                    _ => return None,
                }
            }
        }
        for activity in &state.external_activities {
            match state.activities.get(activity) {
                Some(ActivityClass::SourceWait { store_id })
                    if state.stores.get(store_id).is_some_and(|store| {
                        store
                            .driver
                            .runtime_run
                            .is_some_and(|run| !store.driver.retired_runs.contains(&run))
                            && store.driver.blocked_generation == Some(store.driver.generation)
                            && store.activities.values().any(|activity| activity.root)
                    }) => {}
                Some(ActivityClass::RpcWait {
                    eligible_at,
                    resume_after,
                    ..
                }) if now >= *eligible_at => {
                    let deadline = now + *resume_after;
                    earliest = Some(earliest.map_or(deadline, |old: Instant| old.min(deadline)));
                    eligible_waits += 1;
                }
                _ => return None,
            }
        }
        if eligible_waits == 0 {
            None
        } else {
            Some(earliest.map_or(Wakeup::OnActivation, Wakeup::At))
        }
    }

    fn prepare(&self, now: Instant, suspend_after: Duration) -> Option<Attempt> {
        let state = self.state.lock().unwrap();
        let wakeup = Self::eligible(&state, now, suspend_after)?;
        Some(Attempt {
            revision: state.revision,
            wakeup,
            suspend_at: golem_common::model::Timestamp::now_utc(),
        })
    }

    fn commit(&self, attempt: &Attempt, now: Instant, suspend_after: Duration) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.revision != attempt.revision
            || Self::eligible(&state, now, suspend_after).is_none()
            || matches!(attempt.wakeup, Wakeup::At(deadline) if deadline <= now)
        {
            return false;
        }
        state.stopped = Some(attempt.clone());
        drop(state);
        self.changed.notify_waiters();
        true
    }

    fn register_rpc(
        self: &Arc<Self>,
        eligible_after: Duration,
        resume_after: Duration,
    ) -> RpcActivity {
        let activity = {
            let mut state = self.state.lock().unwrap();
            state.next_id = state.next_id.wrapping_add(1);
            let activity = ActivityId(state.next_id);
            state
                .activities
                .insert(activity.clone(), ActivityClass::Unknown);
            state.external_activities.insert(activity.clone());
            Self::invalidate(&mut state);
            activity
        };
        RpcActivity {
            shared: Arc::new(RpcActivityShared {
                owner: self.clone(),
                activity,
                eligible_after,
                resume_after,
                poll: Mutex::new(RpcPollState::default()),
            }),
        }
    }

    pub(crate) fn register_entity(
        self: &Arc<Self>,
        invocation: EntityInvocationId,
    ) -> EntityActivity {
        let activity = {
            let mut state = self.state.lock().unwrap();
            state.next_id = state.next_id.wrapping_add(1);
            let activity = ActivityId(state.next_id);
            state
                .activities
                .insert(activity.clone(), ActivityClass::Unknown);
            state.external_activities.insert(activity.clone());
            state
                .entity_activities
                .insert(invocation.clone(), activity.clone());
            Self::invalidate(&mut state);
            activity
        };
        EntityActivity {
            owner: self.clone(),
            invocation,
            activity,
        }
    }

    pub(crate) fn register_external(self: &Arc<Self>) -> ExternalActivity {
        let activity = {
            let mut state = self.state.lock().unwrap();
            state.next_id = state.next_id.wrapping_add(1);
            let activity = ActivityId(state.next_id);
            state
                .activities
                .insert(activity.clone(), ActivityClass::Unknown);
            state.external_activities.insert(activity.clone());
            Self::invalidate(&mut state);
            activity
        };
        ExternalActivity {
            owner: self.clone(),
            activity,
            invalidated: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    async fn stopped(&self) -> Attempt {
        loop {
            let notified = self.changed.notified();
            if let Some(stop) = self.state.lock().unwrap().stopped.clone() {
                return stop;
            }
            notified.await;
        }
    }
}

pub(crate) struct EntityActivity {
    owner: Arc<OwnerSuspension>,
    invocation: EntityInvocationId,
    activity: ActivityId,
}

pub(crate) struct ExternalActivity {
    owner: Arc<OwnerSuspension>,
    activity: ActivityId,
    invalidated: Arc<std::sync::atomic::AtomicBool>,
}

/// Identifies an accounted source without retaining its resident Store or driver.
#[derive(Clone)]
pub(crate) struct RuntimeSource {
    owner: Arc<OwnerSuspension>,
    store_id: u64,
}

impl RuntimeSource {
    pub(crate) fn external_activity(&self) -> ExternalActivity {
        self.owner.register_external()
    }
}

impl ExternalActivity {
    /// Only the receive is passive; the caller retains the activity for all processing.
    /// The source must identify the runtime that can produce this receive's next event.
    pub(crate) fn receive<'a, F: Future>(
        &'a mut self,
        source: &'a RuntimeSource,
        future: F,
    ) -> ExternalReceive<'a, F> {
        ExternalReceive {
            activity: self,
            source,
            future,
            fence_delivery: true,
        }
    }

    /// Accounts a join of already-registered children without fencing their cleanup.
    /// Every queued child must veto suspension until it independently becomes passive.
    pub(crate) fn coordinate<'a, F: Future>(
        &'a mut self,
        source: &'a RuntimeSource,
        future: F,
    ) -> ExternalReceive<'a, F> {
        ExternalReceive {
            activity: self,
            source,
            future,
            fence_delivery: false,
        }
    }

    fn active(&self) {
        let mut state = self.owner.state.lock().unwrap();
        if state.activities.contains_key(&self.activity) {
            state
                .activities
                .insert(self.activity.clone(), ActivityClass::Unknown);
            OwnerSuspension::invalidate(&mut state);
        }
    }
}

pub(crate) struct ExternalReceive<'a, F> {
    activity: &'a mut ExternalActivity,
    source: &'a RuntimeSource,
    future: F,
    fence_delivery: bool,
}

struct ExternalReceiveWake {
    owner: Arc<OwnerSuspension>,
    activity: ActivityId,
    invalidated: Arc<std::sync::atomic::AtomicBool>,
    forward: Waker,
}

impl futures::task::ArcWake for ExternalReceiveWake {
    fn wake_by_ref(this: &Arc<Self>) {
        let mut state = this.owner.state.lock().unwrap();
        this.invalidated
            .store(true, std::sync::atomic::Ordering::Relaxed);
        if state.activities.contains_key(&this.activity) {
            state
                .activities
                .insert(this.activity.clone(), ActivityClass::Unknown);
            OwnerSuspension::invalidate(&mut state);
        }
        drop(state);
        this.forward.wake_by_ref();
    }
}

impl<F: Future> Future for ExternalReceive<'_, F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `future` is structurally pinned and never moved.
        let this = unsafe { self.get_unchecked_mut() };
        {
            let mut state = this.activity.owner.state.lock().unwrap();
            if this.fence_delivery && state.stopped.is_some() {
                // Keep custody until the existing cancellation/teardown path drops this wait.
                return Poll::Pending;
            }
            this.activity
                .invalidated
                .store(false, std::sync::atomic::Ordering::Relaxed);
            state
                .activities
                .insert(this.activity.activity.clone(), ActivityClass::Unknown);
            OwnerSuspension::invalidate(&mut state);
        }
        let wake = Arc::new(ExternalReceiveWake {
            owner: this.activity.owner.clone(),
            activity: this.activity.activity.clone(),
            invalidated: this.activity.invalidated.clone(),
            forward: cx.waker().clone(),
        });
        let waker = futures::task::waker_ref(&wake);
        let result =
            unsafe { Pin::new_unchecked(&mut this.future) }.poll(&mut Context::from_waker(&waker));
        if result.is_pending() && Arc::ptr_eq(&this.activity.owner, &this.source.owner) {
            let mut state = this.activity.owner.state.lock().unwrap();
            if !wake.invalidated.load(std::sync::atomic::Ordering::Relaxed)
                && state.stopped.is_none()
            {
                state.activities.insert(
                    this.activity.activity.clone(),
                    ActivityClass::SourceWait {
                        store_id: this.source.store_id,
                    },
                );
                OwnerSuspension::invalidate(&mut state);
            }
        }
        result
    }
}

impl<F> Drop for ExternalReceive<'_, F> {
    fn drop(&mut self) {
        self.activity.active();
    }
}

impl Drop for ExternalActivity {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        state.external_activities.remove(&self.activity);
        state.activities.remove(&self.activity);
        OwnerSuspension::invalidate(&mut state);
    }
}

impl Drop for EntityActivity {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        state.external_activities.remove(&self.activity);
        state.activities.remove(&self.activity);
        if state.entity_activities.get(&self.invocation) == Some(&self.activity) {
            state.entity_activities.remove(&self.invocation);
        }
        for store in state.stores.values_mut() {
            store.delegated.remove(&self.activity);
        }
        OwnerSuspension::invalidate(&mut state);
    }
}

pub(crate) struct NeutralActivity {
    owner: Arc<OwnerSuspension>,
    activity: ActivityId,
    classification: u64,
}

impl Drop for NeutralActivity {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        if matches!(state.activities.get(&self.activity), Some(ActivityClass::Neutral { classification, .. }) if *classification == self.classification)
        {
            state
                .activities
                .insert(self.activity.clone(), ActivityClass::Unknown);
        }
        OwnerSuspension::invalidate(&mut state);
    }
}

#[derive(Default)]
struct RpcPollState {
    generation: u64,
    remote_candidate: bool,
    invalidated: bool,
    revoked: bool,
    passive_since: Option<Instant>,
}

struct RpcActivityShared {
    owner: Arc<OwnerSuspension>,
    activity: ActivityId,
    eligible_after: Duration,
    resume_after: Duration,
    poll: Mutex<RpcPollState>,
}

impl RpcActivityShared {
    fn finish_poll(&self, pending: bool, generation: u64) {
        let mut poll = self.poll.lock().unwrap();
        let passive = pending
            && poll.generation == generation
            && poll.remote_candidate
            && !poll.invalidated
            && !poll.revoked;
        if !passive {
            poll.passive_since = None;
        } else {
            let passive_since = *poll.passive_since.get_or_insert_with(Instant::now);
            let mut state = self.owner.state.lock().unwrap();
            state.next_classification = state.next_classification.wrapping_add(1);
            let classification = state.next_classification;
            state.activities.insert(
                self.activity.clone(),
                ActivityClass::RpcWait {
                    eligible_at: passive_since + self.eligible_after,
                    resume_after: self.resume_after,
                    classification,
                },
            );
            OwnerSuspension::invalidate(&mut state);
        }
    }
}

pub(crate) struct RpcActivity {
    shared: Arc<RpcActivityShared>,
}

#[derive(Clone)]
pub(crate) struct RpcActivityRevoker {
    shared: Arc<RpcActivityShared>,
}

impl RpcActivity {
    pub(crate) fn revoker(&self) -> RpcActivityRevoker {
        RpcActivityRevoker {
            shared: self.shared.clone(),
        }
    }

    pub(crate) async fn coordinate<T>(
        self,
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let owner = self.shared.owner.clone();
        let coordinated = CoordinatedRpc {
            future,
            activity: Some(self),
        };
        tokio::select! {
            biased;
            stop = owner.stopped() => Err(InterruptKind::Suspend(stop.suspend_at).into()),
            result = coordinated => result,
        }
    }
}

impl RpcActivityRevoker {
    pub(crate) fn active(&self) {
        let mut poll = self.shared.poll.lock().unwrap();
        poll.remote_candidate = false;
        poll.passive_since = None;
        let mut state = self.shared.owner.state.lock().unwrap();
        if state.activities.contains_key(&self.shared.activity) && state.stopped.is_none() {
            state
                .activities
                .insert(self.shared.activity.clone(), ActivityClass::Unknown);
            OwnerSuspension::invalidate(&mut state);
        }
    }

    pub(crate) fn revoke(&self) {
        let mut poll = self.shared.poll.lock().unwrap();
        if poll.revoked {
            return;
        }
        poll.revoked = true;
        poll.remote_candidate = false;
        poll.passive_since = None;
        let mut state = self.shared.owner.state.lock().unwrap();
        if state.activities.contains_key(&self.shared.activity) && state.stopped.is_none() {
            state
                .activities
                .insert(self.shared.activity.clone(), ActivityClass::Unknown);
            OwnerSuspension::invalidate(&mut state);
        }
    }

    pub(crate) fn remote_wait<F: Future>(&self, future: F) -> RpcRemoteWait<F> {
        RpcRemoteWait {
            future,
            shared: self.shared.clone(),
        }
    }
}

impl Drop for RpcActivity {
    fn drop(&mut self) {
        let mut state = self.shared.owner.state.lock().unwrap();
        state.external_activities.remove(&self.shared.activity);
        state.activities.remove(&self.shared.activity);
        OwnerSuspension::invalidate(&mut state);
    }
}

pub(crate) struct RpcRemoteWait<F> {
    future: F,
    shared: Arc<RpcActivityShared>,
}

impl<F: Future> Future for RpcRemoteWait<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `future` is structurally pinned and never moved.
        let this = unsafe { self.get_unchecked_mut() };
        let result = unsafe { Pin::new_unchecked(&mut this.future) }.poll(cx);
        if result.is_ready() {
            RpcActivityRevoker {
                shared: this.shared.clone(),
            }
            .active();
        } else {
            this.shared.poll.lock().unwrap().remote_candidate = true;
        }
        result
    }
}

impl<F> Drop for RpcRemoteWait<F> {
    fn drop(&mut self) {
        RpcActivityRevoker {
            shared: self.shared.clone(),
        }
        .active();
    }
}

pub(crate) struct CoordinatedRpc<F> {
    future: F,
    activity: Option<RpcActivity>,
}

struct RpcWake {
    shared: Arc<RpcActivityShared>,
    forward: Waker,
}

impl futures::task::ArcWake for RpcWake {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let mut poll = arc_self.shared.poll.lock().unwrap();
        poll.invalidated = true;
        poll.remote_candidate = false;
        let mut state = arc_self.shared.owner.state.lock().unwrap();
        if state.activities.contains_key(&arc_self.shared.activity) && state.stopped.is_none() {
            state
                .activities
                .insert(arc_self.shared.activity.clone(), ActivityClass::Unknown);
            OwnerSuspension::invalidate(&mut state);
        }
        drop(state);
        drop(poll);
        arc_self.forward.wake_by_ref();
    }
}

impl<F: Future> Future for CoordinatedRpc<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: `future` is structurally pinned and never moved.
        let this = unsafe { self.get_unchecked_mut() };
        let activity = this
            .activity
            .as_ref()
            .expect("polled completed RPC activity");
        let generation = {
            let mut poll = activity.shared.poll.lock().unwrap();
            poll.generation = poll.generation.wrapping_add(1);
            poll.remote_candidate = false;
            poll.invalidated = false;
            let mut state = activity.shared.owner.state.lock().unwrap();
            if state.stopped.is_some() {
                return Poll::Pending;
            }
            state
                .activities
                .insert(activity.shared.activity.clone(), ActivityClass::Unknown);
            OwnerSuspension::invalidate(&mut state);
            poll.generation
        };
        let wake = Arc::new(RpcWake {
            shared: activity.shared.clone(),
            forward: cx.waker().clone(),
        });
        let waker = futures::task::waker_ref(&wake);
        let mut inner_cx = Context::from_waker(&waker);
        let result = unsafe { Pin::new_unchecked(&mut this.future) }.poll(&mut inner_cx);
        activity.shared.finish_poll(result.is_pending(), generation);
        if result.is_ready() {
            this.activity.take();
        }
        result
    }
}

pub(crate) struct RuntimeStore {
    owner: Arc<OwnerSuspension>,
    store_id: u64,
}

impl RuntimeStore {
    pub(crate) fn source(&self) -> RuntimeSource {
        RuntimeSource {
            owner: self.owner.clone(),
            store_id: self.store_id,
        }
    }

    pub(crate) fn new(owner: Arc<OwnerSuspension>) -> Arc<Self> {
        let store_id = {
            let mut state = owner.state.lock().unwrap();
            state.next_store_id = state.next_store_id.wrapping_add(1);
            let store_id = state.next_store_id;
            state.stores.insert(store_id, StoreState::default());
            OwnerSuspension::invalidate(&mut state);
            store_id
        };
        Arc::new(Self { owner, store_id })
    }

    pub(crate) fn timer(
        &self,
        runtime: RuntimeActivityId,
        deadline: Instant,
    ) -> Option<SuspensionWait> {
        self.wait(runtime, Wakeup::At(deadline))
    }

    pub(crate) fn promise(&self, runtime: RuntimeActivityId) -> Option<SuspensionWait> {
        self.wait(runtime, Wakeup::OnActivation)
    }

    pub(crate) fn rpc_activity(
        &self,
        eligible_after: Duration,
        resume_after: Duration,
    ) -> RpcActivity {
        self.owner.register_rpc(eligible_after, resume_after)
    }

    pub(crate) fn neutral_runtime(
        &self,
        runtime: RuntimeActivityId,
        dependencies: &[RuntimeActivityId],
    ) -> Option<NeutralActivity> {
        let mut state = self.owner.state.lock().unwrap();
        let store = state.stores.get(&self.store_id)?;
        let dependencies = dependencies
            .iter()
            .map(|id| {
                store
                    .activities
                    .get(id)
                    .map(|activity| activity.activity.clone())
            })
            .collect::<Option<Vec<_>>>()?;
        self.neutral(&mut state, runtime, dependencies)
    }

    pub(crate) fn neutral_entity(
        &self,
        runtime: RuntimeActivityId,
        invocation: &EntityInvocationId,
    ) -> Option<NeutralActivity> {
        let mut state = self.owner.state.lock().unwrap();
        let dependency = state.entity_activities.get(invocation)?.clone();
        self.neutral(&mut state, runtime, vec![dependency])
    }

    fn neutral(
        &self,
        state: &mut State,
        runtime: RuntimeActivityId,
        dependencies: Vec<ActivityId>,
    ) -> Option<NeutralActivity> {
        let activity = state
            .stores
            .get(&self.store_id)?
            .activities
            .get(&runtime)?
            .activity
            .clone();
        if state.stopped.is_some() || dependencies.is_empty() || dependencies.contains(&activity) {
            return None;
        }
        state.next_classification = state.next_classification.wrapping_add(1);
        let classification = state.next_classification;
        state.activities.insert(
            activity.clone(),
            ActivityClass::Neutral {
                classification,
                dependencies,
            },
        );
        OwnerSuspension::invalidate(state);
        Some(NeutralActivity {
            owner: self.owner.clone(),
            activity,
            classification,
        })
    }

    pub(crate) fn external_activity(&self) -> ExternalActivity {
        self.owner.register_external()
    }

    fn delegate(&self, invocation: &EntityInvocationId) -> Option<DelegatedActivity> {
        let mut state = self.owner.state.lock().unwrap();
        let activity = state.entity_activities.get(invocation)?.clone();
        if state.stopped.is_some() || !state.external_activities.remove(&activity) {
            return None;
        }
        state
            .stores
            .get_mut(&self.store_id)?
            .delegated
            .insert(activity.clone());
        OwnerSuspension::invalidate(&mut state);
        Some(DelegatedActivity {
            store: Arc::new((self.owner.clone(), self.store_id)),
            activity,
        })
    }

    pub(crate) fn rpc_wait(
        &self,
        runtime: RuntimeActivityId,
        eligible_after: Duration,
        resume_after: Duration,
    ) -> Option<SuspensionWait> {
        let activity = self
            .owner
            .state
            .lock()
            .unwrap()
            .stores
            .get(&self.store_id)?
            .activities
            .get(&runtime)?
            .activity
            .clone();
        let mut state = self.owner.state.lock().unwrap();
        if state.stopped.is_some() {
            return None;
        }
        state.next_classification = state.next_classification.wrapping_add(1);
        let classification = state.next_classification;
        state.activities.insert(
            activity.clone(),
            ActivityClass::RpcWait {
                eligible_at: Instant::now() + eligible_after,
                resume_after,
                classification,
            },
        );
        OwnerSuspension::invalidate(&mut state);
        Some(SuspensionWait {
            owner: self.owner.clone(),
            activity,
            classification,
            armed: true,
        })
    }

    fn wait(&self, runtime: RuntimeActivityId, wakeup: Wakeup) -> Option<SuspensionWait> {
        let activity = self
            .owner
            .state
            .lock()
            .unwrap()
            .stores
            .get(&self.store_id)?
            .activities
            .get(&runtime)?
            .activity
            .clone();
        let classification = self.owner.classify_wait(&activity, wakeup)?;
        Some(SuspensionWait {
            owner: self.owner.clone(),
            activity,
            classification,
            armed: true,
        })
    }

    pub(crate) fn certify_root(&self, runtime: RuntimeActivityId) {
        let mut state = self.owner.state.lock().unwrap();
        if let Some(activity) = state
            .stores
            .get_mut(&self.store_id)
            .and_then(|store| store.activities.get_mut(&runtime))
        {
            activity.root = true;
            OwnerSuspension::invalidate(&mut state);
        }
    }

    pub(crate) async fn drive<R>(
        self: &Arc<Self>,
        future: impl Future<Output = wasmtime::Result<R>>,
        entity: Option<&EntityInvocationId>,
        policy: Option<(
            crate::durable_host::WakeupScheduler,
            crate::services::golem_config::SuspendConfig,
        )>,
    ) -> wasmtime::Result<R> {
        let _delegated = entity.and_then(|entity| self.delegate(entity));
        let mut future = std::pin::pin!(future);
        let driver = std::pin::pin!(std::future::poll_fn(|cx| {
            {
                let mut state = self.owner.state.lock().unwrap();
                let Some(store) = state.stores.get_mut(&self.store_id) else {
                    return std::task::Poll::Ready(Err(wasmtime::Error::msg(
                        "runtime store was retired",
                    )));
                };
                store.driver.active_poll = true;
                store.driver.generation = store.driver.generation.wrapping_add(1);
                store.driver.blocked_generation = None;
                OwnerSuspension::invalidate(&mut state);
            }
            let result = future.as_mut().poll(cx);
            {
                let mut state = self.owner.state.lock().unwrap();
                if let Some(store) = state.stores.get_mut(&self.store_id) {
                    store.driver.active_poll = false;
                    if result.is_pending() && store.driver.runtime_candidate.is_some() {
                        store.driver.blocked_generation = Some(store.driver.generation);
                    }
                }
            }
            result
        }));

        // Keep policy in the same task as the Wasmtime driver: dropping this future drops both.
        let Some((scheduler, config)) = policy else {
            return driver.await;
        };
        let policy = async {
            let mut delay = config.wait_suspend_grace;
            loop {
                tokio::time::sleep(delay).await;
                delay = config.wait_suspend_check_interval;
                let Some(attempt) = self.owner.prepare(Instant::now(), config.suspend_after) else {
                    continue;
                };
                if let Wakeup::At(deadline) = attempt.wakeup {
                    let when = chrono::Utc::now()
                        + chrono::Duration::from_std(
                            deadline.saturating_duration_since(Instant::now()),
                        )?;
                    scheduler.sleep_until(when).await?;
                }
                if self
                    .owner
                    .commit(&attempt, Instant::now(), config.suspend_after)
                {
                    return Ok::<(), anyhow::Error>(());
                }
            }
        };
        enum Completion<R> {
            Driver(wasmtime::Result<R>),
            Policy(anyhow::Result<()>),
        }
        // Separate wake registration prevents scheduler persistence from spuriously polling
        // the driver and invalidating the very blocked evidence being persisted.
        let mut tasks = FuturesUnordered::new();
        tasks.push(Either::Left(driver.map(Completion::Driver)));
        tasks.push(Either::Right(policy.map(Completion::Policy)));
        while let Some(completion) = tasks.next().await {
            match completion {
                Completion::Driver(result) => return result,
                Completion::Policy(result) => result.map_err(wasmtime::Error::from_anyhow)?,
            }
        }
        unreachable!("driver completion returns from the loop")
    }
}

struct DelegatedActivity {
    store: Arc<(Arc<OwnerSuspension>, u64)>,
    activity: ActivityId,
}

impl Drop for DelegatedActivity {
    fn drop(&mut self) {
        let (owner, store_id) = &*self.store;
        let mut state = owner.state.lock().unwrap();
        if let Some(store) = state.stores.get_mut(store_id) {
            store.delegated.remove(&self.activity);
        }
        if state.activities.contains_key(&self.activity) {
            state.external_activities.insert(self.activity.clone());
            state
                .activities
                .insert(self.activity.clone(), ActivityClass::Unknown);
        }
        OwnerSuspension::invalidate(&mut state);
    }
}

impl Drop for RuntimeStore {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        if let Some(store) = state.stores.remove(&self.store_id) {
            for activity in store.activities.values() {
                state.activities.remove(&activity.activity);
            }
            OwnerSuspension::invalidate(&mut state);
        }
    }
}

pub(crate) struct SuspensionWait {
    owner: Arc<OwnerSuspension>,
    activity: ActivityId,
    classification: u64,
    armed: bool,
}

impl SuspensionWait {
    fn ready(&mut self) -> bool {
        self.armed = false;
        self.owner.revoke_wait(&self.activity, self.classification)
    }

    async fn stopped(&self) -> golem_common::model::Timestamp {
        self.owner.stopped().await.suspend_at
    }

    pub(crate) async fn wait(
        mut self,
        ready: impl Future<Output = ()>,
        interrupt: impl Future<Output = InterruptKind>,
    ) -> Result<(), InterruptKind> {
        tokio::select! {
            biased;
            kind = interrupt => {
                self.ready();
                Err(kind)
            },
            suspend_at = self.stopped() => Err(InterruptKind::Suspend(suspend_at)),
            _ = ready => {
                if self.ready() {
                    Ok(())
                } else {
                    Err(InterruptKind::Suspend(self.stopped().await))
                }
            },
        }
    }

    pub(crate) async fn wait_result<T>(
        mut self,
        ready: impl Future<Output = T>,
        interrupt: impl Future<Output = InterruptKind>,
    ) -> Result<T, InterruptKind> {
        tokio::select! {
            biased;
            kind = interrupt => {
                self.ready();
                Err(kind)
            },
            suspend_at = self.stopped() => Err(InterruptKind::Suspend(suspend_at)),
            value = ready => {
                if self.ready() {
                    Ok(value)
                } else {
                    Err(InterruptKind::Suspend(self.stopped().await))
                }
            },
        }
    }
}

impl Drop for SuspensionWait {
    fn drop(&mut self) {
        if self.armed {
            self.owner.revoke_wait(&self.activity, self.classification);
        }
    }
}

impl RuntimeObserver for RuntimeStore {
    fn observe(&self, event: RuntimeObservation) {
        let mut state = self.owner.state.lock().unwrap();
        match event {
            RuntimeObservation::ActivityStarted { activity, .. } => {
                state.next_id = state.next_id.wrapping_add(1);
                let id = ActivityId(state.next_id);
                state.activities.insert(id.clone(), ActivityClass::Unknown);
                let Some(store) = state.stores.get_mut(&self.store_id) else {
                    return;
                };
                if let Some(old) = store.activities.insert(
                    activity,
                    RuntimeActivity {
                        activity: id,
                        root: false,
                    },
                ) {
                    state.activities.remove(&old.activity);
                }
                OwnerSuspension::invalidate(&mut state);
            }
            RuntimeObservation::ActivityFinished { activity } => {
                let removed = state
                    .stores
                    .get_mut(&self.store_id)
                    .and_then(|store| store.activities.remove(&activity));
                if let Some(activity) = removed {
                    state.activities.remove(&activity.activity);
                    OwnerSuspension::invalidate(&mut state);
                }
            }
            RuntimeObservation::DriverBlocked { run, generation } => {
                let Some(store) = state.stores.get_mut(&self.store_id) else {
                    return;
                };
                let driver = &mut store.driver;
                if driver.runtime_run == Some(run)
                    && !driver.retired_runs.contains(&run)
                    && generation >= driver.runtime_watermark
                    && driver.active_poll
                {
                    driver.runtime_candidate = Some(generation);
                }
            }
            RuntimeObservation::DriverInvalidated {
                run,
                generation,
                reason,
            } => {
                let mut invalidated = false;
                {
                    let Some(store) = state.stores.get_mut(&self.store_id) else {
                        return;
                    };
                    let driver = &mut store.driver;
                    if driver.retired_runs.contains(&run) {
                        return;
                    }
                    if reason == RuntimeInvalidation::Poll && driver.runtime_run != Some(run) {
                        if let Some(previous) = driver.runtime_run.replace(run) {
                            driver.retired_runs.insert(previous);
                        }
                        driver.runtime_watermark = 0;
                    }
                    if driver.runtime_run != Some(run) {
                        return;
                    }
                    if generation > driver.runtime_watermark {
                        driver.runtime_watermark = generation;
                        driver.runtime_candidate = None;
                        driver.blocked_generation = None;
                        invalidated = true;
                    }
                    if reason == RuntimeInvalidation::DriverDrop {
                        driver.retired_runs.insert(run);
                    }
                }
                if invalidated {
                    OwnerSuspension::invalidate(&mut state);
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use golem_common::model::component::ComponentId;
    use golem_common::model::entity::{AgentEntity, OwnedAgentEntityId};
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::tool::ToolName;
    use golem_common::model::{AgentId, OwnedAgentId};
    use std::future::pending;
    use wasmtime::component::{Accessor, AccessorTask, RuntimeActivityKind};
    use wasmtime::{AsContextMut, Config, Engine, Store};

    use test_r::test;

    struct PendingTask;

    impl AccessorTask<()> for PendingTask {
        async fn run(self, _: &Accessor<()>) -> wasmtime::Result<()> {
            pending().await
        }
    }

    fn runtime_ids(count: usize) -> Vec<RuntimeActivityId> {
        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let engine = Engine::new(&config).unwrap();
        let mut store = Store::new(&engine, ());
        let ids = Arc::new(Mutex::new(Vec::new()));
        let observed = ids.clone();
        store
            .as_context_mut()
            .set_runtime_observer(Arc::new(move |event| {
                if let RuntimeObservation::ActivityStarted { activity, .. } = event {
                    observed.lock().unwrap().push(activity);
                }
            }))
            .unwrap();
        for _ in 0..count {
            store.as_context_mut().spawn(PendingTask);
        }
        let result = ids.lock().unwrap().clone();
        assert_eq!(result.len(), count);
        result
    }

    pub(crate) fn blocked_timer(
        owner: &Arc<OwnerSuspension>,
        deadline: Instant,
    ) -> (Arc<RuntimeStore>, RuntimeActivityId, SuspensionWait) {
        let ids = runtime_ids(2);
        let store = RuntimeStore::new(owner.clone());
        store.observe(RuntimeObservation::ActivityStarted {
            activity: ids[0],
            kind: RuntimeActivityKind::Root,
        });
        store.certify_root(ids[0]);
        let timer = store.timer(ids[0], deadline).unwrap();
        let mut state = owner.state.lock().unwrap();
        let driver = &mut state.stores.get_mut(&store.store_id).unwrap().driver;
        driver.runtime_run = Some(ids[1]);
        driver.generation = 1;
        driver.blocked_generation = Some(1);
        drop(state);
        (store, ids[1], timer)
    }

    pub(crate) fn eligible_now(owner: &OwnerSuspension) -> bool {
        owner.prepare(Instant::now(), Duration::ZERO).is_some()
    }

    pub(crate) fn commit_now(owner: &OwnerSuspension) -> bool {
        owner
            .prepare(Instant::now(), Duration::ZERO)
            .is_some_and(|attempt| owner.commit(&attempt, Instant::now(), Duration::ZERO))
    }

    fn entity(start: u64) -> EntityInvocationId {
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: OwnedAgentId::new(
                    EnvironmentId::new(),
                    &AgentId {
                        component_id: ComponentId::new(),
                        agent_id: "owner".to_string(),
                    },
                ),
                entity: AgentEntity::Tool(ToolName::try_from("timer").unwrap()),
            },
            OplogIndex::from_u64(start),
        )
        .unwrap()
    }

    #[test]
    fn external_receive_is_passive_only_until_ready_or_cancelled() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let source = store.source();
        let mut activity = owner.register_external();
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut receive = Box::pin(activity.receive(&source, rx));
        assert!(poll_once(receive.as_mut()).is_pending());
        let attempt = owner.prepare(now, Duration::ZERO).unwrap();
        tx.send(37).unwrap();
        assert!(!owner.commit(&attempt, now, Duration::ZERO));
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        assert_eq!(poll_once(receive.as_mut()), Poll::Ready(Ok(37)));
        drop(receive);
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        let mut receive = Box::pin(activity.receive(&source, pending::<()>()));
        assert!(poll_once(receive.as_mut()).is_pending());
        assert!(owner.prepare(now, Duration::ZERO).is_some());
        drop(receive);
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        drop(activity);
        assert!(owner.prepare(now, Duration::ZERO).is_some());
    }

    #[test]
    fn external_receive_rejects_buffered_ready_and_wake_during_poll() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let source = store.source();
        let mut activity = owner.register_external();
        let mut receive = Box::pin(activity.receive(&source, std::future::ready(91)));
        assert_eq!(poll_once(receive.as_mut()), Poll::Ready(91));
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        drop(receive);
        let mut receive = Box::pin(activity.receive(
            &source,
            std::future::poll_fn(|cx| {
                cx.waker().wake_by_ref();
                Poll::<()>::Pending
            }),
        ));
        assert!(poll_once(receive.as_mut()).is_pending());
        assert!(owner.prepare(now, Duration::ZERO).is_none());
    }

    #[test]
    fn external_receive_saved_wake_invalidates_later_polls_and_replacements() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let source = store.source();
        let mut activity = owner.register_external();
        let saved = std::cell::RefCell::new(None::<Waker>);
        let mut receive = Box::pin(activity.receive(
            &source,
            std::future::poll_fn(|cx| {
                if let Some(waker) = saved.borrow().as_ref() {
                    waker.wake_by_ref();
                } else {
                    saved.replace(Some(cx.waker().clone()));
                }
                Poll::<()>::Pending
            }),
        ));
        assert!(poll_once(receive.as_mut()).is_pending());
        assert!(owner.prepare(now, Duration::ZERO).is_some());
        assert!(poll_once(receive.as_mut()).is_pending());
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        drop(receive);
        let mut receive = Box::pin(activity.receive(
            &source,
            std::future::poll_fn(|_| {
                saved.borrow().as_ref().unwrap().wake_by_ref();
                Poll::<()>::Pending
            }),
        ));
        assert!(poll_once(receive.as_mut()).is_pending());
        assert!(owner.prepare(now, Duration::ZERO).is_none());
    }

    #[test]
    fn external_receive_repoll_revokes_commit_before_readiness() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let source = store.source();
        let mut activity = owner.register_external();
        let attempt = std::cell::RefCell::new(None::<Attempt>);
        let mut receive = Box::pin(activity.receive(
            &source,
            std::future::poll_fn(|_| match attempt.borrow().as_ref() {
                None => Poll::Pending,
                Some(attempt) => {
                    assert!(!owner.commit(attempt, now, Duration::ZERO));
                    assert!(owner.prepare(now, Duration::ZERO).is_none());
                    Poll::Ready(53)
                }
            }),
        ));
        assert!(poll_once(receive.as_mut()).is_pending());
        attempt.replace(owner.prepare(now, Duration::ZERO));
        assert!(attempt.borrow().is_some());
        assert_eq!(poll_once(receive.as_mut()), Poll::Ready(53));
    }

    #[test]
    fn external_receive_requires_live_same_owner_source_and_a_real_wait() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, run, timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let foreign = RuntimeStore::new(OwnerSuspension::new()).source();
        let mut activity = owner.register_external();
        let mut receive = Box::pin(activity.receive(&foreign, pending::<()>()));
        assert!(poll_once(receive.as_mut()).is_pending());
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        drop(receive);
        let source = store.source();
        let mut receive = Box::pin(activity.receive(&source, pending::<()>()));
        assert!(poll_once(receive.as_mut()).is_pending());
        assert!(owner.prepare(now, Duration::ZERO).is_some());
        drop(timer);
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        let id = *owner.state.lock().unwrap().stores[&store.store_id]
            .activities
            .keys()
            .next()
            .unwrap();
        let _timer = store.timer(id, now + Duration::from_secs(30)).unwrap();
        assert!(owner.prepare(now, Duration::ZERO).is_some());
        owner
            .state
            .lock()
            .unwrap()
            .stores
            .get_mut(&store.store_id)
            .unwrap()
            .driver
            .retired_runs
            .insert(run);
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        owner
            .state
            .lock()
            .unwrap()
            .stores
            .get_mut(&store.store_id)
            .unwrap()
            .driver
            .retired_runs
            .remove(&run);
        store.observe(RuntimeObservation::DriverInvalidated {
            run,
            generation: 2,
            reason: RuntimeInvalidation::DriverDrop,
        });
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        drop(store);
        let (_replacement, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(60));
        assert!(owner.prepare(now, Duration::ZERO).is_none());
    }

    #[test]
    fn external_receive_retains_unpolled_value_after_suspension_commit() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let source = store.source();
        let mut activity = owner.register_external();
        let polls = std::sync::atomic::AtomicUsize::new(0);
        let drops = std::sync::atomic::AtomicUsize::new(0);
        struct DropGuard<'a>(&'a std::sync::atomic::AtomicUsize);
        impl Drop for DropGuard<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let guard = DropGuard(&drops);
        let polls_ref = &polls;
        let saved = std::cell::RefCell::new(None::<Waker>);
        let saved_ref = &saved;
        let future = std::future::poll_fn(move |cx| {
            let _ = &guard;
            saved_ref.replace(Some(cx.waker().clone()));
            if polls_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                Poll::Pending
            } else {
                Poll::Ready(73)
            }
        });
        let mut receive = Box::pin(activity.receive(&source, future));
        assert!(poll_once(receive.as_mut()).is_pending());
        let attempt = owner.prepare(now, Duration::ZERO).unwrap();
        assert!(owner.commit(&attempt, now, Duration::ZERO));
        saved.borrow().as_ref().unwrap().wake_by_ref();
        assert!(poll_once(receive.as_mut()).is_pending());
        assert_eq!(polls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(drops.load(std::sync::atomic::Ordering::Relaxed), 0);
        drop(receive);
        assert_eq!(drops.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn entity_admission_is_unknown_before_first_task_poll() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let _participant = owner.register_entity(entity(1));
        assert!(owner.prepare(now, Duration::from_secs(10)).is_none());
    }

    #[test]
    fn neutral_waiter_requires_blocked_entity_and_reverts_on_drop() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (parent, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let waiter = runtime_ids(1)[0];
        parent.observe(RuntimeObservation::ActivityStarted {
            activity: waiter,
            kind: RuntimeActivityKind::Import,
        });
        let invocation = entity(1);
        let participant = owner.register_entity(invocation.clone());
        let neutral = parent.neutral_entity(waiter, &invocation).unwrap();
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        let (child, _, _child_timer) = blocked_timer(&owner, now + Duration::from_secs(60));
        let delegated = child.delegate(&invocation).unwrap();
        assert!(owner.prepare(now, Duration::ZERO).is_some());
        drop(delegated);
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        let _delegated = child.delegate(&invocation).unwrap();
        drop(neutral);
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        let _neutral = parent.neutral_entity(waiter, &invocation).unwrap();
        assert!(owner.prepare(now, Duration::ZERO).is_some());
        drop(participant);
        assert!(owner.prepare(now, Duration::ZERO).is_none());
    }

    #[test]
    fn missing_participant_and_unstarted_delegation_fail_closed() {
        let owner = OwnerSuspension::new();
        let invocation = entity(1);
        let now = Instant::now();
        let (parent, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let waiter = runtime_ids(1)[0];
        parent.observe(RuntimeObservation::ActivityStarted {
            activity: waiter,
            kind: RuntimeActivityKind::Import,
        });
        let store = RuntimeStore::new(owner.clone());
        assert!(parent.neutral_entity(waiter, &invocation).is_none());
        let _participant = owner.register_entity(invocation.clone());
        let _neutral = parent.neutral_entity(waiter, &invocation).unwrap();
        let _delegated = store.delegate(&invocation).unwrap();
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
    }

    #[test]
    fn neutral_runtime_chain_keeps_unknown_and_cancelled_waiters_visible() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let ids = runtime_ids(3);
        for id in &ids {
            store.observe(RuntimeObservation::ActivityStarted {
                activity: *id,
                kind: RuntimeActivityKind::Background,
            });
        }
        let first = store.neutral_runtime(ids[0], &[ids[1]]).unwrap();
        let _second = store.neutral_runtime(ids[1], &[ids[2]]).unwrap();
        assert!(owner.prepare(now, Duration::ZERO).is_none());
        let _wait = store.promise(ids[2]).unwrap();
        let attempt = owner.prepare(now, Duration::ZERO).unwrap();
        drop(first);
        assert!(!owner.commit(&attempt, now, Duration::ZERO));
        assert!(owner.prepare(now, Duration::ZERO).is_none());
    }

    #[test]
    fn delegated_driver_wake_return_and_drop_revoke_suspension() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct ParentWake {
            owner: Arc<OwnerSuspension>,
            called: AtomicBool,
        }
        impl std::task::Wake for ParentWake {
            fn wake(self: Arc<Self>) {
                assert!(self.owner.state.try_lock().is_ok());
                assert!(self.owner.prepare(Instant::now(), Duration::ZERO).is_none());
                self.called.store(true, Ordering::SeqCst);
            }
        }
        for complete in [false, true] {
            let owner = OwnerSuspension::new();
            let runtime = RuntimeStore::new(owner.clone());
            let invocation = entity(1);
            let _participant = owner.register_entity(invocation.clone());
            let mut config = Config::new();
            config.wasm_component_model_async(true);
            let engine = Engine::new(&config).unwrap();
            let mut store = Store::new(&engine, ());
            store
                .as_context_mut()
                .set_runtime_observer(runtime.clone())
                .unwrap();
            let ready = Arc::new(AtomicBool::new(false));
            let saved = Arc::new(Mutex::new(None));
            let ready_inner = ready.clone();
            let saved_inner = saved.clone();
            let observed = runtime.clone();
            let future = store.run_concurrent(async move |accessor| {
                let id = accessor.runtime_activity().unwrap();
                observed.certify_root(id);
                let _timer = observed
                    .timer(id, Instant::now() + Duration::from_secs(60))
                    .unwrap();
                std::future::poll_fn(|cx| {
                    *saved_inner.lock().unwrap() = Some(cx.waker().clone());
                    if ready_inner.load(Ordering::SeqCst) {
                        Poll::Ready(Ok::<(), wasmtime::Error>(()))
                    } else {
                        Poll::Pending
                    }
                })
                .await
            });
            let mut driver = Box::pin(runtime.drive(future, Some(&invocation), None));
            let parent = Arc::new(ParentWake {
                owner: owner.clone(),
                called: AtomicBool::new(false),
            });
            let parent_waker = Waker::from(parent.clone());
            assert!(
                driver
                    .as_mut()
                    .poll(&mut Context::from_waker(&parent_waker))
                    .is_pending()
            );
            let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
            let wake = saved.lock().unwrap().take().unwrap();
            std::thread::spawn(move || wake.wake()).join().unwrap();
            assert!(parent.called.load(Ordering::SeqCst));
            assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
            assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
            assert!(poll_once(driver.as_mut()).is_pending());
            let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
            if complete {
                ready.store(true, Ordering::SeqCst);
                assert!(poll_once(driver.as_mut()).is_ready());
            }
            drop(driver);
            assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
            assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn parked_borrowed_fiber_drop_releases_observer_lock() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use wasmtime::component::{Component, Linker};

        struct HostDrop {
            owner: Arc<OwnerSuspension>,
            dropped: Arc<AtomicBool>,
        }
        impl Drop for HostDrop {
            fn drop(&mut self) {
                assert!(self.owner.state.try_lock().is_ok());
                self.dropped.store(true, Ordering::SeqCst);
            }
        }

        let owner = OwnerSuspension::new();
        let runtime = RuntimeStore::new(owner.clone());
        let mut config = Config::new();
        config
            .wasm_component_model_async(true)
            .concurrency_support(true);
        let engine = Engine::new(&config).unwrap();
        let component = Component::new(
            &engine,
            r#"
            (component
                (import "park" (func $park))
                (core func $park (canon lower (func $park)))
                (core module $m
                    (import "" "park" (func $park))
                    (func (export "run") call $park))
                (core instance $i (instantiate $m
                    (with "" (instance (export "park" (func $park))))))
                (func (export "run") async (canon lift (core func $i "run"))))
        "#,
        )
        .unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let mut linker = Linker::<()>::new(&engine);
        let host_owner = owner.clone();
        let host_entered = entered.clone();
        let host_dropped = dropped.clone();
        linker
            .root()
            .func_wrap_async("park", move |store, (): ()| {
                let guard = HostDrop {
                    owner: host_owner.clone(),
                    dropped: host_dropped.clone(),
                };
                let entered = host_entered.clone();
                Box::new(async move {
                    let _guard = guard;
                    entered.store(true, Ordering::SeqCst);
                    pending::<()>().await;
                    let _ = store.data();
                    Ok(())
                })
            })
            .unwrap();
        let mut store = Store::new(&engine, ());
        store
            .as_context_mut()
            .set_runtime_observer(runtime.clone())
            .unwrap();
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap();
        let mut driver = Box::pin(runtime.drive(run.call_async(&mut store, ()), None, None));
        assert!(poll_once(driver.as_mut()).is_pending());
        assert!(entered.load(Ordering::SeqCst));
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
        drop(driver);
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
        drop(store);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(
            owner.state.lock().unwrap().stores[&runtime.store_id]
                .activities
                .is_empty()
        );
    }

    #[test]
    fn neutral_caller_cannot_hide_body_wake_or_cleanup() {
        let owner = OwnerSuspension::new();
        let invocation = entity(101);
        let _participant = owner.register_entity(invocation.clone());
        let parent_runtime = RuntimeStore::new(owner.clone());
        let child_runtime = RuntimeStore::new(owner.clone());
        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let engine = Engine::new(&config).unwrap();
        let mut parent_store = Store::new(&engine, ());
        let mut child_store = Store::new(&engine, ());
        parent_store
            .as_context_mut()
            .set_runtime_observer(parent_runtime.clone())
            .unwrap();
        child_store
            .as_context_mut()
            .set_runtime_observer(child_runtime.clone())
            .unwrap();
        let saved = Arc::new(Mutex::new(None));
        let saved_child = saved.clone();
        let observed = child_runtime.clone();
        let child = child_store.run_concurrent(async move |accessor| {
            let id = accessor.runtime_activity().unwrap();
            observed.certify_root(id);
            let _timer = observed
                .timer(id, Instant::now() + Duration::from_secs(60))
                .unwrap();
            std::future::poll_fn(|cx| {
                *saved_child.lock().unwrap() = Some(cx.waker().clone());
                Poll::<wasmtime::Result<()>>::Pending
            })
            .await
        });
        let observed = parent_runtime.clone();
        let awaited_invocation = invocation.clone();
        let parent = parent_store.run_concurrent(async move |accessor| {
            let id = accessor.runtime_activity().unwrap();
            observed.certify_root(id);
            let _neutral = observed.neutral_entity(id, &awaited_invocation).unwrap();
            pending::<wasmtime::Result<()>>().await
        });
        let mut child = Box::pin(child_runtime.drive(child, Some(&invocation), None));
        let mut parent = Box::pin(parent_runtime.drive(parent, None, None));
        assert!(poll_once(child.as_mut()).is_pending());
        assert!(poll_once(parent.as_mut()).is_pending());
        let attempt = owner
            .prepare(Instant::now(), Duration::ZERO)
            .expect("blocked entity permits suspension");
        saved.lock().unwrap().take().unwrap().wake();
        assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
        assert!(poll_once(child.as_mut()).is_pending());
        let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
        let cleanup = owner.register_external();
        assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
        drop(cleanup);
        let attempt = owner.prepare(Instant::now(), Duration::ZERO).unwrap();
        drop(child);
        assert!(!owner.commit(&attempt, Instant::now(), Duration::ZERO));
        assert!(owner.prepare(Instant::now(), Duration::ZERO).is_none());
    }

    fn blocked_promise(owner: &Arc<OwnerSuspension>) -> (Arc<RuntimeStore>, SuspensionWait) {
        let ids = runtime_ids(2);
        let store = RuntimeStore::new(owner.clone());
        store.observe(RuntimeObservation::ActivityStarted {
            activity: ids[0],
            kind: RuntimeActivityKind::Root,
        });
        store.certify_root(ids[0]);
        store.observe(RuntimeObservation::ActivityStarted {
            activity: ids[1],
            kind: RuntimeActivityKind::Import,
        });
        let promise = store.promise(ids[1]).unwrap();
        let run = runtime_ids(1)[0];
        let mut state = owner.state.lock().unwrap();
        let driver = &mut state.stores.get_mut(&store.store_id).unwrap().driver;
        driver.runtime_run = Some(run);
        driver.generation = 1;
        driver.blocked_generation = Some(1);
        drop(state);
        (store, promise)
    }

    fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn registered_unpolled_rpc_vetoes_an_otherwise_eligible_owner() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let _rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
        assert!(owner.prepare(now, Duration::from_secs(10)).is_none());
    }

    #[test]
    fn rpc_remote_phase_freezes_grace_and_durable_recheck_deadline() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
        let remote = rpc.revoker();
        let mut task =
            Box::pin(rpc.coordinate(async move {
                remote.remote_wait(pending::<anyhow::Result<()>>()).await
            }));
        assert!(poll_once(task.as_mut()).is_pending());
        let attempt = owner
            .prepare(Instant::now(), Duration::from_secs(10))
            .unwrap();
        assert!(matches!(attempt.wakeup, Wakeup::At(_)));
        assert!(owner.commit(&attempt, Instant::now(), Duration::from_secs(10)));
    }

    #[test]
    fn concurrent_rpc_publication_cannot_overwrite_wake_or_revocation() {
        let owner = OwnerSuspension::new();
        for iteration in 0..1000 {
            let rpc = owner.register_rpc(Duration::ZERO, Duration::from_secs(5));
            rpc.shared.poll.lock().unwrap().remote_candidate = true;
            let barrier = std::sync::Barrier::new(3);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    barrier.wait();
                    rpc.shared.finish_poll(true, 0);
                });
                scope.spawn(|| {
                    barrier.wait();
                    if iteration % 2 == 0 {
                        rpc.revoker().revoke();
                    } else {
                        let wake = Arc::new(RpcWake {
                            shared: rpc.shared.clone(),
                            forward: futures::task::noop_waker(),
                        });
                        futures::task::ArcWake::wake_by_ref(&wake);
                    }
                });
                barrier.wait();
            });
            assert!(matches!(
                owner
                    .state
                    .lock()
                    .unwrap()
                    .activities
                    .get(&rpc.shared.activity),
                Some(ActivityClass::Unknown)
            ));
        }
    }

    #[test]
    fn retained_completed_remote_wait_cannot_lend_grace_to_a_new_phase() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let rpc = store.rpc_activity(Duration::from_secs(5), Duration::from_secs(3));
        let remote = rpc.revoker();
        rpc.shared.poll.lock().unwrap().passive_since = Some(now - Duration::from_secs(10));
        let mut completed = Box::pin(remote.remote_wait(std::future::ready(())));
        assert!(poll_once(completed.as_mut()).is_ready());
        let mut next = Box::pin(remote.remote_wait(pending::<()>()));
        assert!(poll_once(next.as_mut()).is_pending());
        rpc.shared.finish_poll(true, 0);
        assert!(
            owner
                .prepare(Instant::now(), Duration::from_secs(10))
                .is_none()
        );
        assert!(
            owner
                .prepare(
                    Instant::now() + Duration::from_secs(6),
                    Duration::from_secs(10)
                )
                .is_some()
        );
    }

    #[test]
    fn wake_during_rpc_poll_prevents_stale_passive_publication() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
        let remote = rpc.revoker();
        let mut task = Box::pin(rpc.coordinate(async move {
            remote
                .remote_wait(std::future::poll_fn(|cx| {
                    cx.waker().wake_by_ref();
                    Poll::<anyhow::Result<()>>::Pending
                }))
                .await
        }));
        assert!(poll_once(task.as_mut()).is_pending());
        assert!(
            owner
                .prepare(Instant::now(), Duration::from_secs(10))
                .is_none()
        );
    }

    #[test]
    fn remote_repoll_does_not_restart_rpc_suspension_grace() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let rpc = store.rpc_activity(Duration::from_secs(10), Duration::from_secs(5));
        let shared = rpc.shared.clone();
        let remote = rpc.revoker();
        let captured = Arc::new(Mutex::new(None));
        let task_waker = captured.clone();
        let mut task = Box::pin(rpc.coordinate(async move {
            remote
                .remote_wait(std::future::poll_fn(|cx| {
                    *task_waker.lock().unwrap() = Some(cx.waker().clone());
                    Poll::<anyhow::Result<()>>::Pending
                }))
                .await
        }));

        assert!(poll_once(task.as_mut()).is_pending());
        let first = shared.poll.lock().unwrap().passive_since.unwrap();
        captured.lock().unwrap().take().unwrap().wake();
        assert!(poll_once(task.as_mut()).is_pending());
        assert_eq!(shared.poll.lock().unwrap().passive_since, Some(first));
    }

    #[test]
    fn revocation_is_unknown_until_the_rpc_task_is_actually_dropped() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
        let revoker = rpc.revoker();
        let remote = revoker.clone();
        let mut task =
            Box::pin(rpc.coordinate(async move {
                remote.remote_wait(pending::<anyhow::Result<()>>()).await
            }));
        assert!(poll_once(task.as_mut()).is_pending());
        assert!(
            owner
                .prepare(Instant::now(), Duration::from_secs(10))
                .is_some()
        );
        revoker.revoke();
        assert!(
            owner
                .prepare(Instant::now(), Duration::from_secs(10))
                .is_none()
        );
        drop(task);
        assert!(
            owner
                .prepare(Instant::now(), Duration::from_secs(10))
                .is_some()
        );
    }

    #[test]
    fn rpc_commit_rejects_expired_persisted_deadline_and_returns_typed_stop() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        struct Woken(AtomicBool);
        impl std::task::Wake for Woken {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(dropped.clone());
        let woken = Arc::new(Woken(AtomicBool::new(false)));
        let waker = Waker::from(woken.clone());
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
        let remote = rpc.revoker();
        let mut task = Box::pin(rpc.coordinate(async move {
            let _guard = guard;
            remote.remote_wait(pending::<anyhow::Result<()>>()).await
        }));
        assert!(
            task.as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_pending()
        );
        let attempt = owner
            .prepare(now + Duration::from_secs(1), Duration::from_secs(10))
            .unwrap();
        let Wakeup::At(deadline) = attempt.wakeup else {
            panic!("RPC requires a persisted wakeup")
        };
        assert!(!owner.commit(&attempt, deadline, Duration::from_secs(10)));
        assert!(!owner.commit(
            &attempt,
            deadline + Duration::from_millis(1),
            Duration::from_secs(10)
        ));
        let fresh = owner.prepare(deadline, Duration::from_secs(10)).unwrap();
        assert!(owner.commit(
            &fresh,
            deadline + Duration::from_secs(1),
            Duration::from_secs(10)
        ));
        assert!(woken.0.load(Ordering::SeqCst));
        let Poll::Ready(Err(error)) = poll_once(task.as_mut()) else {
            panic!("committed stop must complete task")
        };
        assert!(matches!(
            error.downcast_ref::<InterruptKind>(),
            Some(InterruptKind::Suspend(_))
        ));
        assert!(dropped.load(Ordering::SeqCst));
        assert!(owner.state.lock().unwrap().external_activities.is_empty());
    }

    #[test]
    fn cancelled_remote_phase_cannot_certify_local_pending_work() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(30));
        let rpc = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
        let remote = rpc.revoker();
        let mut task = Box::pin(rpc.coordinate(async move {
            {
                let mut wait = Box::pin(remote.remote_wait(pending::<()>()));
                std::future::poll_fn(|cx| {
                    assert!(wait.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            pending::<anyhow::Result<()>>().await
        }));
        assert!(poll_once(task.as_mut()).is_pending());
        assert!(
            owner
                .prepare(Instant::now(), Duration::from_secs(10))
                .is_none()
        );
    }

    #[test]
    fn first_poll_in_an_unstarted_sibling_excludes_suspension() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_eligible, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(10));
        let sibling = RuntimeStore::new(owner.clone());
        owner
            .state
            .lock()
            .unwrap()
            .stores
            .get_mut(&sibling.store_id)
            .unwrap()
            .driver
            .active_poll = true;

        assert!(owner.prepare(now, Duration::from_secs(1)).is_none());
    }

    #[test]
    fn earliest_asymmetric_deadline_wins_and_short_deadline_refuses() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let late = now + Duration::from_secs(20);
        let early = now + Duration::from_secs(10);
        let (_first, _, _first_timer) = blocked_timer(&owner, late);
        let (_second, _, _second_timer) = blocked_timer(&owner, early);
        assert_eq!(
            owner.prepare(now, Duration::from_secs(5)).unwrap().wakeup,
            Wakeup::At(early)
        );

        let short_owner = OwnerSuspension::new();
        let (_store, _, _timer) = blocked_timer(&short_owner, now + Duration::from_millis(999));
        assert!(short_owner.prepare(now, Duration::from_secs(1)).is_none());
    }

    #[test]
    fn wake_invalidates_prepare_and_blocked_proof() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, run, _timer) = blocked_timer(&owner, now + Duration::from_secs(10));
        let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
        store.observe(RuntimeObservation::DriverInvalidated {
            run,
            generation: 2,
            reason: RuntimeInvalidation::Wake,
        });
        assert!(
            owner
                .state
                .lock()
                .unwrap()
                .stores
                .get(&store.store_id)
                .unwrap()
                .driver
                .blocked_generation
                .is_none()
        );
        assert!(!owner.commit(&attempt, now, Duration::from_secs(1)));
    }

    #[test]
    fn new_work_or_earlier_deadline_between_prepare_and_commit_refuses() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (store, _, _timer) = blocked_timer(&owner, now + Duration::from_secs(20));
        let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
        let new_activity = runtime_ids(1)[0];
        store.observe(RuntimeObservation::ActivityStarted {
            activity: new_activity,
            kind: RuntimeActivityKind::Import,
        });
        assert!(!owner.commit(&attempt, now, Duration::from_secs(1)));

        store.observe(RuntimeObservation::ActivityFinished {
            activity: new_activity,
        });
        let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
        let earlier = runtime_ids(1)[0];
        store.observe(RuntimeObservation::ActivityStarted {
            activity: earlier,
            kind: RuntimeActivityKind::Root,
        });
        store.certify_root(earlier);
        let _earlier_timer = store.timer(earlier, now + Duration::from_secs(10)).unwrap();
        assert!(!owner.commit(&attempt, now, Duration::from_secs(1)));
    }

    #[test]
    fn retired_run_poll_cannot_replace_current_run_and_old_callbacks_are_inert() {
        let owner = OwnerSuspension::new();
        let store = RuntimeStore::new(owner.clone());
        let ids = runtime_ids(2);
        {
            let mut state = owner.state.lock().unwrap();
            let driver = &mut state.stores.get_mut(&store.store_id).unwrap().driver;
            driver.runtime_run = Some(ids[1]);
            driver.runtime_watermark = 7;
            driver.runtime_candidate = Some(8);
            driver.retired_runs.insert(ids[0]);
        }
        let revision = owner.state.lock().unwrap().revision;
        store.observe(RuntimeObservation::DriverInvalidated {
            run: ids[0],
            generation: 9,
            reason: RuntimeInvalidation::Poll,
        });
        store.observe(RuntimeObservation::DriverBlocked {
            run: ids[0],
            generation: 10,
        });
        let state = owner.state.lock().unwrap();
        let driver = &state.stores.get(&store.store_id).unwrap().driver;
        assert_eq!(driver.runtime_run, Some(ids[1]));
        assert_eq!(driver.runtime_watermark, 7);
        assert_eq!(driver.runtime_candidate, Some(8));
        assert_eq!(state.revision, revision);
    }

    #[test]
    fn timer_wait_cancellation_invalidates_eligibility() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_store, _, timer) = blocked_timer(&owner, now + Duration::from_secs(10));
        assert!(owner.prepare(now, Duration::from_secs(1)).is_some());
        drop(timer);
        assert!(owner.prepare(now, Duration::from_secs(1)).is_none());
    }

    #[test]
    fn promise_only_uses_activation_without_a_timer_deadline() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_store, _promise) = blocked_promise(&owner);
        assert_eq!(
            owner.prepare(now, Duration::from_secs(1)).unwrap().wakeup,
            Wakeup::OnActivation
        );
    }

    #[test]
    fn timer_and_promise_use_timer_deadline_and_short_timer_blocks() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_promise_store, _promise) = blocked_promise(&owner);
        let deadline = now + Duration::from_secs(10);
        let (_timer_store, _, _timer) = blocked_timer(&owner, deadline);
        assert_eq!(
            owner.prepare(now, Duration::from_secs(1)).unwrap().wakeup,
            Wakeup::At(deadline)
        );

        let short_owner = OwnerSuspension::new();
        let (_promise_store, _promise) = blocked_promise(&short_owner);
        let (_timer_store, _, _timer) =
            blocked_timer(&short_owner, now + Duration::from_millis(500));
        assert!(short_owner.prepare(now, Duration::from_secs(1)).is_none());
    }

    #[test]
    async fn promise_readiness_and_cancellation_invalidate_classification() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_store, promise) = blocked_promise(&owner);
        assert!(owner.prepare(now, Duration::from_secs(1)).is_some());
        promise
            .wait(std::future::ready(()), pending())
            .await
            .unwrap();
        assert!(owner.prepare(now, Duration::from_secs(1)).is_none());

        let owner = OwnerSuspension::new();
        let (_store, promise) = blocked_promise(&owner);
        assert!(owner.prepare(now, Duration::from_secs(1)).is_some());
        drop(promise);
        assert!(owner.prepare(now, Duration::from_secs(1)).is_none());
    }

    #[test]
    fn coordinator_generations_are_isolated() {
        let first = OwnerSuspension::new();
        let second = OwnerSuspension::new();
        let now = Instant::now();
        let (_first_store, _, _first_timer) = blocked_timer(&first, now + Duration::from_secs(10));
        let (second_store, second_run, _second_timer) =
            blocked_timer(&second, now + Duration::from_secs(10));
        let attempt = first.prepare(now, Duration::from_secs(1)).unwrap();
        second_store.observe(RuntimeObservation::DriverInvalidated {
            run: second_run,
            generation: 2,
            reason: RuntimeInvalidation::Wake,
        });
        assert!(first.commit(&attempt, now, Duration::from_secs(1)));
        assert!(second.prepare(now, Duration::from_secs(1)).is_none());
    }

    #[test]
    async fn interrupt_wins_over_committed_suspend_and_ready_timer() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_store, _, timer) = blocked_timer(&owner, now + Duration::from_secs(10));
        let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
        assert!(owner.commit(&attempt, now, Duration::from_secs(1)));
        let interrupted_at = golem_common::model::Timestamp::now_utc();
        let result = timer
            .wait(
                tokio::time::sleep_until(now.into()),
                std::future::ready(InterruptKind::Interrupt(interrupted_at)),
            )
            .await;
        assert!(
            matches!(result, Err(InterruptKind::Interrupt(timestamp)) if timestamp == interrupted_at)
        );
    }

    #[test]
    async fn committed_suspend_wins_over_ready_timer_without_interrupt() {
        let owner = OwnerSuspension::new();
        let now = Instant::now();
        let (_store, _, timer) = blocked_timer(&owner, now + Duration::from_secs(10));
        let attempt = owner.prepare(now, Duration::from_secs(1)).unwrap();
        assert!(owner.commit(&attempt, now, Duration::from_secs(1)));
        let result = timer
            .wait(tokio::time::sleep_until(now.into()), std::future::pending())
            .await;
        assert!(
            matches!(result, Err(InterruptKind::Suspend(timestamp)) if timestamp == attempt.suspend_at)
        );
    }
}
