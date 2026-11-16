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
//! The guest-task context slot holds only custom-durability scopes, never the invocation.

use golem_common::model::InvocationId;
use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tracing::error;
use wasmtime::component::{TaskGroupHook, TaskGroupId};

/// The invocation a host call is attributed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum InvocationKey {
    /// An agent or entity invocation of this Store, identified by its invocation ID.
    Invocation(InvocationId),
    /// Work outside any invocation, such as destructors and Store-level tasks. It never carries
    /// invocation-bound authority.
    AgentLevel,
}

impl InvocationKey {
    /// The invocation ID the key names, if any.
    pub(crate) fn invocation_id(&self) -> Option<InvocationId> {
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
}

/// The attribution state of one Store, shared by its [`TaskGroupHook`] and its host code.
#[derive(Clone, Default)]
pub(crate) struct InvocationAttribution {
    state: Arc<Mutex<GroupState>>,
}

thread_local! {
    static SCOPED: Cell<Option<(usize, InvocationKey)>> = const { Cell::new(None) };
}

struct ScopedGuard(Option<(usize, InvocationKey)>);

impl Drop for ScopedGuard {
    fn drop(&mut self) {
        SCOPED.with(|scoped| scoped.set(self.0.take()));
    }
}

impl InvocationAttribution {
    fn id(&self) -> usize {
        Arc::as_ptr(&self.state) as usize
    }

    /// The hook to install on the Store this attribution belongs to.
    pub(crate) fn hook(&self) -> impl TaskGroupHook {
        AttributionHook(self.clone())
    }

    /// Polls the future of one host-to-guest export call with `key` armed for its first poll,
    /// in which Wasmtime prepares the call and starts its task group. The group's `handle_start`
    /// consumes the key.
    pub(crate) async fn armed<F: Future>(&self, key: InvocationKey, future: F) -> F::Output {
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
            let _disarm = Disarm(self);
            future.as_mut().poll(cx)
        })
        .await
    }

    /// Polls the instantiation of this Store's component under `key`. Instantiation runs the
    /// core initializer, whose task groups are not armed; they belong to `key`.
    pub(crate) async fn instantiating<F: Future>(
        &self,
        key: InvocationKey,
        future: F,
    ) -> F::Output {
        let mut future = pin!(future);
        std::future::poll_fn(move |cx| {
            self.state.lock().unwrap().instantiating = Some(key);
            let _done = Instantiated(self);
            self.with_scope(Some(key), || future.as_mut().poll(cx))
        })
        .await
    }

    /// The invocation the currently running host code is attributed to.
    pub(crate) fn resolve(&self) -> Option<InvocationKey> {
        {
            let state = self.state.lock().unwrap();
            if let Some(key) = state
                .current
                .and_then(|group| state.groups.get(&group).copied())
            {
                return Some(key);
            }
        }
        let id = self.id();
        SCOPED
            .with(Cell::get)
            .filter(|(owner, _)| *owner == id)
            .map(|(_, key)| key)
    }

    /// Captures the attribution of the running host code for a store task it spawns.
    pub(crate) fn capture(&self) -> CapturedAttribution {
        CapturedAttribution {
            attribution: self.clone(),
            key: self.resolve(),
        }
    }

    /// Runs `f` with `key` as the scoped attribution of this Store.
    pub(crate) fn with_scope<R>(&self, key: Option<InvocationKey>, f: impl FnOnce() -> R) -> R {
        let previous = SCOPED.with(|scoped| scoped.replace(key.map(|key| (self.id(), key))));
        let _guard = ScopedGuard(previous);
        f()
    }

    /// Polls `future` with `key` as the scoped attribution of this Store.
    pub(crate) async fn scoped<F: Future>(
        &self,
        key: Option<InvocationKey>,
        future: F,
    ) -> F::Output {
        let mut future = pin!(future);
        std::future::poll_fn(move |cx| self.with_scope(key, || future.as_mut().poll(cx))).await
    }
}

/// The attribution a store task keeps for its whole run. Store tasks run outside any guest
/// thread, so without it they resolve to no invocation.
#[derive(Clone)]
pub(crate) struct CapturedAttribution {
    attribution: InvocationAttribution,
    key: Option<InvocationKey>,
}

impl CapturedAttribution {
    /// Polls `future` with the captured invocation as the scoped attribution of its Store.
    pub(crate) async fn run<F: Future>(&self, future: F) -> F::Output {
        self.attribution.scoped(self.key, future).await
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
struct Disarm<'a>(&'a InvocationAttribution);

impl Drop for Disarm<'_> {
    fn drop(&mut self) {
        if let Some(key) = self.0.state.lock().unwrap().armed.take() {
            error!(?key, "An armed export call did not start a task group");
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
pub(crate) struct Attributed<P> {
    attribution: InvocationAttribution,
    last: Option<InvocationKey>,
    inner: Pin<Box<P>>,
}

impl<P> Attributed<P> {
    pub(crate) fn new(attribution: InvocationAttribution, inner: P) -> Self {
        Self {
            attribution,
            last: None,
            inner: Box::pin(inner),
        }
    }

    fn poll_attributed<R>(self: Pin<&mut Self>, f: impl FnOnce(Pin<&mut P>) -> R) -> R {
        let this = self.get_mut();
        if let Some(key) = this.attribution.resolve() {
            this.last = Some(key);
        }
        let attribution = this.attribution.clone();
        attribution.with_scope(this.last, || f(this.inner.as_mut()))
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
        self.poll_attributed(|inner| inner.poll_produce(cx, store, destination, finish))
    }

    fn try_into(
        me: Pin<Box<Self>>,
        ty: std::any::TypeId,
    ) -> Result<Box<dyn std::any::Any>, Pin<Box<Self>>> {
        let Self {
            attribution,
            last,
            inner,
        } = *Pin::into_inner(me);
        P::try_into(inner, ty).map_err(|inner| {
            Box::pin(Self {
                attribution,
                last,
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
        self.poll_attributed(|inner| inner.poll_consume(cx, store, source, finish))
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use futures::FutureExt;

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
}
