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
use golem_common::model::oplog::DurableInvocationId;
use golem_service_base::error::worker_executor::InterruptKind;
use tokio::sync::Notify;
use wasmtime::component::{
    RuntimeActivityId, RuntimeActivityKind, RuntimeInvalidation, RuntimeObservation,
    RuntimeObserver, RuntimeRunId,
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
    blocked_seq: u64,
    delivery_seq: u64,
    runtime_run: Option<RuntimeRunId>,
    runtime_run_retired: bool,
    runtime_watermark: usize,
    runtime_candidate: Option<usize>,
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
        if state.stopped.is_some() {
            return false;
        }
        if matches!(
            state.activities.get(activity),
            Some(ActivityClass::Wait { classification: current, .. }
                | ActivityClass::RpcWait { classification: current, .. })
                if *current == classification
        ) {
            state
                .activities
                .insert(activity.clone(), ActivityClass::Unknown);
            Self::invalidate(&mut state);
        }
        // A finished activity or a replacement classification is not a stopped owner.
        // Leave a newer classification intact while releasing this wait's result.
        true
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
                        store.driver.runtime_run.is_some()
                            && !store.driver.runtime_run_retired
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
    guest_imports: Mutex<GuestImports>,
    guest_blocked: tokio::sync::Notify,
}

/// The guest import calls of one Store, in the order the guest made them.
///
/// Wasmtime registers the activity of a concurrent guest import and polls the host future once
/// inside the guest's call, before the guest can make another call. So the newest import activity
/// is the only one that can still take the next invocation id in guest call order.
#[derive(Default)]
struct GuestImports {
    latest: Option<RuntimeActivityId>,
    invocation_ids: HashMap<RuntimeActivityId, DurableInvocationId>,
}

/// The invocation id of a guest import call, as far as the Store's import order allows.
pub(crate) enum GuestImportInvocationId {
    /// The call already reserved this id for an earlier durable call it made.
    Reserved(DurableInvocationId),
    /// The call is the guest's newest import, so it can reserve the next id now.
    Reservable,
    /// The guest made another import call after this one, so an id reserved now could be out
    /// of guest call order.
    Late,
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
        Arc::new(Self {
            owner,
            store_id,
            guest_imports: Mutex::new(GuestImports::default()),
            guest_blocked: tokio::sync::Notify::new(),
        })
    }

    /// Releases one async input per fresh driver block. Waking this future invalidates the
    /// driver's blocked generation, so delivery consumes an edge, not the current level.
    pub(crate) async fn await_guest_blocked(&self) {
        loop {
            let notified = self.guest_blocked.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self.owner.state.lock().unwrap();
                let driver = &mut state.stores.get_mut(&self.store_id).unwrap().driver;
                if driver.blocked_seq > driver.delivery_seq {
                    driver.delivery_seq = driver.blocked_seq;
                    return;
                }
            }
            notified.await;
        }
    }

    pub(crate) fn guest_import_invocation_id(
        &self,
        activity: RuntimeActivityId,
    ) -> GuestImportInvocationId {
        let imports = self.guest_imports.lock().unwrap();
        if let Some(invocation_id) = imports.invocation_ids.get(&activity) {
            GuestImportInvocationId::Reserved(*invocation_id)
        } else if imports.latest == Some(activity) {
            GuestImportInvocationId::Reservable
        } else {
            GuestImportInvocationId::Late
        }
    }

    pub(crate) fn record_guest_import_invocation_id(
        &self,
        activity: RuntimeActivityId,
        invocation_id: DurableInvocationId,
    ) {
        self.guest_imports
            .lock()
            .unwrap()
            .invocation_ids
            .insert(activity, invocation_id);
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
            let mut blocked = false;
            {
                let mut state = self.owner.state.lock().unwrap();
                if let Some(store) = state.stores.get_mut(&self.store_id) {
                    store.driver.active_poll = false;
                    if result.is_pending() && store.driver.runtime_candidate.is_some() {
                        store.driver.blocked_generation = Some(store.driver.generation);
                        store.driver.blocked_seq += 1;
                        blocked = true;
                    }
                }
            }
            // A probe waker can re-enter the owner state lock.
            if blocked {
                self.guest_blocked.notify_waiters();
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
        match &event {
            RuntimeObservation::ActivityStarted {
                activity,
                kind: RuntimeActivityKind::Import,
            } => self.guest_imports.lock().unwrap().latest = Some(*activity),
            RuntimeObservation::ActivityFinished { activity } => {
                self.guest_imports
                    .lock()
                    .unwrap()
                    .invocation_ids
                    .remove(activity);
            }
            _ => {}
        }
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
                    && !driver.runtime_run_retired
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
                    if driver.runtime_run.is_some_and(|current| {
                        run < current || (run == current && driver.runtime_run_retired)
                    }) {
                        return;
                    }
                    if reason == RuntimeInvalidation::Poll && driver.runtime_run != Some(run) {
                        driver.runtime_run = Some(run);
                        driver.runtime_run_retired = false;
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
                        driver.runtime_run_retired = true;
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
pub(crate) mod tests;
