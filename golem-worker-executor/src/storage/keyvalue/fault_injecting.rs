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

//! Test support: a [`KeyValueStorage`] decorator that fails, or pauses and then fails, chosen
//! operations on demand.
//!
//! Operations are selected by the `api_name` label every call carries, which is how a test names
//! one read on a code path without knowing the key. It lives in the crate rather than under
//! `cfg(test)` so that the in-process test harness and the integration tests can use it too; no
//! configuration constructs it.

use crate::storage::keyvalue::{KeyValueStorage, KeyValueStorageError, KeyValueStorageNamespace};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::future::Future;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// The control handle for a [`FaultInjectingKeyValueStorage`]. Clone it: one clone goes to the
/// decorator, the test keeps another to arm faults and read call counts.
#[derive(Clone, Default)]
pub struct KeyValueStorageFaults {
    state: Arc<Mutex<FaultState>>,
}

#[derive(Default)]
struct FaultState {
    armed: Vec<ArmedFault>,
    calls: HashMap<&'static str, usize>,
    attempts: usize,
    /// When set, a failing operation is applied to the inner storage before its failure is
    /// reported - the lost-response case, where the backend did the work and the answer never
    /// arrived.
    apply_before_failing: bool,
}

/// Which operations a fault selects, by their `api_name` label.
enum Selector {
    Label(&'static str),
    Any,
    AllExcept(&'static [&'static str]),
}

impl Selector {
    fn matches(&self, api_name: &str) -> bool {
        match self {
            Selector::Label(label) => *label == api_name,
            Selector::Any => true,
            Selector::AllExcept(labels) => !labels.contains(&api_name),
        }
    }
}

struct ArmedFault {
    selector: Selector,
    /// Matching calls to let through before the fault starts firing.
    skip: usize,
    /// Matching calls still to fail once it has started.
    remaining: usize,
    error: KeyValueStorageError,
    gate: Option<Arc<GateState>>,
}

#[derive(Default)]
struct GateState {
    entered: Notify,
    release: Notify,
}

/// A paused operation. The test awaits [`entered`](Gate::entered) to learn the operation has
/// reached the fault, does whatever it needs to interleave, then [`release`](Gate::release)s it,
/// at which point the operation fails with the armed error.
pub struct Gate {
    state: Arc<GateState>,
}

impl Gate {
    pub async fn entered(&self) {
        self.state.entered.notified().await;
    }

    pub fn release(&self) {
        self.state.release.notify_one();
    }
}

enum Decision {
    Pass,
    Fail {
        error: KeyValueStorageError,
        apply_first: bool,
    },
}

impl KeyValueStorageFaults {
    /// Fails the next `failures` operations labelled `api_name` with `error`.
    pub fn fail(&self, api_name: &'static str, failures: usize, error: KeyValueStorageError) {
        self.fail_after(api_name, 0, failures, error);
    }

    /// Lets the next `skip` operations labelled `api_name` through, then fails the `failures`
    /// after them with `error`.
    pub fn fail_after(
        &self,
        api_name: &'static str,
        skip: usize,
        failures: usize,
        error: KeyValueStorageError,
    ) {
        self.arm(Selector::Label(api_name), skip, failures, error, None);
    }

    /// Fails the next `failures` operations, whatever their label, with `error`. Pass
    /// `usize::MAX` for a storage that never answers again.
    pub fn fail_all(&self, failures: usize, error: KeyValueStorageError) {
        self.arm(Selector::Any, 0, failures, error, None);
    }

    /// Fails every operation from now on with `error`, except those labelled with one of
    /// `labels` - the shape of a store where one read still answers while the rest do not.
    pub fn fail_all_except(&self, labels: &'static [&'static str], error: KeyValueStorageError) {
        self.arm(Selector::AllExcept(labels), 0, usize::MAX, error, None);
    }

    /// Pauses the next operation labelled `api_name` until the returned gate is released, then
    /// fails it with `error`.
    pub fn gate_next(&self, api_name: &'static str, error: KeyValueStorageError) -> Gate {
        let state = Arc::new(GateState::default());
        self.arm(Selector::Label(api_name), 0, 1, error, Some(state.clone()));
        Gate { state }
    }

    /// Makes every failing operation apply to the inner storage before its failure is reported,
    /// so a retry observes the effect of an attempt whose answer was lost.
    pub fn apply_before_failing(&self) {
        self.state.lock().unwrap().apply_before_failing = true;
    }

    /// Disarms every fault. Operations already paused at a gate stay paused until released.
    pub fn clear(&self) {
        self.state.lock().unwrap().armed.clear();
    }

    /// How many operations labelled `api_name` have been attempted, failed or not.
    pub fn calls(&self, api_name: &'static str) -> usize {
        self.state
            .lock()
            .unwrap()
            .calls
            .get(api_name)
            .copied()
            .unwrap_or(0)
    }

    /// How many operations have been attempted in total, failed or not.
    pub fn attempts(&self) -> usize {
        self.state.lock().unwrap().attempts
    }

    fn arm(
        &self,
        selector: Selector,
        skip: usize,
        remaining: usize,
        error: KeyValueStorageError,
        gate: Option<Arc<GateState>>,
    ) {
        if remaining == 0 {
            return;
        }
        self.state.lock().unwrap().armed.push(ArmedFault {
            selector,
            skip,
            remaining,
            error,
            gate,
        });
    }

    /// Records the call and decides whether it fails. The lock is not held across the gate wait.
    fn decide(&self, api_name: &'static str) -> (Decision, Option<Arc<GateState>>) {
        let mut state = self.state.lock().unwrap();
        *state.calls.entry(api_name).or_default() += 1;
        state.attempts += 1;

        let Some(index) = state
            .armed
            .iter()
            .position(|fault| fault.selector.matches(api_name))
        else {
            return (Decision::Pass, None);
        };
        let fault = &mut state.armed[index];
        if fault.skip > 0 {
            fault.skip -= 1;
            return (Decision::Pass, None);
        }
        fault.remaining = fault.remaining.saturating_sub(1);
        let error = fault.error.clone();
        let gate = fault.gate.clone();
        if fault.remaining == 0 {
            state.armed.remove(index);
        }
        let apply_first = state.apply_before_failing;
        (Decision::Fail { error, apply_first }, gate)
    }

    async fn intercept(&self, api_name: &'static str) -> Decision {
        let (decision, gate) = self.decide(api_name);
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        decision
    }
}

