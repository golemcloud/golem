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

/// An admitted request whose pages have not yet fully faulted into RSS.
///
/// Models the gap between admission and residency: the worker has been admitted
/// for `reserved` bytes but only `resident` of them have actually touched memory
/// so far. Real RSS (what the probe reads) reflects only `resident`; the
/// remaining `reserved - resident` bytes are still in flight and will appear in
/// RSS later. This lag is what lets concurrent admissions on the same RSS
/// snapshot collectively over-commit.
#[derive(Debug, Clone, Copy)]
struct InFlight {
    reserved: u64,
    resident: u64,
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
    /// Admitted requests whose pages are still faulting in. Their `resident`
    /// portion counts toward measured RSS now; their full `reserved` size is
    /// what RSS will reach once they are fully resident.
    in_flight: Vec<InFlight>,
    /// Count of evictions performed, for the no-spurious-eviction property.
    evictions: usize,
    /// The priorities evicted, in order, for the ordering property.
    eviction_order: Vec<EvictionPriority>,
}

impl EnvState {
    /// Measured RSS: the bytes that have actually faulted in. Lags behind what
    /// has been admitted, because in-flight requests are only partially
    /// resident. This is what the probe reports.
    fn usage(&self) -> u64 {
        self.pinned_usage
            + self.residents.iter().map(|r| r.size).sum::<u64>()
            + self.in_flight.iter().map(|f| f.resident).sum::<u64>()
    }

    /// Total bytes that admitted work will eventually occupy once every
    /// in-flight request has fully faulted in. The safety property is stated
    /// against this value: reserved bytes always become resident, so if this
    /// can exceed the limit the environment will OOM once the lag resolves.
    fn eventual_usage(&self) -> u64 {
        self.pinned_usage
            + self.residents.iter().map(|r| r.size).sum::<u64>()
            + self.in_flight.iter().map(|f| f.reserved).sum::<u64>()
    }

    /// Advance residency: each in-flight request faults in up to `step` more of
    /// its reserved bytes, raising measured RSS toward its eventual size.
    /// Fully-resident requests are retired into `pinned_usage`.
    fn tick_residency(&mut self, step: u64) {
        for f in &mut self.in_flight {
            let remaining = f.reserved - f.resident;
            f.resident += remaining.min(step);
        }
        let (done, pending): (Vec<_>, Vec<_>) = self
            .in_flight
            .drain(..)
            .partition(|f| f.resident >= f.reserved);
        self.pinned_usage += done.iter().map(|f| f.reserved).sum::<u64>();
        self.in_flight = pending;
    }

    /// Fault in `step` bytes of granted-but-untouched memory belonging to the
    /// in-flight request at `index`, without faulting in any other request. A
    /// worker may touch the virtual memory it was already granted at any later
    /// time, with no admission call in the loop, so this raises measured RSS for
    /// one worker in isolation.
    fn fault_in_one(&mut self, index: usize, step: u64) {
        if let Some(f) = self.in_flight.get_mut(index) {
            let remaining = f.reserved - f.resident;
            f.resident += remaining.min(step);
        }
    }

    /// Remove the in-flight worker at `index`: it finishes and unloads, freeing
    /// both its resident pages and its remaining grant. Measured RSS drops by its
    /// resident portion. Returns the bytes it was admitted for, so the caller can
    /// release the gate's reservation for it. The surviving workers' reservations
    /// for their own untouched grants must not be credited by this drop.
    fn exit_one(&mut self, index: usize) -> Option<u64> {
        if index < self.in_flight.len() {
            Some(self.in_flight.remove(index).reserved)
        } else {
            None
        }
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
    /// The gate, so eviction can release each evicted resident's grant — in
    /// production, eviction unloads the worker, which releases its grant.
    controller: Arc<AdmissionController>,
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
                self.controller.release(victim.size);
                state.evictions += 1;
                state.eviction_order.push(priority);
            } else {
                i += 1;
            }
        }
        freed
    }
}

fn controller(state: Arc<Mutex<EnvState>>) -> Arc<AdmissionController> {
    controller_with_ratio(state, 1.0)
}

