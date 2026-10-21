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

//! mimalloc allocator memory accounting, sampled into Prometheus gauges.
//!
//! `mi_process_info` reports both the resident set (touched pages actually in
//! physical RAM) and the committed set (pages mimalloc has obtained from the OS
//! and not yet returned). On the containerised Linux deployment the cgroup's
//! `memory.current` — which the admission gate reads — charges the committed
//! set, so a large `committed - resident` gap means mimalloc is holding pages
//! it could return, inflating the gate's measured usage. Comparing committed vs
//! resident distinguishes allocator retention/fragmentation (commit >> rss)
//! from genuinely live memory (commit ~= rss).

use lazy_static::lazy_static;
use prometheus::*;

// `libmimalloc-sys` does not re-export `mi_process_info`, but the symbol is
// present in the linked static library, so we declare it ourselves. All
// out-parameters are `size_t` (bytes for the memory fields, milliseconds for
// the timing fields).
unsafe extern "C" {
    fn mi_process_info(
        elapsed_msecs: *mut usize,
        user_msecs: *mut usize,
        system_msecs: *mut usize,
        current_rss: *mut usize,
        peak_rss: *mut usize,
        current_commit: *mut usize,
        peak_commit: *mut usize,
        page_faults: *mut usize,
    );
}

lazy_static! {
    static ref MIMALLOC_CURRENT_RSS_BYTES: GaugeVec = register_gauge_vec!(
        "golem_mimalloc_current_rss_bytes",
        "mimalloc-reported resident set size: pages currently touched and in physical RAM",
        &["executor_id"]
    )
    .unwrap();
    static ref MIMALLOC_PEAK_RSS_BYTES: GaugeVec = register_gauge_vec!(
        "golem_mimalloc_peak_rss_bytes",
        "mimalloc-reported peak resident set size since process start",
        &["executor_id"]
    )
    .unwrap();
    static ref MIMALLOC_CURRENT_COMMIT_BYTES: GaugeVec = register_gauge_vec!(
        "golem_mimalloc_current_commit_bytes",
        "mimalloc-reported committed memory: pages obtained from the OS and not yet returned (what the cgroup charges as memory.current)",
        &["executor_id"]
    )
    .unwrap();
    static ref MIMALLOC_PEAK_COMMIT_BYTES: GaugeVec = register_gauge_vec!(
        "golem_mimalloc_peak_commit_bytes",
        "mimalloc-reported peak committed memory since process start",
        &["executor_id"]
    )
    .unwrap();
}

/// Reads `mi_process_info` and updates the mimalloc memory gauges. Cheap and
/// allocation-free; intended to be called from a periodic sampling loop.
pub fn sample() {
    let mut elapsed = 0usize;
    let mut user = 0usize;
    let mut system = 0usize;
    let mut current_rss = 0usize;
    let mut peak_rss = 0usize;
    let mut current_commit = 0usize;
    let mut peak_commit = 0usize;
    let mut page_faults = 0usize;

    // Safety: all pointers are valid, non-overlapping stack locations of the
    // expected `size_t` type; mi_process_info only writes through them.
    unsafe {
        mi_process_info(
            &mut elapsed,
            &mut user,
            &mut system,
            &mut current_rss,
            &mut peak_rss,
            &mut current_commit,
            &mut peak_commit,
            &mut page_faults,
        );
    }

    let id = crate::identity::executor_id();
    MIMALLOC_CURRENT_RSS_BYTES
        .with_label_values(&[id])
        .set(current_rss as f64);
    MIMALLOC_PEAK_RSS_BYTES
        .with_label_values(&[id])
        .set(peak_rss as f64);
    MIMALLOC_CURRENT_COMMIT_BYTES
        .with_label_values(&[id])
        .set(current_commit as f64);
    MIMALLOC_PEAK_COMMIT_BYTES
        .with_label_values(&[id])
        .set(peak_commit as f64);
}
