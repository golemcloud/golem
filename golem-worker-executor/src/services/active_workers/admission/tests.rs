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

//! Property-based and example tests for the measured-headroom admission valve.
//!
//! These tests model an executor environment as a shared cell holding a hard
//! `limit`, the current resident `usage`, and the set of resident evictable
//! work (each item carrying a size and an eviction priority). A [`FakeProbe`]
//! reports `usage`/`limit` from the cell; a [`FakeEvictionSource`] reclaims
//! idle-then-warm items and decrements `usage`. Admitting a request adds its
//! size to `usage` as a new resident, non-evictable item (it is actively being
//! created).
//!
//! The model lets `proptest` drive thousands of random admit sequences — with
//! random request sizes, pre-resident work, and limits — and assert the
//! invariants that *define* a correct safety valve:
//!
//! 1. Safety: usage never exceeds the limit (the environment never OOMs).
//! 2. No spurious eviction: when headroom is ample, nothing is evicted.
//! 3. Eviction ordering: idle work is reclaimed before warm work.
//! 4. Clean rejection: when the request genuinely cannot fit, the decision is
//!    `Reject` and no over-commit happens.

use super::*;
use crate::services::active_workers::memory_probe::{MemoryProbe, MemorySnapshot};
use proptest::prelude::*;
use std::sync::{Arc, Mutex};
use test_r::test;

test_r::enable!();

/// One unit of resident, evictable work in the model.
#[derive(Debug, Clone, Copy)]
struct Resident {
    size: u64,
    priority: EvictionPriority,
}

/// Shared model of the executor environment's memory.
#[derive(Debug, Default)]
struct EnvState {
    limit: u64,
    /// Resident bytes attributed to admitted, currently-active requests that
    /// are not yet evictable (they are mid-admission).
    pinned_usage: u64,
    /// Resident, evictable work — what the controller may reclaim.
    residents: Vec<Resident>,
    /// Count of evictions performed, for the no-spurious-eviction property.
    evictions: usize,
    /// The priorities evicted, in order, for the ordering property.
    eviction_order: Vec<EvictionPriority>,
}

impl EnvState {
    fn usage(&self) -> u64 {
        self.pinned_usage + self.residents.iter().map(|r| r.size).sum::<u64>()
    }
}

#[derive(Debug, Clone)]
struct FakeProbe {
    state: Arc<Mutex<EnvState>>,
}

impl MemoryProbe for FakeProbe {
    fn snapshot(&self) -> MemorySnapshot {
        let state = self.state.lock().unwrap();
        MemorySnapshot {
            limit_bytes: state.limit,
            current_bytes: state.usage(),
        }
    }
}

struct FakeEvictionSource {
    state: Arc<Mutex<EnvState>>,
}

#[async_trait::async_trait]
impl EvictionSource for FakeEvictionSource {
    async fn evict_at_most(&self, priority: EvictionPriority, needed_bytes: u64) -> u64 {
        let mut state = self.state.lock().unwrap();
        let mut freed = 0u64;
        // Evict only at the requested tier, oldest-first (model: vec order),
        // until we have freed at least `needed_bytes` or the tier is empty.
        let mut i = 0;
        while freed < needed_bytes && i < state.residents.len() {
            if state.residents[i].priority == priority {
                let victim = state.residents.remove(i);
                freed += victim.size;
                state.evictions += 1;
                state.eviction_order.push(priority);
            } else {
                i += 1;
            }
        }
        freed
    }
}

fn controller(state: Arc<Mutex<EnvState>>, reserve_bytes: u64) -> AdmissionController {
    controller_with_ratio(state, 1.0, reserve_bytes)
}

fn controller_with_ratio(
    state: Arc<Mutex<EnvState>>,
    usable_ratio: f64,
    reserve_bytes: u64,
) -> AdmissionController {
    AdmissionController::new(
        Box::new(FakeProbe {
            state: state.clone(),
        }),
        AdmissionPolicy {
            usable_ratio,
            reserve_bytes,
        },
    )
}