fn controller_with_ratio(
    state: Arc<Mutex<EnvState>>,
    usable_ratio: f64,
) -> Arc<AdmissionController> {
    // Workers already resident when the gate is created had their grants
    // registered at their own admission; seed the gate to match.
    let initial_granted = {
        let s = state.lock().unwrap();
        s.pinned_usage + s.residents.iter().map(|r| r.size).sum::<u64>()
    };
    let controller = AdmissionController::new(
        Box::new(FakeProbe {
            state: state.clone(),
        }),
        AdmissionPolicy { usable_ratio },
    );
    controller.seed_granted(initial_granted);
    Arc::new(controller)
}

fn eviction_source(
    state: Arc<Mutex<EnvState>>,
    controller: Arc<AdmissionController>,
) -> FakeEvictionSource {
    FakeEvictionSource { state, controller }
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

/// Apply one admission attempt where admitted bytes do NOT become resident
/// immediately. On admit the request is recorded as in-flight with zero resident
/// bytes, so measured RSS is unchanged until a later residency tick faults its
/// pages in. This models the real lag between admission and RSS, the window in
/// which concurrent admissions on the same snapshot can collectively
/// over-commit.
async fn apply_staggered_admit(
    controller: &AdmissionController,
    source: &FakeEvictionSource,
    state: &Arc<Mutex<EnvState>>,
    request: u64,
) -> AdmissionDecision {
    let decision = controller.try_admit(request, source).await;
    if decision == AdmissionDecision::Admit {
        state.lock().unwrap().in_flight.push(InFlight {
            reserved: request,
            resident: 0,
        });
    }
    decision
}

/// A probe with a fixed limit that always reports zero current usage, so the
/// gate's admission decision is driven solely by the granted accounting against
/// the ceiling. Used by the concurrency test, where the property under test is
/// that the granted counter cannot be over-committed by racing admissions.
#[derive(Debug)]
struct ZeroUsageProbe {
    limit: u64,
}

impl MemoryProbe for ZeroUsageProbe {
    fn snapshot(&self) -> MemorySnapshot {
        MemorySnapshot {
            limit_bytes: self.limit,
            current_bytes: 0,
        }
    }
}

/// An eviction source with nothing to evict: a rejected request stays rejected.
struct NoEvictionSource;

#[async_trait::async_trait]
impl EvictionSource for NoEvictionSource {
    async fn evict_at_most(&self, _priority: EvictionPriority, _needed_bytes: u64) -> u64 {
        0
    }
}

/// Concurrent admissions must never grant more than the ceiling allows.
///
/// Many admit attempts of equal size race against a controller whose ceiling
/// admits only a known number of them, with no evictable work to fall back on.
/// Exactly `ceiling / request` requests must be admitted and the rest rejected;
/// the total granted must never exceed the ceiling. This can only hold if each
/// admission's "is there room? then reserve" sequence is atomic against the
/// others — if two admits read the same headroom before either reserves, both
/// pass and the granted total overshoots the ceiling.
#[test]
fn concurrent_admissions_never_overcommit_the_ceiling() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .build()
        .unwrap();

    rt.block_on(async {
        const REQUEST: u64 = 10;
        const CAPACITY: u64 = 50; // exactly 5 requests fit
        const ATTEMPTS: usize = 200; // far more than fit, all racing

        let controller = Arc::new(AdmissionController::new(
            Box::new(ZeroUsageProbe { limit: CAPACITY }),
            AdmissionPolicy { usable_ratio: 1.0 },
        ));

        let mut handles = Vec::with_capacity(ATTEMPTS);
        for _ in 0..ATTEMPTS {
            let controller = controller.clone();
            handles.push(tokio::spawn(async move {
                controller.try_admit(REQUEST, &NoEvictionSource).await
            }));
        }

        let mut admitted = 0usize;
        for handle in handles {
            if handle.await.unwrap() == AdmissionDecision::Admit {
                admitted += 1;
            }
        }

        let expected = (CAPACITY / REQUEST) as usize;
        assert_eq!(
            admitted, expected,
            "expected exactly {expected} admissions to fit, got {admitted}"
        );
        // With zero measured usage, headroom is the ceiling minus granted; if it
        // equals the full ceiling again, everything admitted was released, which
        // never happens here. The decisive check: the admitted total fits.
        assert!(
            admitted as u64 * REQUEST <= CAPACITY,
            "granted {} exceeded ceiling {CAPACITY}",
            admitted as u64 * REQUEST
        );
    });
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
    let ctrl = controller(state.clone());
    let source = eviction_source(state.clone(), ctrl.clone());

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
    let ctrl = controller(state.clone());
    let source = eviction_source(state.clone(), ctrl.clone());

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
    let ctrl = controller(state.clone());
    let source = eviction_source(state.clone(), ctrl.clone());

    let decision = apply_admit(&ctrl, &source, &state, 200).await;
    assert_eq!(decision, AdmissionDecision::Reject);
    // No over-commit: usage unchanged.
    assert_eq!(state.lock().unwrap().usage(), 950);
}

