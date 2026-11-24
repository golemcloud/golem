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

//! Host-side attribution of host calls to the invocation whose component-model task makes them.
//!
//! The guest never names its invocation. The host decides it from three sources, in order:
//!
//! 1. The task group of the current component-model thread. Wasmtime creates one group per
//!    host-to-guest call and puts every task that call creates into it; the
//!    [`TaskGroupHook`] records which invocation each group belongs to when the invocation
//!    loop arms the export call, and tracks the group of the thread that runs. Every host call
//!    a guest makes, and every poll of its host future, runs with the calling task's group
//!    current.
//! 2. A scoped value that Golem installs while it polls host work that runs outside any guest
//!    thread: a loop-owned host scope of an invocation, a store task spawned for a call, and a
//!    host stream producer or consumer after its first poll. The value is tagged with its
//!    [`InvocationAttribution`], so work of one Store polled inline inside another Store's poll
//!    does not inherit the outer Store's answer.
//! 3. Nothing: the call is outside any invocation.
//!
//! The guest-task context slot ([`GuestTaskContext`]) holds custom-durability scopes and retry
//! policy scopes, never the invocation. Golem's scoped value also carries the slot value of the
//! task a store task was spawned for, so [`InvocationAttribution::guest_task_context`] answers for
//! store tasks too.
//!
//! The attribution also keeps the authority cut-off of an invocation at its export's return
//! ([`ExportReturned`]): which invocations returned, and which host calls were made before the
//! cut-off applied, so that a call keeps the authority it was made with while the call's host
//! code continues after the return (see [`InvocationAttribution::issued_before_cut_off`]).

use crate::durable_host::durability::GuestTaskContext;
use golem_common::model::InvocationId;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use tracing::{debug, error, warn};
use wasmtime::component::{
    Access, Accessor, HasData, RuntimeActivityId, TaskGroupHook, TaskGroupId,
};

/// The invocation a host call is attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InvocationKey {
    /// An agent or entity invocation of this Store, identified by its invocation ID.
    Invocation(InvocationId),
    /// Work outside any invocation, such as destructors and Store-level tasks. It never carries
    /// invocation-bound authority.
    AgentLevel,
}

impl InvocationKey {
    /// The invocation ID the key names, if any.
    pub fn invocation_id(&self) -> Option<InvocationId> {
        match self {
            Self::Invocation(id) => Some(*id),
            Self::AgentLevel => None,
        }
    }
}

#[derive(Default)]
struct GroupState {
    armed: Option<InvocationKey>,
    /// The invocation of the instantiation in progress, which starts groups without arming.
    instantiating: Option<InvocationKey>,
    groups: HashMap<TaskGroupId, InvocationKey>,
    current: Option<TaskGroupId>,
    /// The return signal of each invocation whose export call can return; see
    /// [`ExportReturned`].
    returned: HashMap<InvocationKey, ExportReturned>,
    /// Whether the guest import that starts next was made before the cut-off of its invocation,
    /// as the Store's call hook decided it.
    next_import_before_cut_off: Option<bool>,
    /// The guest imports in progress that were made before the cut-off of their invocation.
    imports_before_cut_off: HashSet<RuntimeActivityId>,
}

/// The attribution state of one Store, shared by its [`TaskGroupHook`] and its host code.
#[derive(Clone, Default)]
pub struct InvocationAttribution {
    state: Arc<Mutex<GroupState>>,
}

/// The scoped attribution of one Store: its invocation, the context slot value of the task
/// the scoped work belongs to, and whether that task was spawned before its invocation's
/// cut-off.
#[derive(Clone)]
struct Scope {
    store: usize,
    key: Option<InvocationKey>,
    context: Option<Arc<GuestTaskContext>>,
    before_cut_off: bool,
}

thread_local! {
    static SCOPED: RefCell<Option<Scope>> = const { RefCell::new(None) };
    /// The guest import whose host code runs a synchronous Store access now; see
    /// [`with_import_activity`]. Runtime activity IDs are never reused in a process, so the ID
    /// needs no Store tag.
    static IMPORT_ACTIVITY: Cell<Option<RuntimeActivityId>> = const { Cell::new(None) };
}

struct ScopedGuard(Option<Scope>);