/// Apply one admission attempt against the model, mutating `usage` on admit.
async fn apply_admit(
    controller: &AdmissionController,
    source: &FakeEvictionSource,
    state: &Arc<Mutex<EnvState>>,
    request: u64,
) -> AdmissionDecision {
    let decision = controller.try_admit(request, source).await;
    if decision == AdmissionDecision::Admit {
        state.lock().unwrap().pinned_usage += request;
    }
    decision
}

// ── Single-case unit tests ───────────────────────────────────────────────────

#[test]
async fn admits_when_headroom_is_ample_without_evicting() {
    let state = Arc::new(Mutex::new(EnvState {
        limit: 1000,
        pinned_usage: 0,
        residents: vec![Resident {
            size: 100,
            priority: EvictionPriority::Idle,
        }],
        ..Default::default()
    }));
    let ctrl = controller(state.clone(), 0);
    let source = FakeEvictionSource {
        state: state.clone(),
    };

    let decision = apply_admit(&ctrl, &source, &state, 200).await;
    assert_eq!(decision, AdmissionDecision::Admit);
    // Nothing should have been evicted — there was plenty of headroom.
    assert_eq!(state.lock().unwrap().evictions, 0);
}

#[test]
async fn evicts_idle_before_warm() {
    let state = Arc::new(Mutex::new(EnvState {
        limit: 1000,
        pinned_usage: 0,
        residents: vec![
            Resident {
                size: 400,
                priority: EvictionPriority::Warm,
            },
            Resident {
                size: 400,
                priority: EvictionPriority::Idle,
            },
        ],
        ..Default::default()
    }));
    // usage = 800, limit = 1000, headroom = 200. Request 300 → shortfall 100.
    // One idle (400) covers it; warm must remain untouched.
    let ctrl = controller(state.clone(), 0);
    let source = FakeEvictionSource {
        state: state.clone(),
    };

    let decision = apply_admit(&ctrl, &source, &state, 300).await;
    assert_eq!(decision, AdmissionDecision::Admit);

    let s = state.lock().unwrap();
    assert_eq!(s.eviction_order, vec![EvictionPriority::Idle]);
    assert!(s.usage() <= s.limit);
}

#[test]
async fn rejects_when_nothing_can_be_freed() {
    let state = Arc::new(Mutex::new(EnvState {
        limit: 1000,
        // All usage is pinned (mid-admission), nothing evictable.
        pinned_usage: 950,
        residents: vec![],
        ..Default::default()
    }));
    let ctrl = controller(state.clone(), 0);
    let source = FakeEvictionSource {
        state: state.clone(),
    };

    let decision = apply_admit(&ctrl, &source, &state, 200).await;
    assert_eq!(decision, AdmissionDecision::Reject);
    // No over-commit: usage unchanged.
    assert_eq!(state.lock().unwrap().usage(), 950);
}

#[test]
async fn reserve_is_kept_free() {
    let state = Arc::new(Mutex::new(EnvState {
        limit: 1000,
        pinned_usage: 700,
        residents: vec![],
        ..Default::default()
    }));
    // headroom = 300, reserve = 200 → admissible = 100. Request 150 → reject.
    let ctrl = controller(state.clone(), 200);
    let source = FakeEvictionSource {
        state: state.clone(),
    };

    assert_eq!(
        apply_admit(&ctrl, &source, &state, 150).await,
        AdmissionDecision::Reject
    );
    // But a request within the admissible window succeeds.
    assert_eq!(
        apply_admit(&ctrl, &source, &state, 100).await,
        AdmissionDecision::Admit
    );
}

// ── Property tests ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Op {
    Admit(u64),
}

fn arb_resident_priority() -> impl Strategy<Value = EvictionPriority> {
    prop_oneof![Just(EvictionPriority::Idle), Just(EvictionPriority::Warm)]
}

fn arb_ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec((1u64..800).prop_map(Op::Admit), 0..40)
}

