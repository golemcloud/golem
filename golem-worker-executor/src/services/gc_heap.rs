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

use crate::services::active_agents::MemoryGrant;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Accounts the GC heap that Wasmtime allocates for a store, which holds every object a
/// WebAssembly-GC guest creates. A Kotlin agent keeps essentially all of its memory here, and only
/// its canonical-ABI staging buffer in linear memory.
///
/// Deliberately separate from [`crate::services::linear_memory::LinearMemoryTracker`] rather than a
/// second pool inside it, for two reasons. The limiter reports a GC heap's own byte sizes, not a
/// total across several memories, so the linear tracker's arithmetic does not apply. And a GC heap
/// is monotonic for the store's lifetime — Wasmtime grows it and never shrinks it, and the
/// collector reclaims objects inside the heap instead of returning capacity — so none of the
/// linear tracker's reconciliation, protected totals or startup reservations are needed.
///
/// The heap takes host admission capacity and counts toward the agent's memory limit. It is
/// deliberately **not** metered and writes **no** oplog growth hint; `sdks/kotlin/MEMORY.md`
/// records that choice and what it costs.
#[derive(Default)]
pub struct GcHeapTracker {
    /// The heap's committed size. Monotonic: only a completed growth raises it.
    bytes: AtomicU64,
    /// Capacity reserved for growths that Wasmtime has permitted but not yet completed. Released
    /// if the growth fails, merged into `held` once it succeeds.
    pending: Mutex<Vec<MemoryGrant>>,
    /// Capacity reserved for the committed size, held until the store drops with this tracker.
    held: Mutex<Option<MemoryGrant>>,
}

