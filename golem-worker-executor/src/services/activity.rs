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

use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

tokio::task_local! {
    static CURRENT_ACTIVITY: ActivityGuard;
}

/// Admission and quiescence for owned work. Closing and registering use the same lock,
/// so work either belongs to the drain or is rejected before it starts.
pub(crate) struct ActivityGate {
    state: Mutex<ActivityState>,
    drained: Notify,
}

struct ActivityState {
    accepting: bool,
    active: usize,
}

impl ActivityGate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ActivityState {
                accepting: true,
                active: 0,
            }),
            drained: Notify::new(),
        })
    }

    pub(crate) fn try_enter(self: &Arc<Self>) -> Option<ActivityGuard> {
        let mut state = self.state.lock().unwrap();
        if !state.accepting {
            return None;
        }
        state.active += 1;
        Some(ActivityGuard(Arc::new(ActivityTicket(self.clone()))))
    }

    pub(crate) fn is_accepting(&self) -> bool {
        self.state.lock().unwrap().accepting
    }

    pub(crate) fn inherit_or_enter(self: &Arc<Self>) -> Option<ActivityGuard> {
        CURRENT_ACTIVITY
            .try_with(|guard| Arc::ptr_eq(&guard.0.0, self).then(|| guard.clone()))
            .ok()
            .flatten()
            .or_else(|| self.try_enter())
    }

    pub(crate) fn close(&self) {
        self.state.lock().unwrap().accepting = false;
    }

    pub(crate) fn try_close_if_idle(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.accepting || state.active != 0 {
            return false;
        }
        state.accepting = false;
        true
    }

    pub(crate) fn reopen(&self) {
        self.state.lock().unwrap().accepting = true;
    }

    pub(crate) async fn wait_drained(&self) {
        loop {
            let drained = self.drained.notified();
            if self.state.lock().unwrap().active == 0 {
                return;
            }
            drained.await;
        }
    }
}

/// Cloning transfers activity ownership to detached children without admitting new work.
pub(crate) struct ActivityGuard(Arc<ActivityTicket>);

impl ActivityGuard {
    pub(crate) async fn scope<F: Future>(self, future: F) -> F::Output {
        CURRENT_ACTIVITY.scope(self, future).await
    }
}

/// Keeps detached storage tasks in the caller's drain, including their own children.
pub(crate) fn spawn_with_activity<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let activity = CURRENT_ACTIVITY.try_with(Clone::clone).ok();
    tokio::spawn(async move {
        match activity {
            Some(activity) => activity.scope(future).await,
            None => future.await,
        }
    })
}

impl Clone for ActivityGuard {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

struct ActivityTicket(Arc<ActivityGate>);

impl Drop for ActivityTicket {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.active -= 1;
        let drained = state.active == 0;
        drop(state);
        if drained {
            self.0.drained.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ActivityGate, spawn_with_activity};
    use futures::poll;
    use test_r::test;

    #[test]
    fn idle_close_does_not_change_busy_admission_and_fences_future_work() {
        let gate = ActivityGate::new();
        let active = gate.try_enter().unwrap();
        assert!(!gate.try_close_if_idle());
        assert!(gate.try_enter().is_some());
        drop(active);
        assert!(gate.try_close_if_idle());
        assert!(gate.try_enter().is_none());
        assert!(!gate.try_close_if_idle());
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn cancelled_parent_keeps_nested_storage_in_its_drain() {
        let gate = ActivityGate::new();
        let activity = gate.try_enter().unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let child_gate = gate.clone();
        let parent = tokio::spawn(activity.scope(async move {
            spawn_with_activity(async move {
                child_gate.close();
                // Closing rejects unrelated work, but an admitted task can transfer ownership.
                let inherited = child_gate.inherit_or_enter().unwrap();
                drop(inherited);
                spawn_with_activity(async move {
                    released.await.unwrap();
                    finished.send(()).unwrap();
                });
            })
            .await
            .unwrap();
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        }));
        ready.await.unwrap();
        assert!(gate.inherit_or_enter().is_none());
        parent.abort();
        assert!(parent.await.unwrap_err().is_cancelled());
        let drained = gate.wait_drained();
        tokio::pin!(drained);
        assert!(poll!(&mut drained).is_pending());
        release.send(()).unwrap();
        drained.await;
        done.await.unwrap();
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn closing_rejects_new_work_and_drains_detached_children() {
        let gate = ActivityGate::new();
        let parent = gate.try_enter().unwrap();
        let child = parent.clone();
        gate.close();
        assert!(gate.try_enter().is_none());
        let drained = gate.wait_drained();
        tokio::pin!(drained);
        assert!(poll!(&mut drained).is_pending());
        drop(parent);
        assert!(poll!(&mut drained).is_pending());
        drop(child);
        drained.await;
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn reopening_does_not_forget_older_activity() {
        let gate = ActivityGate::new();
        let old = gate.try_enter().unwrap();
        gate.close();
        gate.reopen();
        let new = gate.try_enter().unwrap();
        gate.close();
        drop(new);
        let drained = gate.wait_drained();
        tokio::pin!(drained);
        assert!(poll!(&mut drained).is_pending());
        drop(old);
        drained.await;
    }
}