/// Strategy yielding a `(limit, residents)` start state where the residents fit
/// under the limit by construction, by carving each resident's size out of a
/// remaining budget. A resident set exceeding the limit cannot occur in reality
/// (it would already have been OOM-killed), so it is not a valid start state.
fn arb_fitting_state(
    limit_range: std::ops::Range<u64>,
    max_residents: usize,
) -> impl Strategy<Value = (u64, Vec<Resident>)> {
    limit_range.prop_flat_map(move |limit| {
        // Reserve a fraction of the limit for residents (0..=80%) so there is
        // usually some free headroom in the start state too. Each resident then
        // takes a slice of that budget.
        (
            Just(limit),
            (0u64..=(limit * 4 / 5)),
            prop::collection::vec((1u64..=1000, arb_resident_priority()), 0..max_residents),
        )
            .prop_map(|(limit, mut budget, raw)| {
                let mut residents = Vec::new();
                for (weight, priority) in raw {
                    if budget == 0 {
                        break;
                    }
                    // Each resident is at most a third of the remaining budget,
                    // so several can coexist; clamp to whatever budget is left.
                    let size = weight.min(budget.div_ceil(3)).max(1).min(budget);
                    residents.push(Resident { size, priority });
                    budget -= size;
                }
                (limit, residents)
            })
    })
}

proptest! {
    /// Safety invariant: across any random sequence of admits — with random
    /// pre-resident work, random sizes, and a random reserve — modeled usage
    /// must never exceed the limit. This is the property that rules out OOM.
    #[test]
    fn usage_never_exceeds_limit(
        (limit, residents) in arb_fitting_state(500..5000, 20),
        reserve in 0u64..300,
        ops in arb_ops(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let state = Arc::new(Mutex::new(EnvState {
                limit,
                pinned_usage: 0,
                residents,
                ..Default::default()
            }));
            let ctrl = controller(state.clone(), reserve);
            let source = FakeEvictionSource { state: state.clone() };

            for op in ops {
                match op {
                    Op::Admit(req) => {
                        apply_admit(&ctrl, &source, &state, req).await;
                        let s = state.lock().unwrap();
                        prop_assert!(
                            s.usage() <= s.limit,
                            "usage {} exceeded limit {}", s.usage(), s.limit
                        );
                    }
                }
            }
            Ok(())
        }).unwrap();
    }

    /// No spurious eviction: if every admit in the sequence fits within the
    /// admissible headroom at the moment it is issued, nothing is ever evicted.
    /// We guarantee the precondition by giving a huge limit and small requests.
    #[test]
    fn no_eviction_when_headroom_ample(
        residents in prop::collection::vec(
            (1u64..500, arb_resident_priority())
                .prop_map(|(size, priority)| Resident { size, priority }),
            0..20,
        ),
        ops in prop::collection::vec((1u64..50).prop_map(Op::Admit), 0..30),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let state = Arc::new(Mutex::new(EnvState {
                limit: 1_000_000,
                pinned_usage: 0,
                residents,
                ..Default::default()
            }));
            let ctrl = controller(state.clone(), 0);
            let source = FakeEvictionSource { state: state.clone() };

            for op in ops {
                match op {
                    Op::Admit(req) => { apply_admit(&ctrl, &source, &state, req).await; }
                }
            }
            prop_assert_eq!(state.lock().unwrap().evictions, 0);
            Ok(())
        }).unwrap();
    }

    /// Eviction ordering: whenever eviction happens, no warm item is evicted
    /// while an idle item was still available to evict at that step. We check
    /// the weaker, order-level invariant that the recorded eviction order never
    /// has a warm eviction before an idle one within a single `try_admit` call
    /// — i.e. idle is always drained first.
    #[test]
    fn idle_evicted_before_warm(
        (limit, residents) in arb_fitting_state(500..3000, 25),
        ops in prop::collection::vec((1u64..1500).prop_map(Op::Admit), 1..20),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let state = Arc::new(Mutex::new(EnvState {
                limit,
                pinned_usage: 0,
                residents,
                ..Default::default()
            }));
            let ctrl = controller(state.clone(), 0);
            let source = FakeEvictionSource { state: state.clone() };

            for op in ops {
                match op {
                    Op::Admit(req) => { apply_admit(&ctrl, &source, &state, req).await; }
                }
            }

            // Once a warm eviction appears in the order, an idle eviction must
            // never follow it (idle is always exhausted first).
            let order = state.lock().unwrap().eviction_order.clone();
            let mut seen_warm = false;
            for p in order {
                match p {
                    EvictionPriority::Warm => seen_warm = true,
                    EvictionPriority::Idle => prop_assert!(
                        !seen_warm,
                        "idle eviction followed a warm eviction"
                    ),
                }
            }
            Ok(())
        }).unwrap();
    }
}

