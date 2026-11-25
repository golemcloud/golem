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
use golem_common::metrics::catalog;
use prometheus::IntGaugeVec;
use std::sync::LazyLock;

static BUILD_INFO: LazyLock<IntGaugeVec> = LazyLock::new(|| catalog::BUILD_INFO.register());

/// Sets `golem_build_info` for the version of this process.
pub fn record_build_info() {
    BUILD_INFO.with_label_values(&[golem_version()]).set(1);
}

pub mod storage {
    use golem_common::metrics::catalog;
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        pub static ref LOGICAL_OPERATIONS_TOTAL: IntCounterVec =
            catalog::STORAGE_LOGICAL_OPERATIONS_TOTAL.register();
        pub static ref STORAGE_BYTES_WRITTEN_TOTAL: CounterVec =
            catalog::STORAGE_BYTES_WRITTEN_TOTAL.register();
        pub static ref STORAGE_OBJECTS_WRITTEN_TOTAL: CounterVec =
            catalog::STORAGE_OBJECTS_WRITTEN_TOTAL.register();
        pub static ref STORAGE_OBJECTS_DELETED_TOTAL: CounterVec =
            catalog::STORAGE_OBJECTS_DELETED_TOTAL.register();
        pub static ref COMPILATION_CACHE_BYTES_WRITTEN_TOTAL: Counter =
            catalog::COMPILATION_CACHE_BYTES_WRITTEN_TOTAL.register();
        pub static ref COMPILATION_CACHE_OBJECTS_WRITTEN_TOTAL: Counter =
            catalog::COMPILATION_CACHE_OBJECTS_WRITTEN_TOTAL.register();
    }

    pub fn record_logical_operation(
        kind: &'static str,
        operation: &'static str,
        service: &'static str,
        api: &'static str,
        entity: &'static str,
    ) {
        LOGICAL_OPERATIONS_TOTAL
            .with_label_values(&[kind, operation, service, api, entity])
            .inc();
    }

    pub fn record_storage_bytes_written(storage_type: &str, bytes: u64) {
        STORAGE_BYTES_WRITTEN_TOTAL
            .with_label_values(&[storage_type])
            .inc_by(bytes as f64);
    }

    pub fn record_storage_objects_written(storage_type: &str, count: u64) {
        STORAGE_OBJECTS_WRITTEN_TOTAL
            .with_label_values(&[storage_type])
            .inc_by(count as f64);
    }

    pub fn record_storage_objects_deleted(storage_type: &str, count: u64) {
        STORAGE_OBJECTS_DELETED_TOTAL
            .with_label_values(&[storage_type])
            .inc_by(count as f64);
    }

    pub fn record_compilation_cache_bytes_written(bytes: u64) {
        COMPILATION_CACHE_BYTES_WRITTEN_TOTAL.inc_by(bytes as f64);
    }

    pub fn record_compilation_cache_objects_written(count: u64) {
        COMPILATION_CACHE_OBJECTS_WRITTEN_TOTAL.inc_by(count as f64);
    }
}

pub mod grpc {
    //! Metrics for internal service-to-service gRPC calls made via `GrpcClient`.

    use std::time::Duration;

    use golem_common::metrics::catalog;
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        static ref INTERNAL_GRPC_SUCCESS_SECONDS: HistogramVec =
            catalog::INTERNAL_GRPC_SUCCESS_SECONDS.register();
        static ref INTERNAL_GRPC_FAILURE_TOTAL: CounterVec =
            catalog::INTERNAL_GRPC_FAILURE_TOTAL.register();
        static ref INTERNAL_GRPC_RETRY_TOTAL: CounterVec =
            catalog::INTERNAL_GRPC_RETRY_TOTAL.register();
    }

    pub fn record_internal_grpc_success(target: &str, op: &str, duration: Duration) {
        INTERNAL_GRPC_SUCCESS_SECONDS
            .with_label_values(&[target, op])
            .observe(duration.as_secs_f64());
    }

    pub fn record_internal_grpc_failure(target: &str, op: &str, status: &tonic::Status) {
        let status_code = format!("{:?}", status.code());
        INTERNAL_GRPC_FAILURE_TOTAL
            .with_label_values(&[target, op, &status_code])
            .inc();
    }

    pub fn record_internal_grpc_retry(target: &str, op: &str) {
        INTERNAL_GRPC_RETRY_TOTAL
            .with_label_values(&[target, op])
            .inc();
    }
}

pub mod tls {
    //! Metrics for the TLS material the services keep current from files.

    use std::time::SystemTime;

    use golem_common::metrics::catalog;
    use lazy_static::lazy_static;
    use prometheus::*;

    lazy_static! {
        pub static ref CERTIFICATE_NOT_AFTER_SECONDS: GaugeVec =
            catalog::TLS_CERTIFICATE_NOT_AFTER_SECONDS.register();
        pub static ref RELOAD_FAILURES_TOTAL: IntCounterVec =
            catalog::TLS_CERTIFICATE_RELOAD_FAILURES_TOTAL.register();
    }

    pub fn record_certificate_not_after(use_name: &str, not_after: SystemTime) {
        let seconds = match not_after.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => d.as_secs_f64(),
            Err(e) => -e.duration().as_secs_f64(),
        };
        CERTIFICATE_NOT_AFTER_SECONDS
            .with_label_values(&[use_name])
            .set(seconds);
    }

    pub fn record_reload_failure(use_name: &str) {
        RELOAD_FAILURES_TOTAL.with_label_values(&[use_name]).inc();
    }
}
