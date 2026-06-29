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

//! Per-component memory charge for the shared compiled module.
//!
//! A component's compiled module is loaded into the wasmtime engine once and
//! shared by every worker of that component, so its size must be charged to the
//! memory pool once per resident component rather than once per worker. This
//! registry tracks how many workers of each component are resident and holds a
//! single module-sized charge for as long as at least one is.
//!
//! The charge is represented by an opaque guard obtained from a [`ChargeSource`]
//! (the worker memory pool in production). The first resident worker of a
//! component acquires the charge; the last to unload drops it. The registry is
//! decoupled from the pool via [`ChargeSource`] so the refcounting can be
//! property-tested in isolation.

use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

/// Acquires an opaque, RAII charge of a given byte size from some pool. The
/// returned value releases the charge when dropped.
#[async_trait]
pub trait ChargeSource: Send + Sync {
    type Charge: Send + Sync + 'static;

    async fn acquire_charge(&self, bytes: u64) -> Self::Charge;
}

/// Tracks resident-worker refcounts per component key and holds one module-sized
/// charge per component while any worker of it is resident.
pub struct ComponentChargeRegistry<K, S: ChargeSource> {
    source: S,
    state: Mutex<HashMap<K, Entry<S::Charge>>>,
}

struct Entry<C> {
    refcount: usize,
    /// The held module charge. Always `Some` while `refcount > 0`.
    charge: Option<Arc<C>>,
}

/// Handle representing one worker's residency of a component. While at least one
/// `ComponentChargeGuard` for a key is alive, the registry holds that
/// component's module charge. Dropping the last guard releases it.
pub struct ComponentChargeGuard<K, S: ChargeSource>
where
    K: Eq + Hash + Clone + Send + 'static,
{
    registry: Arc<ComponentChargeRegistry<K, S>>,
    key: K,
}

impl<K, S> Debug for ComponentChargeGuard<K, S>
where
    K: Eq + Hash + Clone + Send + 'static,
    S: ChargeSource,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentChargeGuard").finish()
    }
}

/// Type-erased held component charge. A worker holds one of these for as long as
/// it is resident; dropping it releases the worker's residency of its component.
/// Erasing the source/key types lets non-generic holders store the guard.
pub trait HeldComponentCharge: Send + Sync + Debug {}

impl<K, S> HeldComponentCharge for ComponentChargeGuard<K, S>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    S: ChargeSource + 'static,
    S::Charge: Sync,
{
}

impl<K, S> ComponentChargeRegistry<K, S>
where
    K: Eq + Hash + Clone + Send + 'static,
    S: ChargeSource,
{
    pub fn new(source: S) -> Arc<Self> {
        Arc::new(Self {
            source,
            state: Mutex::new(HashMap::new()),
        })
    }

    /// Register one resident worker of `key` (whose module is `charge_bytes`).
    /// Acquires the module charge if this is the first resident worker of the
    /// component. The returned guard releases residency on drop.
    pub async fn acquire(
        self: &Arc<Self>,
        key: K,
        charge_bytes: u64,
    ) -> ComponentChargeGuard<K, S> {
        // Decide under the lock whether this caller is the one that must acquire
        // the (possibly blocking) charge, so only the first resident worker of a
        // component does so. Acquire the charge outside the lock, then publish it.
        let must_acquire = {
            let mut state = self.state.lock().unwrap();
            let entry = state.entry(key.clone()).or_insert(Entry {
                refcount: 0,
                charge: None,
            });
            entry.refcount += 1;
            entry.refcount == 1
        };

        if must_acquire {
            let charge = Arc::new(self.source.acquire_charge(charge_bytes).await);
            let mut state = self.state.lock().unwrap();
            if let Some(entry) = state.get_mut(&key) {
                // Only publish if still resident (refcount could have churned).
                if entry.refcount > 0 && entry.charge.is_none() {
                    entry.charge = Some(charge);
                }
            }
        }

        ComponentChargeGuard {
            registry: self.clone(),
            key,
        }
    }

    fn release(&self, key: &K) {
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.get_mut(key) {
            entry.refcount = entry.refcount.saturating_sub(1);
            if entry.refcount == 0 {
                // Drop the held charge (returns it to the pool) and forget the
                // component entirely.
                state.remove(key);
            }
        }
    }

    /// Snapshot of the resident-worker refcount per component. Used by the memory
    /// eviction planner to credit a component's shared module to the stop that
    /// removes its last resident worker. A snapshot can race with concurrent
    /// acquires/releases, but the planner is only advisory (it never releases
    /// bytes — the charge guard does that on drop), so a slightly stale count can
    /// at worst make the eviction loop stop scanning a little early or late.
    pub fn charge_refcounts(&self) -> HashMap<K, usize> {
        let state = self.state.lock().unwrap();
        state
            .iter()
            .map(|(key, entry)| (key.clone(), entry.refcount))
            .collect()
    }
}

impl<K, S> Drop for ComponentChargeGuard<K, S>
where
    K: Eq + Hash + Clone + Send + 'static,
    S: ChargeSource,
{
    fn drop(&mut self) {
        self.registry.release(&self.key);
    }
}

impl<K, S: ChargeSource> Debug for ComponentChargeRegistry<K, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentChargeRegistry").finish()
    }
}

#[cfg(test)]
mod tests;
