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

//! Tests for the per-component module charge registry.
//!
//! A [`FakeChargeSource`] models a pool by tracking total charged bytes in an
//! atomic; each charge it hands out decrements that total when dropped. The
//! tests then assert the registry's contract: a component's module is charged
//! exactly once while any worker of it is resident, released when the last
//! unloads, and never leaked or double-charged under concurrent churn.

use super::*;
use proptest::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use test_r::test;

test_r::enable!();

/// A charge that returns `bytes` to the shared counter when dropped.
struct FakeCharge {
    bytes: u64,
    charged_total: Arc<AtomicU64>,
}

impl Drop for FakeCharge {
    fn drop(&mut self) {
        self.charged_total.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct FakeChargeSource {
    charged_total: Arc<AtomicU64>,
    /// Number of times a charge was actually acquired, to detect double-charge.
    acquire_count: Arc<AtomicU64>,
}

impl FakeChargeSource {
    fn new() -> Self {
        Self {
            charged_total: Arc::new(AtomicU64::new(0)),
            acquire_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl ChargeSource for FakeChargeSource {
    type Charge = FakeCharge;

    async fn acquire_charge(&self, bytes: u64) -> FakeCharge {
        self.acquire_count.fetch_add(1, Ordering::SeqCst);
        self.charged_total.fetch_add(bytes, Ordering::SeqCst);
        FakeCharge {
            bytes,
            charged_total: self.charged_total.clone(),
        }
    }
}

const MODULE_BYTES: u64 = 17 * 1024 * 1024;

// ── Single-case unit tests ───────────────────────────────────────────────────

#[test]
async fn first_worker_charges_once_last_releases() {
    let source = FakeChargeSource::new();
    let charged = source.charged_total.clone();
    let count = source.acquire_count.clone();
    let registry = ComponentChargeRegistry::new(source);

    let g1 = registry.acquire("comp-a", MODULE_BYTES).await;
    assert_eq!(charged.load(Ordering::SeqCst), MODULE_BYTES);
    assert_eq!(count.load(Ordering::SeqCst), 1);

    // Second worker of the same component: no additional charge.
    let g2 = registry.acquire("comp-a", MODULE_BYTES).await;
    assert_eq!(charged.load(Ordering::SeqCst), MODULE_BYTES);
    assert_eq!(count.load(Ordering::SeqCst), 1);

    // Dropping one of two keeps the charge.
    drop(g1);
    assert_eq!(charged.load(Ordering::SeqCst), MODULE_BYTES);

    // Dropping the last releases it.
    drop(g2);
    assert_eq!(charged.load(Ordering::SeqCst), 0);
}

#[test]
async fn distinct_components_each_charge_once() {
    let source = FakeChargeSource::new();
    let charged = source.charged_total.clone();
    let registry = ComponentChargeRegistry::new(source);

    let _a = registry.acquire("comp-a", MODULE_BYTES).await;
    let _b = registry.acquire("comp-b", MODULE_BYTES).await;
    let _b2 = registry.acquire("comp-b", MODULE_BYTES).await;

    // Two distinct components → charged twice, regardless of worker count.
    assert_eq!(charged.load(Ordering::SeqCst), 2 * MODULE_BYTES);
}

#[test]
async fn re_acquiring_after_full_release_charges_again() {
    let source = FakeChargeSource::new();
    let charged = source.charged_total.clone();
    let count = source.acquire_count.clone();
    let registry = ComponentChargeRegistry::new(source);

    drop(registry.acquire("comp-a", MODULE_BYTES).await);
    assert_eq!(charged.load(Ordering::SeqCst), 0);

    // A fresh residency after full release acquires the charge again.
    let _g = registry.acquire("comp-a", MODULE_BYTES).await;
    assert_eq!(charged.load(Ordering::SeqCst), MODULE_BYTES);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

// ── Property tests ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Op {
    /// Acquire a guard for component index `usize`.
    Acquire(usize),
    /// Drop the n-th currently-held guard (modulo number held).
    Drop(usize),
}

fn arb_ops(num_components: usize) -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![
            (0..num_components).prop_map(Op::Acquire),
            (0usize..100).prop_map(Op::Drop),
        ],
        0..80,
    )
}

proptest! {
    /// The charged total always equals the sum of `MODULE_BYTES` over the distinct
    /// components that currently have at least one held guard. This is the core
    /// "once per resident component" contract: never per-worker, never leaked,
    /// never double-charged.
    #[test]
    fn charge_tracks_distinct_resident_components(
        num_components in 1usize..6,
        ops in arb_ops(6),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let source = FakeChargeSource::new();
            let charged = source.charged_total.clone();
            let registry = ComponentChargeRegistry::new(source);

            // Held guards keyed by component index.
            let mut held: Vec<(usize, ComponentChargeGuard<&'static str, FakeChargeSource>)> =
                Vec::new();
            let keys: Vec<&'static str> =
                ["c0", "c1", "c2", "c3", "c4", "c5"][..num_components].to_vec();

            for op in ops {
                match op {
                    Op::Acquire(i) => {
                        let i = i % num_components;
                        let guard = registry.acquire(keys[i], MODULE_BYTES).await;
                        held.push((i, guard));
                    }
                    Op::Drop(n) => {
                        if !held.is_empty() {
                            let idx = n % held.len();
                            held.remove(idx);
                        }
                    }
                }

                // Distinct resident component count == charged_total / MODULE_BYTES.
                let mut distinct: Vec<usize> = held.iter().map(|(i, _)| *i).collect();
                distinct.sort_unstable();
                distinct.dedup();
                let expected = distinct.len() as u64 * MODULE_BYTES;
                prop_assert_eq!(
                    charged.load(Ordering::SeqCst),
                    expected,
                    "charged total did not match distinct resident components"
                );
            }

            // After dropping everything, nothing remains charged.
            drop(held);
            prop_assert_eq!(charged.load(Ordering::SeqCst), 0);
            Ok(())
        }).unwrap();
    }
}
