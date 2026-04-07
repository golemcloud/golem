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

use golem_common::golem_version;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref VERSION_INFO: IntCounterVec =
        register_int_counter_vec!("version_info", "Version info of the server", &["version"])
            .unwrap();
}

pub fn register_all() -> Registry {
    VERSION_INFO.with_label_values(&[golem_version()]).inc();

    default_registry().clone()
}

pub mod storage {
    pub use golem_service_base::metrics::storage::*;

    pub const STORAGE_TYPE_COMPONENT: &str = "component";

    pub fn record_component_uploaded(account_id: &str, environment_id: &str, bytes: u64) {
        record_storage_bytes_written(STORAGE_TYPE_COMPONENT, account_id, environment_id, bytes);
        record_storage_objects_written(STORAGE_TYPE_COMPONENT, account_id, environment_id, 1);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use test_r::test;

        test_r::enable!();

        #[test]
        fn record_component_uploaded_increments_bytes_and_objects() {
            let acct = "acct-comp-1";
            let env = "env-comp-1";

            let bytes_before = STORAGE_BYTES_WRITTEN_TOTAL
                .with_label_values(&[STORAGE_TYPE_COMPONENT, acct, env])
                .get();
            let objects_before = STORAGE_OBJECTS_WRITTEN_TOTAL
                .with_label_values(&[STORAGE_TYPE_COMPONENT, acct, env])
                .get();

            record_component_uploaded(acct, env, 1024);

            assert_eq!(
                STORAGE_BYTES_WRITTEN_TOTAL
                    .with_label_values(&[STORAGE_TYPE_COMPONENT, acct, env])
                    .get(),
                bytes_before + 1024.0
            );
            assert_eq!(
                STORAGE_OBJECTS_WRITTEN_TOTAL
                    .with_label_values(&[STORAGE_TYPE_COMPONENT, acct, env])
                    .get(),
                objects_before + 1.0
            );
        }

        #[test]
        fn different_accounts_are_independent() {
            let acct_a = "acct-comp-ind-a";
            let acct_b = "acct-comp-ind-b";
            let env = "env-comp-ind-1";

            record_component_uploaded(acct_a, env, 512);

            assert_eq!(
                STORAGE_BYTES_WRITTEN_TOTAL
                    .with_label_values(&[STORAGE_TYPE_COMPONENT, acct_b, env])
                    .get(),
                0.0,
                "acct_b should be unaffected"
            );
        }
    }
}
