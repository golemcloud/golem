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

use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    pub static ref VERSION_INFO: IntCounterVec =
        register_int_counter_vec!("version_info", "Version info of the server", &["version"])
            .unwrap();
}

pub mod storage {
    use lazy_static::lazy_static;
    use prometheus::*;

    pub const STORAGE_TYPE_COMPILATION_CACHE: &str = "compilation_cache";

    lazy_static! {
        pub static ref STORAGE_BYTES_WRITTEN_TOTAL: CounterVec = register_counter_vec!(
            "golem_storage_bytes_written_total",
            "Total bytes written to storage, by storage type, account and environment",
            &["storage_type", "account_id", "environment_id"]
        )
        .unwrap();
        pub static ref STORAGE_BYTES_DELETED_TOTAL: CounterVec = register_counter_vec!(
            "golem_storage_bytes_deleted_total",
            "Total bytes deleted from storage, by storage type, account and environment",
            &["storage_type", "account_id", "environment_id"]
        )
        .unwrap();
        pub static ref STORAGE_OBJECTS_WRITTEN_TOTAL: CounterVec = register_counter_vec!(
            "golem_storage_objects_written_total",
            "Total objects written to storage, by storage type, account and environment",
            &["storage_type", "account_id", "environment_id"]
        )
        .unwrap();
        pub static ref STORAGE_OBJECTS_DELETED_TOTAL: CounterVec = register_counter_vec!(
            "golem_storage_objects_deleted_total",
            "Total objects deleted from storage, by storage type, account and environment",
            &["storage_type", "account_id", "environment_id"]
        )
        .unwrap();
    }

    pub fn record_storage_bytes_written(
        storage_type: &str,
        account_id: &str,
        environment_id: &str,
        bytes: u64,
    ) {
        STORAGE_BYTES_WRITTEN_TOTAL
            .with_label_values(&[storage_type, account_id, environment_id])
            .inc_by(bytes as f64);
    }

    pub fn record_storage_objects_written(
        storage_type: &str,
        account_id: &str,
        environment_id: &str,
        count: u64,
    ) {
        STORAGE_OBJECTS_WRITTEN_TOTAL
            .with_label_values(&[storage_type, account_id, environment_id])
            .inc_by(count as f64);
    }

    pub fn record_storage_bytes_deleted(
        storage_type: &str,
        account_id: &str,
        environment_id: &str,
        bytes: u64,
    ) {
        STORAGE_BYTES_DELETED_TOTAL
            .with_label_values(&[storage_type, account_id, environment_id])
            .inc_by(bytes as f64);
    }

    pub fn record_storage_objects_deleted(
        storage_type: &str,
        account_id: &str,
        environment_id: &str,
        count: u64,
    ) {
        STORAGE_OBJECTS_DELETED_TOTAL
            .with_label_values(&[storage_type, account_id, environment_id])
            .inc_by(count as f64);
    }
}