impl GcHeapTracker {
    /// The committed size of the heap.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Acquire)
    }

    /// Holds [grant] against a growth Wasmtime has permitted but not yet completed.
    pub(crate) fn reserve_pending(&self, grant: MemoryGrant) {
        self.pending.lock().unwrap().push(grant);
    }

    /// Promotes every pending reservation and records [desired] as the committed size. Called once
    /// Wasmtime reports the growth completed.
    pub(crate) fn commit(&self, desired: u64) {
        let pending: Vec<MemoryGrant> = self.pending.lock().unwrap().drain(..).collect();
        if !pending.is_empty() {
            let mut held = self.held.lock().unwrap();
            for grant in pending {
                match held.as_mut() {
                    Some(existing) => existing.merge(grant),
                    None => *held = Some(grant),
                }
            }
        }
        // The heap only grows, so a stale report must not lower the committed size.
        self.bytes.fetch_max(desired, Ordering::AcqRel);
    }

    /// Drops every pending reservation, returning its capacity to the host. Called when a growth
    /// Wasmtime permitted then failed, and safe to call with nothing pending: the limiter contract
    /// allows a failure notice with no preceding growth request.
    pub(crate) fn growth_failed(&self) {
        self.pending.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    use crate::services::active_agents::admission::{
        AdmissionController, AdmissionPolicy, NoEvictionSource,
    };
    use crate::services::active_agents::memory_probe::{MemoryProbe, MemorySnapshot};
    use std::sync::Arc;

    /// The reservations are inert, so `Drop` releases nothing to a controller. What these cover is
    /// this tracker's own bookkeeping: that a completed growth keeps its reservation, that a failed
    /// one lets it go, and that the committed size only ever rises. The `host_capacity` tests below
    /// use real reservations against a real controller, which is what shows nothing is leaked.
    fn grant(bytes: u64) -> MemoryGrant {
        MemoryGrant::inert(bytes)
    }

    /// Reports a fixed limit and no resident usage, so the controller's headroom moves only with
    /// what this tracker reserves.
    #[derive(Debug)]
    struct FixedProbe {
        limit: u64,
    }

    impl MemoryProbe for FixedProbe {
        fn snapshot(&self) -> MemorySnapshot {
            MemorySnapshot {
                limit_bytes: self.limit,
                current_bytes: 0,
            }
        }
    }

    fn controller(limit: u64) -> Arc<AdmissionController> {
        Arc::new(AdmissionController::new(
            Box::new(FixedProbe { limit }),
            AdmissionPolicy { usable_ratio: 1.0 },
        ))
    }

    #[test]
    fn a_new_tracker_holds_nothing() {
        let tracker = GcHeapTracker::default();
        assert_eq!(tracker.bytes(), 0);
    }

    #[test]
    fn a_completed_growth_keeps_its_reservation_and_raises_the_size() {
        let tracker = GcHeapTracker::default();
        tracker.reserve_pending(grant(4096));
        tracker.commit(4096);
        assert_eq!(tracker.bytes(), 4096);
        assert!(
            tracker.pending.lock().unwrap().is_empty(),
            "the reservation must be promoted"
        );
        assert!(
            tracker.held.lock().unwrap().is_some(),
            "and then held for the store's lifetime"
        );
    }

    #[test]
    fn successive_growths_merge_into_one_held_reservation() {
        let tracker = GcHeapTracker::default();
        for size in [4096u64, 8192, 16384] {
            tracker.reserve_pending(grant(size));
            tracker.commit(size);
        }
        assert_eq!(tracker.bytes(), 16384);
        assert!(tracker.held.lock().unwrap().is_some());
        assert!(tracker.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_growth_releases_its_reservation_and_leaves_the_size_alone() {
        let tracker = GcHeapTracker::default();
        tracker.reserve_pending(grant(4096));
        tracker.commit(4096);
        tracker.reserve_pending(grant(1_000_000));
        tracker.growth_failed();
        assert_eq!(
            tracker.bytes(),
            4096,
            "a growth that failed is not part of the heap"
        );
        assert!(
            tracker.pending.lock().unwrap().is_empty(),
            "its reservation must not be retained"
        );
        assert!(
            tracker.held.lock().unwrap().is_some(),
            "the earlier one still is"
        );
    }

    #[test]
    fn a_failure_notice_with_nothing_pending_is_harmless() {
        // The limiter contract allows `memory_grow_failed` without a preceding `memory_growing`.
        let tracker = GcHeapTracker::default();
        tracker.growth_failed();
        tracker.growth_failed();
        assert_eq!(tracker.bytes(), 0);
    }

    #[test]
    fn the_committed_size_never_falls() {
        // The heap only grows, so a report that arrives out of order must not lower the size and
        // hand back limit headroom the heap is still using.
        let tracker = GcHeapTracker::default();
        tracker.reserve_pending(grant(16384));
        tracker.commit(16384);
        tracker.commit(4096);
        assert_eq!(tracker.bytes(), 16384);
    }

    /// What the inert-reservation tests above cannot reach: whether a GC heap really takes host
    /// admission capacity and really gives it back. A leak here would strand part of the node's
    /// budget for as long as the process lives, and nothing else in the suite would notice.
    mod host_capacity {
        use super::*;
        // Explicit, because the glob above and std's prelude both offer a `test` and neither wins.
        use test_r::test;

        const LIMIT: u64 = 256 * 1024 * 1024;
        const HEAP: u64 = 32 * 1024 * 1024;

        #[test]
        async fn a_committed_heap_holds_host_capacity_and_releases_it_on_teardown() {
            let controller = controller(LIMIT);
            let before = controller.headroom_bytes();

            let tracker = GcHeapTracker::default();
            let grant = controller
                .admit(HEAP, &NoEvictionSource)
                .await
                .expect("the host has room for this heap");
            tracker.reserve_pending(grant);
            tracker.commit(HEAP);

            assert_eq!(
                controller.headroom_bytes(),
                before - HEAP,
                "a committed heap must hold its capacity while the agent is alive"
            );

            drop(tracker);
            assert_eq!(
                controller.headroom_bytes(),
                before,
                "and must return all of it when the agent is torn down"
            );
        }

        #[test]
        async fn a_failed_growth_returns_its_capacity_at_once() {
            let controller = controller(LIMIT);
            let before = controller.headroom_bytes();

            let tracker = GcHeapTracker::default();
            let grant = controller
                .admit(HEAP, &NoEvictionSource)
                .await
                .expect("admitted");
            tracker.reserve_pending(grant);
            tracker.growth_failed();

            assert_eq!(
                controller.headroom_bytes(),
                before,
                "capacity reserved for a growth that failed must not stay held"
            );
            assert_eq!(tracker.bytes(), 0);
        }

        #[test]
        async fn successive_growths_release_together_and_leave_nothing_behind() {
            let controller = controller(LIMIT);
            let before = controller.headroom_bytes();

            let tracker = GcHeapTracker::default();
            let mut total = 0;
            for step in 1..=4u64 {
                let delta = step * 1024 * 1024;
                total += delta;
                let grant = controller
                    .admit(delta, &NoEvictionSource)
                    .await
                    .expect("admitted");
                tracker.reserve_pending(grant);
                tracker.commit(total);
            }
            assert_eq!(controller.headroom_bytes(), before - total);
            assert_eq!(tracker.bytes(), total);

            drop(tracker);
            assert_eq!(
                controller.headroom_bytes(),
                before,
                "merging reservations must not lose one"
            );
        }
    }
}