impl Debug for KeyValueStorageFaults {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyValueStorageFaults")
            .finish_non_exhaustive()
    }
}

/// Delegates every operation to `inner` unless a fault armed on its [`KeyValueStorageFaults`]
/// selects it.
#[derive(Debug)]
pub struct FaultInjectingKeyValueStorage {
    inner: Arc<dyn KeyValueStorage + Send + Sync>,
    faults: KeyValueStorageFaults,
}

impl FaultInjectingKeyValueStorage {
    pub fn new(
        inner: Arc<dyn KeyValueStorage + Send + Sync>,
        faults: KeyValueStorageFaults,
    ) -> Self {
        Self { inner, faults }
    }

    /// Runs `op` against the inner storage unless a fault selects it. `op` is built before the
    /// decision but only awaited when the decision says so.
    async fn run<T>(
        &self,
        api_name: &'static str,
        op: impl Future<Output = Result<T, KeyValueStorageError>>,
    ) -> Result<T, KeyValueStorageError> {
        match self.faults.intercept(api_name).await {
            Decision::Pass => op.await,
            Decision::Fail {
                error,
                apply_first: true,
            } => {
                let _ = op.await;
                Err(error)
            }
            Decision::Fail { error, .. } => Err(error),
        }
    }
}

#[async_trait]
impl KeyValueStorage for FaultInjectingKeyValueStorage {
    async fn set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .set(svc_name, api_name, entity_name, namespace, key, value),
        )
        .await
    }

    async fn set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        pairs: &[(&str, &[u8])],
    ) -> Result<(), KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .set_many(svc_name, api_name, entity_name, namespace, pairs),
        )
        .await
    }

    async fn compare_and_set_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        expected: Option<&[u8]>,
        pairs: &[(&str, &[u8])],
    ) -> Result<bool, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner.compare_and_set_many(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                expected,
                pairs,
            ),
        )
        .await
    }

    async fn set_if_not_exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<bool, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .set_if_not_exists(svc_name, api_name, entity_name, namespace, key, value),
        )
        .await
    }

    async fn get(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Option<Bytes>, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .get(svc_name, api_name, entity_name, namespace, key),
        )
        .await
    }

    async fn get_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Arc<[String]>,
    ) -> Result<Vec<Option<Bytes>>, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .get_many(svc_name, api_name, entity_name, namespace, keys),
        )
        .await
    }

    async fn get_all(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .get_all(svc_name, api_name, entity_name, namespace),
        )
        .await
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), KeyValueStorageError> {
        self.run(api_name, self.inner.del(svc_name, api_name, namespace, key))
            .await
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Arc<[String]>,
    ) -> Result<(), KeyValueStorageError> {
        self.run(
            api_name,
            self.inner.del_many(svc_name, api_name, namespace, keys),
        )
        .await
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner.exists(svc_name, api_name, namespace, key),
        )
        .await
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, KeyValueStorageError> {
        self.run(api_name, self.inner.keys(svc_name, api_name, namespace))
            .await
    }

    async fn add_to_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .add_to_set(svc_name, api_name, entity_name, namespace, key, value),
        )
        .await
    }

    async fn remove_from_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .remove_from_set(svc_name, api_name, entity_name, namespace, key, value),
        )
        .await
    }

    async fn members_of_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<Bytes>, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .members_of_set(svc_name, api_name, entity_name, namespace, key),
        )
        .await
    }

    async fn add_to_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        score: f64,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        self.run(
            api_name,
            self.inner.add_to_sorted_set(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                score,
                value,
            ),
        )
        .await
    }

    async fn remove_from_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        value: &[u8],
    ) -> Result<(), KeyValueStorageError> {
        self.run(
            api_name,
            self.inner.remove_from_sorted_set(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                value,
            ),
        )
        .await
    }

    async fn get_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .get_sorted_set(svc_name, api_name, entity_name, namespace, key),
        )
        .await
    }

    async fn query_sorted_set(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<(f64, Bytes)>, KeyValueStorageError> {
        self.run(
            api_name,
            self.inner
                .query_sorted_set(svc_name, api_name, entity_name, namespace, key, min, max),
        )
        .await
    }
}