// ── Property tests ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Op {
    Admit(u64),
}

/// An operation in a staggered-start schedule. Unlike [`Op`], admitted bytes do
/// not become resident immediately — `Tick` advances residency separately, so
/// the schedule can interleave admissions and page-faulting in any order.
#[derive(Debug, Clone)]
enum StaggeredOp {
    /// Attempt to admit a worker reserving this many bytes.
    Admit(u64),
    /// Fault in up to this many more bytes of every in-flight worker.
    Tick(u64),
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
    /// pre-resident work and random sizes — modeled usage must never exceed the
    /// limit. This is the property that rules out OOM.
    #[test]
    fn usage_never_exceeds_limit(
        (limit, residents) in arb_fitting_state(500..5000, 20),
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
            let ctrl = controller(state.clone());
            let source = eviction_source(state.clone(), ctrl.clone());

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
            let ctrl = controller(state.clone());
            let source = eviction_source(state.clone(), ctrl.clone());

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
            let ctrl = controller(state.clone());
            let source = eviction_source(state.clone(), ctrl.clone());

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

// ── Staggered-start safety ───────────────────────────────────────────────────

/// A schedule of admissions interleaved with residency ticks. Admissions
/// reserve bytes that only become resident when a later `Tick` faults them in,
/// so the schedule exercises the lag between admission and measured RSS in which
/// concurrent admissions can collectively over-commit. Skewed toward `Admit` so
/// bursts of admissions land between ticks (the dangerous case).
fn arb_staggered_schedule() -> impl Strategy<Value = Vec<StaggeredOp>> {
    prop::collection::vec(
        prop_oneof![
            3 => (1u64..800).prop_map(StaggeredOp::Admit),
            1 => (1u64..800).prop_map(StaggeredOp::Tick),
        ],
        0..60,
    )
}

proptest! {
    /// Safety invariant under staggered starts: for any interleaving of
    /// admissions and residency ticks, once every admitted worker has fully
    /// faulted its pages in, resident usage must not exceed the limit.
    ///
    /// Reserved bytes always eventually become resident, so the check is made
    /// against the state after a final full-residency tick: if that can exceed
    /// the limit, the environment OOMs once the admission lag resolves. This is
    /// the general form of the staggered-burst case — admissions that read the
    /// same low RSS snapshot before each other's pages are counted.
    #[test]
    fn staggered_starts_never_exceed_limit_once_resident(
        (limit, residents) in arb_fitting_state(500..5000, 20),
        schedule in arb_staggered_schedule(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let state = Arc::new(Mutex::new(EnvState {
                limit,
                pinned_usage: 0,
                residents,
                ..Default::default()
            }));
            let ctrl = controller(state.clone());
            let source = eviction_source(state.clone(), ctrl.clone());

            for op in schedule {
                match op {
                    StaggeredOp::Admit(req) => {
                        apply_staggered_admit(&ctrl, &source, &state, req).await;
                    }
                    StaggeredOp::Tick(step) => {
                        state.lock().unwrap().tick_residency(step);
                    }
                }
                // Even mid-flight, measured RSS must never exceed the limit.
                let s = state.lock().unwrap();
                prop_assert!(
                    s.usage() <= s.limit,
                    "resident usage {} exceeded limit {} mid-schedule", s.usage(), s.limit
                );
            }

            // Fault in everything still in flight, then check the eventual
            // resident footprint fits.
            state.lock().unwrap().tick_residency(u64::MAX);
            let s = state.lock().unwrap();
            prop_assert!(
                s.eventual_usage() <= s.limit,
                "eventual resident usage {} exceeded limit {} once fully resident",
                s.eventual_usage(), s.limit
            );
            Ok(())
        }).unwrap();
    }
}

// ── Granted virtual memory ───────────────────────────────────────────────────

/// One step of a schedule that stresses granted-but-untouched memory.
#[derive(Debug, Clone)]
enum GrantOp {
    /// Attempt to admit a worker granted this many bytes of linear memory.
    Grant(u64),
    /// Fault in up to this many bytes of the in-flight worker at this index,
    /// in isolation from the others.
    FaultIn(usize, u64),
    /// The in-flight worker at this index finishes and unloads, dropping its
    /// resident pages and its remaining grant.
    Exit(usize),
}

fn arb_grant_schedule() -> impl Strategy<Value = Vec<GrantOp>> {
    prop::collection::vec(
        prop_oneof![
            3 => (1u64..800).prop_map(GrantOp::Grant),
            3 => (0usize..20, 1u64..800).prop_map(|(i, step)| GrantOp::FaultIn(i, step)),
            1 => (0usize..20).prop_map(GrantOp::Exit),
        ],
        0..80,
    )
}

proptest! {
    /// A worker may fault in the virtual memory it was already granted at any
    /// later time, with no admission call in the loop. Once every granted byte
    /// of every admitted worker becomes resident, that resident footprint must
    /// not exceed the limit.
    ///
    /// Granted bytes can always become resident — nothing in the runtime forces
    /// a worker to leave granted pages untouched — so the safety check is made
    /// against the sum of granted sizes after faulting everything in. If that
    /// can exceed the limit, a node of workers touching their already-granted
    /// pages will OOM with no grow and no admission to intercept it.
    #[test]
    fn granted_memory_never_exceeds_limit_once_faulted_in(
        limit in 800u64..6000,
        schedule in arb_grant_schedule(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let state = Arc::new(Mutex::new(EnvState { limit, ..Default::default() }));
            // usable_ratio 1.0 isolates the granted-memory hole from the host
            // carve-out.
            let ctrl = controller(state.clone());
            let source = eviction_source(state.clone(), ctrl.clone());

            for op in schedule {
                match op {
                    GrantOp::Grant(bytes) => {
                        apply_staggered_admit(&ctrl, &source, &state, bytes).await;
                    }
                    GrantOp::FaultIn(index, step) => {
                        state.lock().unwrap().fault_in_one(index, step);
                    }
                    GrantOp::Exit(index) => {
                        let reserved = state.lock().unwrap().exit_one(index);
                        if let Some(reserved) = reserved {
                            ctrl.release(reserved);
                        }
                    }
                }
                let s = state.lock().unwrap();
                prop_assert!(
                    s.usage() <= s.limit,
                    "resident usage {} exceeded limit {} mid-schedule", s.usage(), s.limit
                );
            }

            // Every granted byte may yet fault in. Once it all does, it must fit.
            state.lock().unwrap().tick_residency(u64::MAX);
            let s = state.lock().unwrap();
            prop_assert!(
                s.eventual_usage() <= s.limit,
                "granted memory {} exceeded limit {} once fully faulted in",
                s.eventual_usage(), s.limit
            );
            Ok(())
        }).unwrap();
    }