impl Drop for ScopedGuard {
    fn drop(&mut self) {
        SCOPED.with(|scoped| *scoped.borrow_mut() = self.0.take());
    }
}

/// Runs `f` with the Store access of `accessor`, so that authority reads and store-task captures
/// in `f` see the cut-off state that the accessor's guest import was made with (see
/// [`InvocationAttribution::issued_before_cut_off`]).
///
/// Every synchronous Store access of an accessor's host code that can run after an await must
/// go through it: the accessor's inherent `with` does not know which import it serves.
pub fn with_import_access<T, D: HasData + ?Sized, R>(
    accessor: &Accessor<T, D>,
    f: impl FnOnce(Access<'_, T, D>) -> R,
) -> R {
    with_import_activity(accessor.runtime_activity(), || accessor.with(f))
}

/// Runs `f`, a synchronous Store access of the host code of the guest import `activity`.
fn with_import_activity<R>(activity: Option<RuntimeActivityId>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<RuntimeActivityId>);
    impl Drop for Restore {
        fn drop(&mut self) {
            IMPORT_ACTIVITY.with(|current| current.set(self.0));
        }
    }
    let _restore = Restore(IMPORT_ACTIVITY.with(|current| current.replace(activity)));
    f()
}

/// The return signal of one invocation's export call: set when the call's result is sent, or when
/// the call ends without one (a trap, a poisoned Store).
///
/// [`ExportReturned::observe`] sets it from the waker of the export future, which `task.return`
/// wakes synchronously inside the intrinsic, before more guest code runs. Every holder reads the
/// same flag:
///
/// - the Store's call hook applies the invocation's authority cut-off at the next host call that
///   the invocation makes;
/// - in replay, the open invocation anchor of the invocation reads it: before it is set, replay
///   reproduces only calls the live run made before the export returned, and their records
///   precede the invocation's terminal; after it, a background task's records can follow the
///   terminal. Setting it only makes cursor readers wait for the terminal read that they did not
///   wait for before, so it does not wake parked readers.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExportReturned(Arc<AtomicBool>);

impl ExportReturned {
    pub(crate) fn mark(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn is_marked(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Polls `future`, the future of the export call, with a waker that sets this signal when it
    /// fires and then wakes the caller's waker.
    ///
    /// The export future waits only on the channel that its task's result is sent through, so its
    /// waker fires when the result is sent or the sender is dropped. A poll that finds the future
    /// still pending after the signal was set means something else woke it, and the signal was set
    /// too early; it is reported as an error.
    pub(crate) async fn observe<F: Future>(&self, future: F) -> F::Output {
        let mut future = pin!(future);
        let signal = Arc::new(ReturnWaker {
            returned: self.clone(),
            outer: futures::task::AtomicWaker::new(),
        });
        let waker = Waker::from(signal.clone());
        std::future::poll_fn(move |cx| {
            signal.outer.register(cx.waker());
            let woken = self.is_marked();
            match future.as_mut().poll(&mut Context::from_waker(&waker)) {
                Poll::Ready(output) => {
                    self.mark();
                    Poll::Ready(output)
                }
                Poll::Pending => {
                    if woken {
                        error!(
                            "An export call was woken without its result; its authority cut-off \
                             applied before the export returned"
                        );
                    }
                    Poll::Pending
                }
            }
        })
        .await
    }
}

struct ReturnWaker {
    returned: ExportReturned,
    outer: futures::task::AtomicWaker,
}

impl Wake for ReturnWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.returned.mark();
        self.outer.wake();
    }
}

impl InvocationAttribution {
    fn id(&self) -> usize {
        Arc::as_ptr(&self.state) as usize
    }

    /// The hook to install on the Store this attribution belongs to.
    pub fn hook(&self) -> impl TaskGroupHook {
        AttributionHook(self.clone())
    }

    /// Polls the future of one host-to-guest export call with `key` armed for its first poll,
    /// in which Wasmtime prepares the call and starts its task group. The group's `handle_start`
    /// consumes the key.
    pub async fn armed<F: Future>(&self, key: InvocationKey, future: F) -> F::Output {
        let mut future = pin!(future);
        let mut first = true;
        std::future::poll_fn(move |cx| {
            if !std::mem::take(&mut first) {
                return future.as_mut().poll(cx);
            }
            let previous = self.state.lock().unwrap().armed.replace(key);
            if let Some(previous) = previous {
                error!(
                    ?previous,
                    ?key,
                    "An armed export call did not start a task group"
                );
            }
            let mut disarm = Disarm {
                attribution: self,
                running: false,
            };
            let poll = future.as_mut().poll(cx);
            disarm.running = poll.is_pending();
            poll
        })
        .await
    }

