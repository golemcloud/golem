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
//! The committed count of `mi_process_info` is the address space that mimalloc
//! made readable and writable for the heap. The kernel charges a page to the
//! cgroup only when the process touches it, so the committed count can be
//! larger than `memory.current`. On Linux, mimalloc gives freed pages back with
//! `MADV_DONTNEED` and keeps them counted as committed, so the count goes down
//! only when mimalloc unmaps memory. It is close to a high-water mark of the
//! heap, not the heap that is in use.
//!
//! On Linux, the current resident set of `mi_process_info` is a copy of the
//! committed count, so there is no gauge for it. The peak resident set comes
//! from `getrusage` and is the peak of the whole process, linear memories
//! included.

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
    static ref MIMALLOC_PEAK_RSS_BYTES: GaugeVec = register_gauge_vec!(
        "golem_mimalloc_peak_rss_bytes",
        "Peak resident set size of the executor process since it started, from getrusage",
        &["executor_id"]
    )
    .unwrap();
    static ref MIMALLOC_CURRENT_COMMIT_BYTES: GaugeVec = register_gauge_vec!(
        "golem_mimalloc_current_commit_bytes",
        "Heap address space that mimalloc committed; on Linux it goes down only when mimalloc unmaps memory, so it is close to a high-water mark and can exceed the resident heap",
        &["executor_id"]
    )
    .unwrap();
    static ref MIMALLOC_PEAK_COMMIT_BYTES: GaugeVec = register_gauge_vec!(
        "golem_mimalloc_peak_commit_bytes",
        "Peak of golem_mimalloc_current_commit_bytes since the executor started",
        &["executor_id"]
    )
    .unwrap();
}

/// Reads `mi_process_info` and updates the mimalloc memory gauges. Cheap and
/// allocation-free; intended to be called from a periodic sampling loop.
pub fn sample() {
    let mut peak_rss = 0usize;
    let mut current_commit = 0usize;
    let mut peak_commit = 0usize;

    // Safety: the non-null pointers are valid, non-overlapping stack locations
    // of the expected `size_t` type; mi_process_info only writes through them,
    // and skips the null ones.
    unsafe {
        mi_process_info(
            std::ptr::null_mut(), // elapsed_msecs
            std::ptr::null_mut(), // user_msecs
            std::ptr::null_mut(), // system_msecs
            std::ptr::null_mut(), // current_rss
            &mut peak_rss,
            &mut current_commit,
            &mut peak_commit,
            std::ptr::null_mut(), // page_faults
        );
    }

    let id = crate::identity::executor_id();
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
