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

//! Runs in its own test binary, so no other test writes the `component` storage series and the
//! counters grow by exactly the recorded upload.

use golem_registry_service::metrics::storage::{
    STORAGE_BYTES_WRITTEN_TOTAL, STORAGE_OBJECTS_WRITTEN_TOTAL, STORAGE_TYPE_COMPONENT,
    record_component_uploaded,
};
use test_r::test;

test_r::enable!();

#[test]
fn record_component_uploaded_increments_bytes_and_objects() {
    let bytes = STORAGE_BYTES_WRITTEN_TOTAL.with_label_values(&[STORAGE_TYPE_COMPONENT]);
    let objects = STORAGE_OBJECTS_WRITTEN_TOTAL.with_label_values(&[STORAGE_TYPE_COMPONENT]);
    let (bytes_before, objects_before) = (bytes.get(), objects.get());

    record_component_uploaded(1024);

    assert_eq!(bytes.get() - bytes_before, 1024.0);
    assert_eq!(objects.get() - objects_before, 1.0);
}
