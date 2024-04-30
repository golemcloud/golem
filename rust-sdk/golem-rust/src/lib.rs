// Copyright 2024 Golem Cloud
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

#[allow(unused)]
#[rustfmt::skip]
pub mod bindings;

#[cfg(feature = "uuid")]
mod uuid;

mod transaction;

use bindings::golem::api::host::*;

pub use bindings::golem::api::host::oplog_commit;
pub use bindings::golem::api::host::RetryPolicy;
pub use bindings::golem::api::host::PersistenceLevel;

pub use transaction::*;

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
pub fn generate_idempotency_key() -> Uuid {
    bindings::golem::api::host::generate_idempotency_key()
}

pub struct RetryPolicyGuard {
    original: RetryPolicy,
}

impl Drop for RetryPolicyGuard {
    fn drop(&mut self) {
        set_retry_policy(self.original);
    }
}

/// Temporarily sets the retry policy to the given value.
///
/// When the returned guard is dropped, the original retry policy is restored.
#[must_use]
pub fn use_retry_policy(policy: RetryPolicy) -> RetryPolicyGuard {
    let original = get_retry_policy();
    set_retry_policy(policy);
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
/// In case of a failure, the whole operation will be reexecuted during retry.
#[must_use]
pub fn mark_atomic_operation() -> AtomicOperationGuard {
    let begin = mark_begin_operation();
    AtomicOperationGuard { begin }
}

/// Executes the given function as an atomic operation.
///
/// In case of a failure, the whole operation will be reexecuted during retry.
pub fn atomically<T>(f: impl FnOnce() -> T) -> T {
    let _guard = mark_atomic_operation();
    f()
}
