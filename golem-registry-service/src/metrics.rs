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

use prometheus::{IntCounterVec, Registry, default_registry};
use std::sync::LazyLock;

use golem_common::metrics::catalog;

pub(crate) static COMPILATION_ENQUEUE_TOTAL: LazyLock<IntCounterVec> =
    LazyLock::new(|| catalog::COMPONENT_COMPILATION_ENQUEUE_TOTAL.register());

pub fn register_all() -> Registry {
    golem_service_base::metrics::record_build_info();

    default_registry().clone()
}

pub mod storage {
    pub use golem_service_base::metrics::storage::*;

    pub const STORAGE_TYPE_COMPONENT: &str = "component";

    pub fn record_component_uploaded(bytes: u64) {
        record_storage_bytes_written(STORAGE_TYPE_COMPONENT, bytes);
        record_storage_objects_written(STORAGE_TYPE_COMPONENT, 1);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use test_r::test;

        test_r::enable!();

        /// Other tests of this crate may upload components at the same time, so the counters
        /// grow by at least the recorded upload.
        #[test]
        fn record_component_uploaded_increments_bytes_and_objects() {
            let bytes_before = STORAGE_BYTES_WRITTEN_TOTAL
                .with_label_values(&[STORAGE_TYPE_COMPONENT])
                .get();
            let objects_before = STORAGE_OBJECTS_WRITTEN_TOTAL
                .with_label_values(&[STORAGE_TYPE_COMPONENT])
                .get();

            record_component_uploaded(1024);

            assert!(
                STORAGE_BYTES_WRITTEN_TOTAL
                    .with_label_values(&[STORAGE_TYPE_COMPONENT])
                    .get()
                    >= bytes_before + 1024.0
            );
            assert!(
                STORAGE_OBJECTS_WRITTEN_TOTAL
                    .with_label_values(&[STORAGE_TYPE_COMPONENT])
                    .get()
                    >= objects_before + 1.0
            );
        }
    }
}