    /// Polls the instantiation of this Store's component under `key`. Instantiation runs the
    /// core initializer, whose task groups are not armed; they belong to `key`.
    pub async fn instantiating<F: Future>(&self, key: InvocationKey, future: F) -> F::Output {
        let mut future = pin!(future);
        std::future::poll_fn(move |cx| {
            self.state.lock().unwrap().instantiating = Some(key);
            let _done = Instantiated(self);
            self.with_scope(Some(key), || future.as_mut().poll(cx))
        })
        .await
    }

    /// The invocation the currently running host code is attributed to.
    pub fn resolve(&self) -> Option<InvocationKey> {
        {
            let state = self.state.lock().unwrap();
            if let Some(key) = state
                .current
                .and_then(|group| state.groups.get(&group).copied())
            {
                return Some(key);
            }
        }
        self.scope().and_then(|scope| scope.key)
    }

    /// The context slot value of the task the running host code belongs to: the calling guest
    /// task's, or the value a store task captured when it was spawned.
    pub fn guest_task_context(&self) -> Option<Arc<GuestTaskContext>> {
        if self.state.lock().unwrap().current.is_none() {
            return self.scope().and_then(|scope| scope.context);
        }
        wasmtime::component::current_guest_task_context::<GuestTaskContext>().unwrap_or_else(
            |err| {
                warn!(%err, "The guest task context of a host call is unavailable");
                None
            },
        )
    }

    fn scope(&self) -> Option<Scope> {
        let id = self.id();
        SCOPED.with(|scoped| scoped.borrow().clone().filter(|scope| scope.store == id))
    }

    /// Captures the attribution of the running host code for a store task it spawns. The task
    /// keeps the cut-off state of the code that spawns it: a task spawned before its invocation's
    /// cut-off keeps the authority the invocation had before it.
    pub fn capture(&self) -> CapturedAttribution {
        let key = self.resolve();
        let before_cut_off = self.issued_before_cut_off()
            || key.is_some_and(|key| {
                key != InvocationKey::AgentLevel && !self.export_returned_is_marked(key)
            });
        CapturedAttribution {
            attribution: self.clone(),
            key,
            context: self.guest_task_context(),
            before_cut_off,
        }
    }

    /// The return signal of the export call of `key`, created at the first request. Replay hands
    /// it to the invocation's open anchor, and the invocation loop observes the export call with
    /// it ([`ExportReturned::observe`]).
    pub(crate) fn export_returned(&self, key: InvocationKey) -> ExportReturned {
        self.state
            .lock()
            .unwrap()
            .returned
            .entry(key)
            .or_default()
            .clone()
    }

    /// Whether the export call of `key` returned.
    pub(crate) fn export_returned_is_marked(&self, key: InvocationKey) -> bool {
        self.state
            .lock()
            .unwrap()
            .returned
            .get(&key)
            .is_some_and(ExportReturned::is_marked)
    }

    /// Forgets the return signals of every invocation other than `key`, when the loop begins
    /// `key`: the loop runs one invocation at a time, and work of an earlier invocation reads the
    /// running invocation's entry.
    pub(crate) fn retain_export_returned(&self, key: InvocationKey) {
        self.state
            .lock()
            .unwrap()
            .returned
            .retain(|existing, _| *existing == key);
    }

    /// Called from the Store's call hook on `CallHook::CallingHost`: whether the guest import
    /// that starts next is made before the cut-off of its invocation.
    pub(crate) fn mark_next_import_before_cut_off(&self, before_cut_off: bool) {
        self.state.lock().unwrap().next_import_before_cut_off = Some(before_cut_off);
    }

    /// Called from the runtime observer when a guest import activity starts.
    pub(crate) fn import_started(&self, activity: RuntimeActivityId) {
        let mut state = self.state.lock().unwrap();
        if state.next_import_before_cut_off.take() == Some(true) {
            state.imports_before_cut_off.insert(activity);
        }
    }

