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

//! Runs in its own test binary with a single test, so no other test writes these series and the
//! counters grow by exactly what the test records.

use golem_service_base::metrics::storage::*;
use test_r::test;

test_r::enable!();

#[test]
fn the_storage_recorders_add_exactly_the_recorded_amounts() {
    const STORAGE_TYPE: &str = "storage_metrics_test";
    let bytes = STORAGE_BYTES_WRITTEN_TOTAL.with_label_values(&[STORAGE_TYPE]);
    let written = STORAGE_OBJECTS_WRITTEN_TOTAL.with_label_values(&[STORAGE_TYPE]);
    let deleted = STORAGE_OBJECTS_DELETED_TOTAL.with_label_values(&[STORAGE_TYPE]);
    let cache_bytes = &*COMPILATION_CACHE_BYTES_WRITTEN_TOTAL;
    let cache_objects = &*COMPILATION_CACHE_OBJECTS_WRITTEN_TOTAL;
    let before = [
        bytes.get(),
        written.get(),
        deleted.get(),
        cache_bytes.get(),
        cache_objects.get(),
    ];

    record_storage_bytes_written(STORAGE_TYPE, 300);
    record_storage_objects_written(STORAGE_TYPE, 2);
    record_storage_objects_deleted(STORAGE_TYPE, 3);
    record_compilation_cache_bytes_written(70);
    record_compilation_cache_objects_written(5);

    let after = [
        bytes.get(),
        written.get(),
        deleted.get(),
        cache_bytes.get(),
        cache_objects.get(),
    ];
    let deltas: Vec<f64> = after.iter().zip(before).map(|(a, b)| a - b).collect();
    assert_eq!(deltas, [300.0, 2.0, 3.0, 70.0, 5.0]);
}
