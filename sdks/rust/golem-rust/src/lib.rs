// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[cfg(test)]
test_r::enable!();

pub use uuid::Uuid;
pub use wstd;

pub mod bindings {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-rust",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@1.5.0": golem_wasm::golem_core_1_5_x::types,
            "wasi:io/poll@0.2.3": wstd::wasi::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wstd::wasi::clocks::wall_clock,
        }
    });
}

#[cfg(feature = "export_load_snapshot")]
pub mod load_snapshot {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-rust-load-snapshot",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@1.5.0": golem_wasm::golem_core_1_5_x::types,
            "wasi:io/poll@0.2.3": wstd::wasi::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wstd::wasi::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_rust_load_snapshot_impl as export_load_snapshot;
}

#[cfg(feature = "export_save_snapshot")]
pub mod save_snapshot {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-rust-save-snapshot",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@1.5.0": golem_wasm::golem_core_1_5_x::types,
            "wasi:io/poll@0.2.3": wstd::wasi::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wstd::wasi::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_rust_save_snapshot_impl as export_save_snapshot;
}

#[cfg(feature = "export_golem_agentic")]
pub mod golem_agentic {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-agentic",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,

        with: {
            "golem:core/types@1.5.0": golem_wasm::golem_core_1_5_x::types,
            "wasi:io/poll@0.2.3": wstd::wasi::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wstd::wasi::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_agentic_impl as export_golem_agentic;
}

#[cfg(feature = "export_golem_agentic")]
pub use ctor;

#[cfg(feature = "export_golem_agentic")]
pub use async_trait;

#[cfg(feature = "export_golem_agentic")]
pub use serde;

#[cfg(feature = "export_golem_agentic")]
pub use serde_json;

#[cfg(feature = "export_oplog_processor")]
pub mod oplog_processor {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-rust-oplog-processor",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@1.5.0": golem_wasm::golem_core_1_5_x::types,
            "wasi:io/poll@0.2.3": wstd::wasi::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wstd::wasi::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_rust_oplog_processor_impl as export_oplog_processor;
}

#[cfg(feature = "export_golem_agentic")]
pub mod agentic;

#[cfg(feature = "durability")]
pub mod durability;

#[cfg(feature = "json")]
mod json;

#[cfg(feature = "json")]
pub use json::*;

mod transaction;
pub mod value_and_type;

use bindings::golem::api::host::*;

pub use golem_wasm;

pub use bindings::golem::api::host::{
    complete_promise, create_promise, fork, get_promise, oplog_commit,
};
pub use bindings::golem::api::host::{ForkResult, PersistenceLevel, PromiseId};

pub use transaction::*;

#[cfg(feature = "macro")]
pub use golem_rust_macro::*;

/// Awaits a promise blocking the execution of the agent. The agent is going to be
/// suspended until the promise is completed.
///
/// Use `await_promise` for an async version of this function, allowing to interleave
/// awaiting of the promise with other operations.
pub fn blocking_await_promise(promise_id: &PromiseId) -> Vec<u8> {
    let promise = get_promise(promise_id);
    let pollable = promise.subscribe();
    pollable.block();
    promise.get().unwrap()
}

/// Awaits a promise.
///
/// If only promises or timeouts are awaited simultaneously, the agent is going to be
/// suspended until any of them completes.
pub async fn await_promise(promise_id: &PromiseId) -> Vec<u8> {
    let promise = get_promise(promise_id);
    let pollable = promise.subscribe();
    wstd::io::AsyncPollable::new(pollable).wait_for().await;
    promise.get().unwrap()
}

#[derive(Clone, Debug, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub min_delay: std::time::Duration,
    pub max_delay: std::time::Duration,
    pub multiplier: f64,
    pub max_jitter_factor: Option<f64>,
}

impl From<bindings::golem::api::host::RetryPolicy> for RetryPolicy {
    fn from(value: bindings::golem::api::host::RetryPolicy) -> Self {
        Self {
            max_attempts: value.max_attempts,
            min_delay: std::time::Duration::from_nanos(value.min_delay),
            max_delay: std::time::Duration::from_nanos(value.max_delay),
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
        }
    }
}