    /// Called from the runtime observer when an activity finishes.
    pub(crate) fn activity_finished(&self, activity: RuntimeActivityId) {
        self.state
            .lock()
            .unwrap()
            .imports_before_cut_off
            .remove(&activity);
    }

    /// Whether the running host code belongs to a call made before its invocation's cut-off,
    /// whose own code continues after the cut-off: a guest import made before it, whose
    /// synchronous Store access runs in [`with_import_activity`], or a store task spawned before
    /// it. Such code keeps the scope card that the cut-off hides. Any other code, which includes
    /// the synchronous part of every host call, sees the invocation's current state.
    pub fn issued_before_cut_off(&self) -> bool {
        let import_before_cut_off = IMPORT_ACTIVITY.with(Cell::get).is_some_and(|activity| {
            self.state
                .lock()
                .unwrap()
                .imports_before_cut_off
                .contains(&activity)
        });
        import_before_cut_off || self.scope().is_some_and(|scope| scope.before_cut_off)
    }

    /// Runs `f` with `key` as the scoped attribution of this Store.
    pub fn with_scope<R>(&self, key: Option<InvocationKey>, f: impl FnOnce() -> R) -> R {
        self.with_task_scope(key, None, false, f)
    }

    fn with_task_scope<R>(
        &self,
        key: Option<InvocationKey>,
        context: Option<Arc<GuestTaskContext>>,
        before_cut_off: bool,
        f: impl FnOnce() -> R,
    ) -> R {
        let scope = (key.is_some() || context.is_some()).then(|| Scope {
            store: self.id(),
            key,
            context,
            before_cut_off,
        });
        let previous = SCOPED.with(|scoped| scoped.replace(scope));
        let _guard = ScopedGuard(previous);
        f()
    }

    /// Polls `future` with `key` as the scoped attribution of this Store.
    pub async fn scoped<F: Future>(&self, key: Option<InvocationKey>, future: F) -> F::Output {
        let mut future = pin!(future);
        std::future::poll_fn(move |cx| self.with_scope(key, || future.as_mut().poll(cx))).await
    }
}

/// The attribution a store task keeps for its whole run. Store tasks run outside any guest
/// thread, so without it they resolve to no invocation.
#[derive(Clone)]
pub struct CapturedAttribution {
    attribution: InvocationAttribution,
    key: Option<InvocationKey>,
    context: Option<Arc<GuestTaskContext>>,
    before_cut_off: bool,
}

impl CapturedAttribution {
    /// Runs a context window with the invocation, task context and cut-off state captured at
    /// initiation.
    pub(crate) fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        self.attribution
            .with_task_scope(self.key, self.context.clone(), self.before_cut_off, f)
    }

    /// Polls `future` with the captured invocation, task context and cut-off state as the scoped
    /// attribution of its Store.
    pub async fn run<F: Future>(&self, future: F) -> F::Output {
        let mut future = pin!(future);
        std::future::poll_fn(move |cx| self.with(|| future.as_mut().poll(cx))).await
    }
}

/// Ends an instantiation poll, also when it panics.
struct Instantiated<'a>(&'a InvocationAttribution);

impl Drop for Instantiated<'_> {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().instantiating = None;
    }
}

/// Clears the armed key after the first poll of an export call, also when that poll panics.
/// A call still running without having started a task group is a missed path. A call that
/// ended in its first poll, such as one Wasmtime refused to enter, ran no guest code.
struct Disarm<'a> {
    attribution: &'a InvocationAttribution,
    running: bool,
}

impl Drop for Disarm<'_> {
    fn drop(&mut self) {
        if let Some(key) = self.attribution.state.lock().unwrap().armed.take() {
            if self.running {
                error!(?key, "An armed export call did not start a task group");
            } else {
                debug!(
                    ?key,
                    "An armed export call ended before it started a task group"
                );
            }
        }
    }
}

struct AttributionHook(InvocationAttribution);

impl TaskGroupHook for AttributionHook {
    fn handle_start(&mut self, id: TaskGroupId) -> wasmtime::Result<()> {
        let mut state = self.0.state.lock().unwrap();
        let key = match state.armed.take().or(state.instantiating) {
            Some(key) => key,
            None => {
                error!("An export call started a task group without an armed invocation");
                InvocationKey::AgentLevel
            }
        };
        state.groups.insert(id, key);
        Ok(())
    }