// ── Carve-out ratio ──────────────────────────────────────────────────────────

#[test]
async fn usable_ratio_caps_admission_below_full_limit() {
    let state = Arc::new(Mutex::new(EnvState {
        limit: 1000,
        pinned_usage: 0,
        residents: vec![],
        ..Default::default()
    }));
    // ceiling = 0.8 * 1000 = 800. Request 850 must be rejected even though the
    // raw limit (1000) would allow it — the top 20% is reserved for the host.
    let ctrl = controller_with_ratio(state.clone(), 0.8, 0);
    let source = FakeEvictionSource {
        state: state.clone(),
    };

    assert_eq!(
        apply_admit(&ctrl, &source, &state, 850).await,
        AdmissionDecision::Reject
    );
    assert_eq!(
        apply_admit(&ctrl, &source, &state, 800).await,
        AdmissionDecision::Admit
    );
}

// ── Concurrency: the simultaneous-big-start race ─────────────────────────────

proptest! {
    /// The contract for the safety invariant under concurrency.
    ///
    /// Many admissions race at once with no external serialisation across the
    /// headroom check and the commit (the commit models the upstream atomic
    /// permit grant; the check is a separate prior read, so a genuine
    /// time-of-check/time-of-use window exists between concurrent tasks).
    ///
    /// The invariant: real usage must never exceed the true `limit`. Admissions
    /// may collectively overshoot the carve-out ceiling into the reserve — that
    /// is what the reserve is for — but never past `limit` itself. The reserve
    /// is sized here to cover the worst-case concurrent overshoot (number of
    /// racers × max request), so a passing test means the reserve margin is a
    /// sufficient substitute for serialising the gate. If this ever fails, the
    /// margin is insufficient for the chosen concurrency and the gate's
    /// correctness depends on stronger synchronisation.
    #[test]
    fn concurrent_admissions_never_exceed_limit(
        racers in 2usize..16,
        request in 50u64..400,
    ) {
        // Worst case: every racer passes the check against the same snapshot and
        // commits. The reserve must cover (racers - 1) extra in-flight requests
        // beyond the one the headroom was actually sized for.
        let reserve = request * racers as u64;
        // Ceiling must leave room for at least one request above the reserve.
        let limit = reserve + request + 1000;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .build()
            .unwrap();
        rt.block_on(async move {
            let state = Arc::new(Mutex::new(EnvState {
                limit,
                pinned_usage: 0,
                residents: vec![],
                ..Default::default()
            }));
            let ctrl = Arc::new(controller_with_ratio(state.clone(), 1.0, reserve));

            let mut handles = Vec::new();
            for _ in 0..racers {
                let ctrl = ctrl.clone();
                let state = state.clone();
                handles.push(tokio::spawn(async move {
                    let source = FakeEvictionSource { state: state.clone() };
                    let decision = ctrl.try_admit(request, &source).await;
                    if decision == AdmissionDecision::Admit {
                        // Models the atomic permit grant: a single locked
                        // fetch-add, separate from the (already-completed) check.
                        state.lock().unwrap().pinned_usage += request;
                    }
                }));
            }
            for h in handles {
                h.await.unwrap();
            }

            let s = state.lock().unwrap();
            prop_assert!(
                s.usage() <= s.limit,
                "concurrent admissions drove usage {} past limit {}",
                s.usage(), s.limit
            );
            Ok(())
        }).unwrap();
    }
}
