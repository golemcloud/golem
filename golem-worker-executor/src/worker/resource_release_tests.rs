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

use super::ObservedPrimaryStore;
use crate::metrics::resource_release::{Cause, Origin, tests::TestMetrics};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use test_r::test;

#[test]
fn primary_store_receipt_follows_actual_store_destruction() {
    struct StoreData(Arc<AtomicBool>);
    impl Drop for StoreData {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let engine = wasmtime::Engine::default();
    let metrics = TestMetrics::new();
    let scope = metrics.scope();
    let store = ObservedPrimaryStore::new(&engine, StoreData(dropped.clone()), Some(&scope));
    let store = store.map(async_lock::Mutex::new);
    scope.start(Origin::AcceptedStop, Cause::Interrupt);
    scope.seal();
    assert!(!dropped.load(Ordering::Acquire));
    assert_eq!(metrics.pending("accepted_stop", "primary_store_drop"), 1.0);
    metrics.advance(1000);
    store.mark_primary_execution_stopped();
    assert!(!dropped.load(Ordering::Acquire));
    assert_eq!(
        metrics.count("accepted_stop", "primary_execution_stopped", "released"),
        1.0
    );
    drop(store);
    assert!(dropped.load(Ordering::Acquire));
    metrics.assert_finished("accepted_stop", "released");
    assert_eq!(
        metrics.count("accepted_stop", "primary_store_drop", "released"),
        1.0
    );
}