    fn handle_enter(&mut self, id: TaskGroupId) -> wasmtime::Result<()> {
        self.0.state.lock().unwrap().current = Some(id);
        Ok(())
    }

    fn handle_exit(&mut self, id: TaskGroupId) -> wasmtime::Result<()> {
        let mut state = self.0.state.lock().unwrap();
        if state.current == Some(id) {
            state.current = None;
        }
        Ok(())
    }

    fn handle_finish(&mut self, id: TaskGroupId) -> wasmtime::Result<()> {
        let mut state = self.0.state.lock().unwrap();
        state.groups.remove(&id);
        if state.current == Some(id) {
            state.current = None;
        }
        Ok(())
    }
}

/// Wraps a host stream producer or consumer so polls after its first one, which the event loop
/// makes outside any guest thread, keep the attribution of the read or write that started.
pub struct Attributed<P> {
    attribution: InvocationAttribution,
    last: Option<InvocationKey>,
    context: Option<Arc<GuestTaskContext>>,
    inner: Pin<Box<P>>,
}

impl<P> Attributed<P> {
    pub fn new(attribution: InvocationAttribution, inner: P) -> Self {
        Self {
            attribution,
            last: None,
            context: None,
            inner: Box::pin(inner),
        }
    }

    fn poll_attributed<'s, D, R>(
        self: Pin<&mut Self>,
        mut store: wasmtime::StoreContextMut<'s, D>,
        f: impl FnOnce(Pin<&mut P>, wasmtime::StoreContextMut<'s, D>) -> R,
    ) -> R {
        let this = self.get_mut();
        if let Some(key) = this.attribution.resolve() {
            this.last = Some(key);
        }
        if this.attribution.state.lock().unwrap().current.is_some() {
            this.context = store
                .guest_task_context::<GuestTaskContext>()
                .unwrap_or_else(|err| {
                    warn!(%err, "The guest task context of a stream read or write is unavailable");
                    None
                });
        }
        let attribution = this.attribution.clone();
        attribution.with_task_scope(this.last, this.context.clone(), false, || {
            f(this.inner.as_mut(), store)
        })
    }
}

impl<D, P> wasmtime::component::StreamProducer<D> for Attributed<P>
where
    P: wasmtime::component::StreamProducer<D>,
{
    type Item = P::Item;
    type Buffer = P::Buffer;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: wasmtime::StoreContextMut<'a, D>,
        destination: wasmtime::component::Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<wasmtime::component::StreamResult>> {
        self.poll_attributed(store, |inner, store| {
            inner.poll_produce(cx, store, destination, finish)
        })
    }

    fn try_into(
        me: Pin<Box<Self>>,
        ty: std::any::TypeId,
    ) -> Result<Box<dyn std::any::Any>, Pin<Box<Self>>> {
        let Self {
            attribution,
            last,
            context,
            inner,
        } = *Pin::into_inner(me);
        P::try_into(inner, ty).map_err(|inner| {
            Box::pin(Self {
                attribution,
                last,
                context,
                inner,
            })
        })
    }
}

