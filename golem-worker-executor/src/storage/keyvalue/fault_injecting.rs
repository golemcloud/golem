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
}

struct ArmedFault {
    api_name: &'static str,
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
        self.state.lock().unwrap().armed.push(ArmedFault {
            api_name,
            skip,
            remaining: failures,
            error,
            gate: None,
        });
    }

    /// Pauses the next operation labelled `api_name` until the returned gate is released, then
    /// fails it with `error`.
    pub fn gate_next(&self, api_name: &'static str, error: KeyValueStorageError) -> Gate {
        let state = Arc::new(GateState::default());
        self.state.lock().unwrap().armed.push(ArmedFault {
            api_name,
            skip: 0,
            remaining: 1,
            error,
            gate: Some(state.clone()),
        });
        Gate { state }
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

    /// Records the call and decides whether it fails. The lock is not held across the gate wait.
    fn decide(
        &self,
        api_name: &'static str,
    ) -> Option<(KeyValueStorageError, Option<Arc<GateState>>)> {
        let mut state = self.state.lock().unwrap();
        *state.calls.entry(api_name).or_default() += 1;

        let index = state
            .armed
            .iter()
            .position(|fault| fault.api_name == api_name)?;
        let fault = &mut state.armed[index];
        if fault.skip > 0 {
            fault.skip -= 1;
            return None;
        }
        fault.remaining -= 1;
        let decision = (fault.error.clone(), fault.gate.clone());
        if fault.remaining == 0 {
            state.armed.remove(index);
        }
        Some(decision)
    }

    async fn intercept(&self, api_name: &'static str) -> Result<(), KeyValueStorageError> {
        match self.decide(api_name) {
            None => Ok(()),
            Some((error, gate)) => {
                if let Some(gate) = gate {
                    gate.entered.notify_one();
                    gate.release.notified().await;
                }
                Err(error)
            }
        }
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
        self.faults.intercept(api_name).await?;
        self.inner
            .set(svc_name, api_name, entity_name, namespace, key, value)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .set_many(svc_name, api_name, entity_name, namespace, pairs)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .compare_and_set_many(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                expected,
                pairs,
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
        self.faults.intercept(api_name).await?;
        self.inner
            .set_if_not_exists(svc_name, api_name, entity_name, namespace, key, value)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .get(svc_name, api_name, entity_name, namespace, key)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .get_many(svc_name, api_name, entity_name, namespace, keys)
            .await
    }

    async fn get_all(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        entity_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<(String, Bytes)>, KeyValueStorageError> {
        self.faults.intercept(api_name).await?;
        self.inner
            .get_all(svc_name, api_name, entity_name, namespace)
            .await
    }

    async fn del(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<(), KeyValueStorageError> {
        self.faults.intercept(api_name).await?;
        self.inner.del(svc_name, api_name, namespace, key).await
    }

    async fn del_many(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        keys: Arc<[String]>,
    ) -> Result<(), KeyValueStorageError> {
        self.faults.intercept(api_name).await?;
        self.inner
            .del_many(svc_name, api_name, namespace, keys)
            .await
    }

    async fn exists(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
        key: &str,
    ) -> Result<bool, KeyValueStorageError> {
        self.faults.intercept(api_name).await?;
        self.inner.exists(svc_name, api_name, namespace, key).await
    }

    async fn keys(
        &self,
        svc_name: &'static str,
        api_name: &'static str,
        namespace: KeyValueStorageNamespace,
    ) -> Result<Vec<String>, KeyValueStorageError> {
        self.faults.intercept(api_name).await?;
        self.inner.keys(svc_name, api_name, namespace).await
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
        self.faults.intercept(api_name).await?;
        self.inner
            .add_to_set(svc_name, api_name, entity_name, namespace, key, value)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .remove_from_set(svc_name, api_name, entity_name, namespace, key, value)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .members_of_set(svc_name, api_name, entity_name, namespace, key)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .add_to_sorted_set(
                svc_name,
                api_name,
                entity_name,
                namespace,
                key,
                score,
                value,
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
        self.faults.intercept(api_name).await?;
        self.inner
            .remove_from_sorted_set(svc_name, api_name, entity_name, namespace, key, value)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .get_sorted_set(svc_name, api_name, entity_name, namespace, key)
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
        self.faults.intercept(api_name).await?;
        self.inner
            .query_sorted_set(svc_name, api_name, entity_name, namespace, key, min, max)
            .await
    }
}