impl From<RetryPolicy> for bindings::golem::api::host::RetryPolicy {
    fn from(val: RetryPolicy) -> Self {
        bindings::golem::api::host::RetryPolicy {
            max_attempts: val.max_attempts,
            min_delay: val.min_delay.as_nanos() as u64,
            max_delay: val.max_delay.as_nanos() as u64,
            multiplier: val.multiplier,
            max_jitter_factor: val.max_jitter_factor,
        }
    }
}

pub struct PersistenceLevelGuard {
    original_level: PersistenceLevel,
}

impl Drop for PersistenceLevelGuard {
    fn drop(&mut self) {
        set_oplog_persistence_level(self.original_level);
    }
}

/// Temporarily sets the oplog persistence level to the given value.
///
/// When the returned guard is dropped, the original persistence level is restored.
#[must_use]
pub fn use_persistence_level(level: PersistenceLevel) -> PersistenceLevelGuard {
    let original_level = get_oplog_persistence_level();
    set_oplog_persistence_level(level);
    PersistenceLevelGuard { original_level }
}

/// Executes the given function with the oplog persistence level set to the given value.
pub fn with_persistence_level<R>(level: PersistenceLevel, f: impl FnOnce() -> R) -> R {
    let _guard = use_persistence_level(level);
    f()
}

pub struct IdempotenceModeGuard {
    original: bool,
}

impl Drop for IdempotenceModeGuard {
    fn drop(&mut self) {
        set_idempotence_mode(self.original);
    }
}

/// Temporarily sets the idempotence mode to the given value.
///
/// When the returned guard is dropped, the original idempotence mode is restored.
#[must_use]
pub fn use_idempotence_mode(mode: bool) -> IdempotenceModeGuard {
    let original = get_idempotence_mode();
    set_idempotence_mode(mode);
    IdempotenceModeGuard { original }
}

/// Executes the given function with the idempotence mode set to the given value.
pub fn with_idempotence_mode<R>(mode: bool, f: impl FnOnce() -> R) -> R {
    let _guard = use_idempotence_mode(mode);
    f()
}

/// Generates an idempotency key. This operation will never be replayed —
/// i.e. not only is this key generated, but it is persisted and committed, such that the key can be used in third-party systems (e.g. payment processing)
/// to introduce idempotence.
pub fn generate_idempotency_key() -> uuid::Uuid {
    Into::into(bindings::golem::api::host::generate_idempotency_key())
}

pub struct RetryPolicyGuard {
    original: RetryPolicy,
}

impl Drop for RetryPolicyGuard {
    fn drop(&mut self) {
        set_retry_policy(Into::into(self.original.clone()));
    }
}

/// Temporarily sets the retry policy to the given value.
///
/// When the returned guard is dropped, the original retry policy is restored.
#[must_use]
pub fn use_retry_policy(policy: RetryPolicy) -> RetryPolicyGuard {
    let original = Into::into(get_retry_policy());
    set_retry_policy(Into::into(policy));
    RetryPolicyGuard { original }
}

/// Executes the given function with the retry policy set to the given value.
pub fn with_retry_policy<R>(policy: RetryPolicy, f: impl FnOnce() -> R) -> R {
    let _guard = use_retry_policy(policy);
    f()
}

pub struct AtomicOperationGuard {
    begin: OplogIndex,
}

impl Drop for AtomicOperationGuard {
    fn drop(&mut self) {
        mark_end_operation(self.begin);
    }
}

/// Marks a block as an atomic operation
///
/// When the returned guard is dropped, the operation gets committed.
/// In case of a failure, the whole operation will be re-executed during retry.
#[must_use]
pub fn mark_atomic_operation() -> AtomicOperationGuard {
    let begin = mark_begin_operation();
    AtomicOperationGuard { begin }
}

/// Executes the given function as an atomic operation.
///
/// In case of a failure, the whole operation will be re-executed during retry.
pub fn atomically<T>(f: impl FnOnce() -> T) -> T {
    let _guard = mark_atomic_operation();
    f()
}
