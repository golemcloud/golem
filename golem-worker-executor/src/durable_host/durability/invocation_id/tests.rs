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

use super::*;
use std::future::pending;
use std::sync::Arc;
use test_r::test;
use wasmtime::component::{Accessor, AccessorTask, RuntimeObservation};
use wasmtime::{AsContextMut, Config, Engine, Store};

const ROOT: OplogIndex = OplogIndex::from_u64(5);

struct PendingTask;

impl AccessorTask<()> for PendingTask {
    async fn run(self, _: &Accessor<()>) -> wasmtime::Result<()> {
        pending().await
    }
}

/// Distinct runtime activity identities, taken from a Store that only spawns tasks.
fn activities(count: usize) -> Vec<RuntimeActivityId> {
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    let mut store = Store::new(&Engine::new(&config).unwrap(), ());
    let ids = Arc::new(Mutex::new(Vec::new()));
    let observed = ids.clone();
    store
        .as_context_mut()
        .set_runtime_observer(Arc::new(move |event| {
            if let RuntimeObservation::ActivityStarted { activity, .. } = event {
                observed.lock().unwrap().push(activity);
            }
        }))
        .unwrap();
    for _ in 0..count {
        store.as_context_mut().spawn(PendingTask);
    }
    let ids = ids.lock().unwrap().clone();
    assert_eq!(ids.len(), count);
    ids
}

/// A guest import that makes no durable call.
fn plain_import(
    numbering: &GuestImportNumbering,
    activity: RuntimeActivityId,
    how: ImportNumbering,
) {
    numbering.number_next_import(how);
    numbering.import_started(activity);
    numbering.host_call_returned();
}

/// A guest import that reserves its invocation id at entry.
fn import(numbering: &GuestImportNumbering, activity: RuntimeActivityId, how: ImportNumbering) {
    numbering.number_next_import(how);
    numbering.import_started(activity);
    numbering.reserve_import_identity(Some(activity)).unwrap();
    numbering.host_call_returned();
}

fn id(anchor: OplogIndex, ordinal: u64) -> GuestImportIdentity {
    GuestImportIdentity::Id(DurableInvocationId::new(anchor, ordinal))
}

#[test]
fn custom_body_imports_do_not_shift_root_siblings() {
    let ids = activities(4);
    let custom_start = OplogIndex::from_u64(6);
    let root = ImportNumbering::Under {
        root: ROOT,
        anchor: ROOT,
    };
    let in_custom_body = ImportNumbering::Under {
        root: ROOT,
        anchor: custom_start,
    };

    let live = GuestImportNumbering::default();
    import(&live, ids[0], root);
    import(&live, ids[1], in_custom_body);
    import(&live, ids[2], in_custom_body);
    import(&live, ids[3], root);

    // A completed custom invocation is replayed without running its body.
    let replay = GuestImportNumbering::default();
    import(&replay, ids[0], root);
    import(&replay, ids[3], root);

    for numbering in [&live, &replay] {
        assert_eq!(
            numbering.reserve_import_identity(Some(ids[0])).unwrap(),
            id(ROOT, 0)
        );
        assert_eq!(
            numbering.reserve_import_identity(Some(ids[3])).unwrap(),
            id(ROOT, 1)
        );
    }
    assert_eq!(
        live.reserve_import_identity(Some(ids[2])).unwrap(),
        id(custom_start, 1)
    );
}

#[test]
fn a_new_root_restarts_the_numbering() {
    let ids = activities(2);
    let numbering = GuestImportNumbering::default();
    import(
        &numbering,
        ids[0],
        ImportNumbering::Under {
            root: ROOT,
            anchor: ROOT,
        },
    );
    let next_invocation = OplogIndex::from_u64(9);
    import(
        &numbering,
        ids[1],
        ImportNumbering::Under {
            root: next_invocation,
            anchor: next_invocation,
        },
    );
    assert_eq!(
        numbering.reserve_import_identity(Some(ids[1])).unwrap(),
        id(next_invocation, 0)
    );
}

#[test]
fn store_holding_calls_take_the_identity_of_the_running_import() {
    let ids = activities(2);
    let numbering = GuestImportNumbering::default();
    import(
        &numbering,
        ids[0],
        ImportNumbering::Under {
            root: ROOT,
            anchor: ROOT,
        },
    );

    numbering.number_next_import(ImportNumbering::Unidentified(
        UnidentifiedReason::Snapshotting,
    ));
    numbering.import_started(ids[1]);
    assert_eq!(
        numbering.store_holding_call_identity(ROOT).unwrap(),
        GuestImportIdentity::Unidentified(UnidentifiedReason::Snapshotting)
    );
    numbering.host_call_returned();

    // Host code between guest calls takes the next ordinal.
    assert_eq!(
        numbering.store_holding_call_identity(ROOT).unwrap(),
        id(ROOT, 1)
    );
}

#[test]
fn imports_without_a_durable_call_take_no_ordinal() {
    let ids = activities(5);
    let root = ImportNumbering::Under {
        root: ROOT,
        anchor: ROOT,
    };
    // The live run polls twice between the durable imports; the replay polls once.
    let live = GuestImportNumbering::default();
    import(&live, ids[0], root);
    plain_import(&live, ids[1], root);
    plain_import(&live, ids[2], root);
    import(&live, ids[3], root);
    let replay = GuestImportNumbering::default();
    import(&replay, ids[0], root);
    plain_import(&replay, ids[1], root);
    import(&replay, ids[3], root);
    for numbering in [&live, &replay] {
        assert_eq!(
            numbering.reserve_import_identity(Some(ids[3])).unwrap(),
            id(ROOT, 1)
        );
    }
}

#[test]
fn an_import_cannot_reserve_after_a_later_import_started() {
    let ids = activities(2);
    let root = ImportNumbering::Under {
        root: ROOT,
        anchor: ROOT,
    };
    let numbering = GuestImportNumbering::default();
    plain_import(&numbering, ids[0], root);
    import(&numbering, ids[1], root);
    assert!(numbering.reserve_import_identity(Some(ids[0])).is_err());
    assert_eq!(
        numbering.reserve_import_identity(Some(ids[1])).unwrap(),
        id(ROOT, 0)
    );
}