    /// Liveness: once every admitted worker has unloaded and its pages have left
    /// memory, the gate's admissible headroom must return to the full ceiling.
    ///
    /// Reservations for workers that exit while still holding untouched granted
    /// memory must be released on unload. If they were not, each such exit would
    /// permanently shrink headroom, and a node churning workers would slowly
    /// refuse all admissions despite being empty.
    #[test]
    fn headroom_recovers_after_all_workers_exit(
        limit in 800u64..6000,
        schedule in arb_grant_schedule(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let usable_ratio = 0.8;
            let state = Arc::new(Mutex::new(EnvState { limit, ..Default::default() }));
            let ctrl = controller_with_ratio(state.clone(), usable_ratio);
            let source = eviction_source(state.clone(), ctrl.clone());

            for op in schedule {
                match op {
                    GrantOp::Grant(bytes) => {
                        apply_staggered_admit(&ctrl, &source, &state, bytes).await;
                    }
                    GrantOp::FaultIn(index, step) => {
                        state.lock().unwrap().fault_in_one(index, step);
                    }
                    GrantOp::Exit(index) => {
                        let reserved = state.lock().unwrap().exit_one(index);
                        if let Some(reserved) = reserved {
                            ctrl.release(reserved);
                        }
                    }
                }
            }

            // Unload every worker still resident, releasing each reservation, and
            // clear measured RSS — the environment is now empty.
            loop {
                let reserved = state.lock().unwrap().exit_one(0);
                match reserved {
                    Some(reserved) => ctrl.release(reserved),
                    None => break,
                }
            }
            {
                let mut s = state.lock().unwrap();
                s.pinned_usage = 0;
                s.residents.clear();
            }

            let ceiling = (limit as f64 * usable_ratio) as u64;
            let headroom = ctrl.headroom_bytes();
            prop_assert_eq!(
                headroom, ceiling,
                "headroom {} did not recover to ceiling {} after all workers exited",
                headroom, ceiling
            );
            Ok(())
        }).unwrap();
    }
}