impl<D, P> wasmtime::component::StreamConsumer<D> for Attributed<P>
where
    P: wasmtime::component::StreamConsumer<D>,
{
    type Item = P::Item;

    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: wasmtime::StoreContextMut<D>,
        source: wasmtime::component::Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<wasmtime::component::StreamResult>> {
        self.poll_attributed(store, |inner, store| {
            inner.poll_consume(cx, store, source, finish)
        })
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use futures::FutureExt;

    #[test]
    fn a_captured_context_window_preserves_its_origin_and_restores_the_caller() {
        let attribution = InvocationAttribution::default();
        let origin = InvocationKey::Invocation(InvocationId(uuid::Uuid::new_v4()));
        let caller = InvocationKey::Invocation(InvocationId(uuid::Uuid::new_v4()));
        let context = Arc::new(GuestTaskContext::default());
        let captured =
            attribution.with_task_scope(Some(origin), Some(context.clone()), false, || {
                attribution.capture()
            });

        attribution.with_scope(Some(caller), || {
            captured.with(|| {
                assert_eq!(attribution.resolve(), Some(origin));
                assert!(Arc::ptr_eq(
                    &attribution.guest_task_context().unwrap(),
                    &context,
                ));
            });
            assert_eq!(attribution.resolve(), Some(caller));
            assert!(attribution.guest_task_context().is_none());
        });
        assert_eq!(attribution.resolve(), None);
        assert!(attribution.guest_task_context().is_none());
    }

    #[test]
    fn a_scoped_invocation_answers_only_for_its_store_and_only_while_polled() {
        let outer = InvocationAttribution::default();
        let inner = InvocationAttribution::default();
        let a = InvocationKey::Invocation(InvocationId(uuid::Uuid::new_v4()));
        let b = InvocationKey::Invocation(InvocationId(uuid::Uuid::new_v4()));

        let mut yielded = false;
        let mut future = Box::pin(outer.scoped(
            Some(a),
            std::future::poll_fn(|cx| {
                assert_eq!(outer.resolve(), Some(a));
                assert_eq!(inner.resolve(), None);
                inner.with_scope(Some(b), || {
                    assert_eq!(inner.resolve(), Some(b));
                });
                assert_eq!(outer.resolve(), Some(a));
                assert_eq!(inner.resolve(), None);
                if std::mem::replace(&mut yielded, true) {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }),
        ));

        assert!((&mut future).now_or_never().is_none());
        assert_eq!(outer.resolve(), None);
        assert!(future.now_or_never().is_some());
        assert_eq!(outer.resolve(), None);
    }

    /// A waker that counts its wakes.
    struct CountingWaker(std::sync::atomic::AtomicUsize);

    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counting_waker() -> (Arc<CountingWaker>, Waker) {
        let counter = Arc::new(CountingWaker(Default::default()));
        let waker = Waker::from(counter.clone());
        (counter, waker)
    }

    #[test]
    fn the_return_signal_is_set_by_the_wake_of_the_result_before_the_next_poll() {
        let returned = ExportReturned::default();
        let (sender, receiver) = futures::channel::oneshot::channel::<u32>();
        let mut call = Box::pin(returned.observe(receiver));
        let (counter, waker) = counting_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(call.as_mut().poll(&mut cx).is_pending());
        assert!(!returned.is_marked(), "nothing returned yet");

        sender.send(7).unwrap();
        assert!(
            returned.is_marked(),
            "the send sets the signal synchronously, before the export future is polled again"
        );
        assert_eq!(counter.0.load(Ordering::SeqCst), 1, "the wake is forwarded");

        assert!(matches!(call.as_mut().poll(&mut cx), Poll::Ready(Ok(7))));
        assert!(returned.is_marked());
    }

    #[test]
    fn the_return_signal_is_set_when_the_result_sender_is_dropped() {
        let returned = ExportReturned::default();
        let (sender, receiver) = futures::channel::oneshot::channel::<u32>();
        let mut call = Box::pin(returned.observe(receiver));
        let (counter, waker) = counting_waker();
        let mut cx = Context::from_waker(&waker);

        assert!(call.as_mut().poll(&mut cx).is_pending());
        drop(sender);
        assert!(
            returned.is_marked(),
            "a call that ends without a result also ends"
        );
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
        assert!(matches!(call.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
    }

    #[test]
    fn the_return_signal_forwards_the_wake_to_the_latest_poller() {
        let returned = ExportReturned::default();
        let (sender, receiver) = futures::channel::oneshot::channel::<u32>();
        let mut call = Box::pin(returned.observe(receiver));
        let (first, first_waker) = counting_waker();
        let (second, second_waker) = counting_waker();

        assert!(
            call.as_mut()
                .poll(&mut Context::from_waker(&first_waker))
                .is_pending()
        );
        assert!(
            call.as_mut()
                .poll(&mut Context::from_waker(&second_waker))
                .is_pending()
        );
        sender.send(1).unwrap();
        assert_eq!(first.0.load(Ordering::SeqCst), 0);
        assert_eq!(second.0.load(Ordering::SeqCst), 1);
    }

    /// Distinct runtime activity identities, taken from a Store that only spawns tasks.
    fn activities(count: usize) -> Vec<RuntimeActivityId> {
        use wasmtime::component::RuntimeObservation;
        use wasmtime::{AsContextMut, Config, Engine, Store};

        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let mut store = Store::new(&Engine::new(&config).unwrap(), ());
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
            store
                .as_context_mut()
                .spawn(async |_: &Accessor<()>| {
                    std::future::pending::<wasmtime::Result<()>>().await
                })
                .unwrap();
        }
        let ids = ids.lock().unwrap().clone();
        assert_eq!(ids.len(), count);
        ids
    }

    #[test]
    fn an_import_made_before_the_cut_off_keeps_its_state_until_it_finishes() {
        let attribution = InvocationAttribution::default();
        let ids = activities(2);
        let (before, after) = (ids[0], ids[1]);

        attribution.mark_next_import_before_cut_off(true);
        attribution.import_started(before);
        attribution.mark_next_import_before_cut_off(false);
        attribution.import_started(after);

        assert!(!attribution.issued_before_cut_off(), "outside any import");
        assert!(with_import_activity(Some(before), || attribution.issued_before_cut_off()));
        assert!(!with_import_activity(Some(after), || attribution.issued_before_cut_off()));
        assert!(
            !attribution.issued_before_cut_off(),
            "the import's Store access ended"
        );

        attribution.activity_finished(before);
        assert!(!with_import_activity(Some(before), || attribution.issued_before_cut_off()));
        assert!(
            !InvocationAttribution::default().issued_before_cut_off(),
            "another Store does not know the import"
        );
    }

    #[test]
    fn a_decision_without_an_import_activity_does_not_carry_to_the_next_import() {
        let attribution = InvocationAttribution::default();
        let ids = activities(1);

        // The call hook also runs for intrinsics such as `task.return`, which start no import.
        attribution.mark_next_import_before_cut_off(true);
        attribution.mark_next_import_before_cut_off(false);
        attribution.import_started(ids[0]);
        assert!(!with_import_activity(Some(ids[0]), || attribution.issued_before_cut_off()));
    }

    #[test]
    fn a_store_task_keeps_the_cut_off_state_of_the_code_that_spawned_it() {
        let attribution = InvocationAttribution::default();
        let key = InvocationKey::Invocation(InvocationId(uuid::Uuid::new_v4()));
        let returned = attribution.export_returned(key);

        let spawned_before = attribution.with_scope(Some(key), || attribution.capture());
        returned.mark();
        let spawned_after = attribution.with_scope(Some(key), || attribution.capture());

        let observe = |captured: &CapturedAttribution| {
            captured
                .run(std::future::ready(()).map(|()| attribution.issued_before_cut_off()))
                .now_or_never()
                .unwrap()
        };
        assert!(observe(&spawned_before));
        assert!(!observe(&spawned_after));
        let spawned_by_spawned_before = spawned_before
            .run(std::future::ready(()).map(|()| attribution.capture()))
            .now_or_never()
            .unwrap();
        assert!(
            observe(&spawned_by_spawned_before),
            "a task spawned by a task spawned before the cut-off keeps its state"
        );
    }

    #[test]
    fn a_store_task_spawned_after_the_return_by_an_import_made_before_it_keeps_its_state() {
        use wasmtime::{AsContextMut, Config, Engine, Store};

        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let mut store = Store::new(&Engine::new(&config).unwrap(), ());
        store
            .as_context_mut()
            .set_runtime_observer(Arc::new(|_| {}))
            .unwrap();
        let attribution = InvocationAttribution::default();
        let key = InvocationKey::Invocation(InvocationId(uuid::Uuid::new_v4()));
        let returned = attribution.export_returned(key);

        let (through_import, plain) =
            futures::executor::block_on(store.run_concurrent(async |accessor: &Accessor<()>| {
                // The accessor serves an import that the call hook saw before the return, and
                // its host code reaches the capture only after the export returned.
                let activity = accessor
                    .runtime_activity()
                    .expect("a Store with a runtime observer identifies the accessor");
                attribution.mark_next_import_before_cut_off(true);
                attribution.import_started(activity);
                returned.mark();
                attribution.with_scope(Some(key), || {
                    (
                        with_import_access(accessor, |_| attribution.capture()),
                        accessor.with(|_| attribution.capture()),
                    )
                })
            }))
            .unwrap();

        assert!(through_import.before_cut_off);
        assert!(
            !plain.before_cut_off,
            "without the import's activity, the capture sees only the current cut-off state"
        );
    }
}