// ── Density ──────────────────────────────────────────────────────────────────

proptest! {
    /// Density invariant: in a settled state (no admission lag outstanding), the
    /// gate packs the environment to within one request of the usable ceiling
    /// before it starts rejecting. It must not stop admitting while substantial
    /// usable room remains.
    ///
    /// The schedule admits a fixed request size, fully faulting each admitted
    /// worker in before the next admit so measured RSS tracks admitted bytes and
    /// the in-flight reservation drains to zero — the steady-state regime where
    /// density matters. At the first rejection, resident usage must be at least
    /// `ceiling - request`: the only room a correct gate may leave free is the
    /// part too small to fit one more request.
    #[test]
    fn admits_to_within_one_request_of_the_ceiling(
        limit in 2000u64..20_000,
        request in 50u64..600,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async move {
            let usable_ratio = 0.8;
            let state = Arc::new(Mutex::new(EnvState {
                limit,
                ..Default::default()
            }));
            let ctrl = controller_with_ratio(state.clone(), usable_ratio);
            let source = eviction_source(state.clone(), ctrl.clone());

            let ceiling = (limit as f64 * usable_ratio) as u64;

            // Admit until the first rejection, faulting each worker fully in
            // before the next so no reservation lag is outstanding.
            let mut rejected = false;
            for _ in 0..((limit / request) + 2) {
                let decision = apply_staggered_admit(&ctrl, &source, &state, request).await;
                state.lock().unwrap().tick_residency(u64::MAX);
                if decision == AdmissionDecision::Reject {
                    rejected = true;
                    break;
                }
            }

            prop_assert!(rejected, "gate never rejected; ceiling {ceiling} too large for the schedule");

            let s = state.lock().unwrap();
            prop_assert!(
                s.usage() + request > ceiling,
                "gate rejected at resident usage {} with ceiling {ceiling}: left more than one request ({request}) of usable room free",
                s.usage()
            );
            // And it must never have over-committed.
            prop_assert!(s.eventual_usage() <= s.limit);
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
    let ctrl = controller_with_ratio(state.clone(), 0.8);
    let source = eviction_source(state.clone(), ctrl.clone());

    assert_eq!(
        apply_admit(&ctrl, &source, &state, 850).await,
        AdmissionDecision::Reject
    );
    assert_eq!(
        apply_admit(&ctrl, &source, &state, 800).await,
        AdmissionDecision::Admit
    );
}

// ── First-worker module charge is gated, not blindly committed ───────────────
//
// The composition test that the first worker's memory and module are gated
// together — so a worker whose memory alone fits but whose memory + module does
// not is refused rather than admitted and then over-committed — lives in the
// `active_workers::tests::component_module_charge` module, where the admission
// controller and the component-charge registry are composed exactly as the
// production start path composes them.

/// Concurrent memory grows must not deadlock against the admission eviction
/// scan.
///
/// A memory grow acquires a permit while the growing worker holds its own
/// instance lock, and the admission slow path scans the worker set, taking each
/// other worker's instance lock to classify it for eviction. With many workers
/// growing at once under memory pressure these two must not form an AB-BA cycle.
/// Workloads that never grow memory never exercise this path.
mod grow_lock_ordering {
    use super::super::{AdmissionController, AdmissionPolicy, EvictionPriority, EvictionSource};
    use crate::services::active_workers::memory_probe::{MemoryProbe, MemorySnapshot};
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::test;
    use tokio::sync::Mutex as AsyncMutex;

    /// Per-worker lock, standing in for `Worker::instance`.
    type WorkerLock = Arc<AsyncMutex<()>>;

    /// Probe pinned to zero admissible headroom so `try_admit` takes the slow
    /// (scanning) path, modelling the moment a grow's requested delta does not
    /// fit the current headroom.
    #[derive(Debug)]
    struct SaturatedProbe;

    impl MemoryProbe for SaturatedProbe {
        fn snapshot(&self) -> MemorySnapshot {
            MemorySnapshot {
                limit_bytes: 1,
                current_bytes: u64::MAX,
            }
        }
    }

    /// Probe reporting ample headroom so `try_admit` takes the fast path and
    /// never scans — the same grow code path, but not under memory pressure.
    #[derive(Debug)]
    struct AmpleHeadroomProbe;

    impl MemoryProbe for AmpleHeadroomProbe {
        fn snapshot(&self) -> MemorySnapshot {
            MemorySnapshot {
                limit_bytes: u64::MAX,
                current_bytes: 0,
            }
        }
    }

    /// Eviction source that, like `evict_at_most_memory`, scans every worker and
    /// takes each worker's instance lock (via `eviction_class`) to classify it.
    /// Frees nothing (all workers active). The lock on each worker is held only
    /// briefly, faithfully — the deadlock comes from the ordering, not hold time.
    struct ScanningEvictionSource {
        workers: Vec<WorkerLock>,
    }

    #[async_trait::async_trait]
    impl EvictionSource for ScanningEvictionSource {
        async fn evict_at_most(&self, _priority: EvictionPriority, _needed_bytes: u64) -> u64 {
            for worker in &self.workers {
                let _guard = worker.lock().await;
            }
            0
        }
    }

    /// Models the grow path's lock interaction: run the admission scan, which
    /// takes other workers' instance locks, without holding this worker's own
    /// instance lock, then take it afterwards to merge the permit (as
    /// `Worker::increase_memory` does).
    async fn grow_then_lock(
        controller: &AdmissionController,
        own: &WorkerLock,
        workers: Vec<WorkerLock>,
    ) {
        let source = ScanningEvictionSource { workers };
        controller.try_admit(1, &source).await;
        let _own_guard = own.lock().await;
    }

    fn workers(n: usize) -> Vec<WorkerLock> {
        (0..n).map(|_| Arc::new(AsyncMutex::new(()))).collect()
    }

    fn controller(probe: Box<dyn MemoryProbe>) -> Arc<AdmissionController> {
        Arc::new(AdmissionController::new(
            probe,
            AdmissionPolicy { usable_ratio: 1.0 },
        ))
    }

    /// Many workers growing concurrently under memory pressure (every grow takes
    /// the scanning slow path) must all complete without deadlocking.
    #[test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_grows_do_not_deadlock_under_pressure() {
        const WORKERS: usize = 32;
        const DEADLINE: Duration = Duration::from_secs(10);

        let workers = workers(WORKERS);
        let controller = controller(Box::new(SaturatedProbe));

        let mut grows = Vec::new();
        for i in 0..WORKERS {
            let controller = controller.clone();
            let all = workers.clone();
            let own = workers[i].clone();
            grows.push(tokio::spawn(async move {
                grow_then_lock(&controller, &own, all).await;
            }));
        }

        let all_done = async {
            for task in grows {
                let _ = task.await;
            }
        };

        let result = tokio::time::timeout(DEADLINE, all_done).await;
        assert!(
            result.is_ok(),
            "concurrent grows deadlocked: the scan must not run while a worker holds its own instance lock"
        );
    }

    /// With comfortable headroom the gate admits on the fast path without
    /// scanning, so no worker's instance lock is taken during admission and
    /// concurrent grows complete. Confirms the deadlock risk is specific to the
    /// scan-under-pressure path.
    #[test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_deadlock_with_ample_headroom() {
        const WORKERS: usize = 32;
        const DEADLINE: Duration = Duration::from_secs(10);

        let workers = workers(WORKERS);
        let controller = controller(Box::new(AmpleHeadroomProbe));

        let mut grows = Vec::new();
        for i in 0..WORKERS {
            let controller = controller.clone();
            let all = workers.clone();
            let own = workers[i].clone();
            grows.push(tokio::spawn(async move {
                grow_then_lock(&controller, &own, all).await;
            }));
        }

        let all_done = async {
            for task in grows {
                let _ = task.await;
            }
        };

        let result = tokio::time::timeout(DEADLINE, all_done).await;
        assert!(
            result.is_ok(),
            "grows with ample headroom should not scan and should not deadlock"
        );
    }
}
