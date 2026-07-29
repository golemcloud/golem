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

//! Seam 3 runtime test for concurrent durability.
//!
//! Durable replay of concurrently-completing host calls requires that the
//! host->guest event *delivery* order equals the order the host recorded the
//! completions (the oplog `End` order). The Golem Wasmtime fork pins the
//! component-model-async ready queue to FIFO/insertion order for exactly this
//! reason (`WaitableSet.ready` is a `VecDeque` drained front-first); before that
//! change the ready set was a `BTreeSet<Waitable>` ordered by table-slot
//! identity (`TableId` rep), so when several host calls completed before the
//! guest drained them they were delivered by allocation order instead of
//! completion order.
//!
//! Unlike the `replay_state` fuzz tests (Seam 1), which operate on fabricated
//! oplogs, this test exercises the *actual* runtime: a bespoke minimal
//! component-model-async guest (`test-components/concurrent-delivery-order`,
//! unrelated to the Golem agent guest interface) starts several concurrent
//! async host calls and reports the order it observed them complete. The host
//! drives the completions in a chosen order and asserts the guest's observed
//! delivery order equals that completion order.
//!
//! The committed fixture is built by the `build-concurrent-delivery-order-component`
//! cargo-make task.

use test_r::test;
use wasmtime::component::{Accessor, Component, Linker};
use wasmtime::{Config, Engine, Store};

/// Host-side state driving the completion order of the bespoke `call` host
/// function.
struct DeliveryState {
    /// Ids in the order the host releases (completes) their `call`s.
    schedule: Vec<u32>,
    /// Index of the next id to release.
    step: usize,
}

fn engine() -> Engine {
    let mut config = Config::default();
    // Mirror the production component-model-async configuration (see
    // `Golem::create_wasmtime_config`).
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_error_context(true);
    Engine::new(&config).expect("failed to create engine")
}

fn fixture_component(engine: &Engine) -> Component {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-components/concurrent-delivery-order/concurrent_delivery_order.wasm"
    );
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        panic!(
            "failed to read fixture {path}: {e} \
             (run `cargo make build-concurrent-delivery-order-component`)"
        )
    });
    Component::new(engine, &bytes).expect("failed to load fixture component")
}

/// Runs the fixture's `run(count)` export with the bespoke `host.call`
/// completing in `schedule` order, returning the order in which the guest
/// observed the completions (its delivery order).
async fn delivery_order_for(schedule: Vec<u32>) -> Vec<u32> {
    let count = schedule.len() as u32;
    let engine = engine();
    let component = fixture_component(&engine);

    let mut linker = Linker::<DeliveryState>::new(&engine);
    linker
        .instance("golem:cmtest/host")
        .expect("host instance")
        .func_wrap_concurrent(
            "call",
            |accessor: &Accessor<DeliveryState>, (id,): (u32,)| {
                Box::pin(async move {
                    // Complete strictly in `schedule` order: the call whose id
                    // is next in the schedule advances the cursor and returns;
                    // every other call yields until it is its turn. This makes
                    // the host completion order deterministic and independent of
                    // the order the guest initiated the calls, so the assertion
                    // isolates the runtime's delivery order.
                    loop {
                        let released = accessor.with(|mut access| {
                            let state = access.data_mut();
                            if state.schedule.get(state.step) == Some(&id) {
                                state.step += 1;
                                true
                            } else {
                                false
                            }
                        });
                        if released {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    Ok(())
                })
            },
        )
        .expect("register golem:cmtest/host#call");

    let mut store = Store::new(&engine, DeliveryState { schedule, step: 0 });
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .expect("instantiate fixture");
    let run = instance
        .get_typed_func::<(u32,), (Vec<u32>,)>(&mut store, "run")
        .expect("`run` export");
    let (order,) = run
        .call_async(&mut store, (count,))
        .await
        .expect("call `run`");
    order
}

/// When the host completes the concurrent calls in the reverse of the order the
/// guest started them, the guest must observe them in that same reverse
/// (completion) order. With the pre-fork `BTreeSet`-by-rep ready queue these
/// would be delivered biased towards initiation/rep order instead.
#[test]
async fn delivery_order_matches_completion_order_when_reversed() {
    assert_eq!(delivery_order_for(vec![2, 1, 0]).await, vec![2, 1, 0]);
    assert_eq!(
        delivery_order_for(vec![4, 3, 2, 1, 0]).await,
        vec![4, 3, 2, 1, 0]
    );
}

/// For an arbitrary completion order the guest's delivery order must equal it
/// exactly, including the identity order as a baseline.
#[test]
async fn delivery_order_matches_completion_order_for_permutations() {
    for schedule in [
        vec![0, 1, 2],
        vec![1, 2, 0],
        vec![2, 0, 3, 1],
        vec![3, 1, 4, 0, 2],
        vec![5, 2, 4, 0, 3, 1],
    ] {
        assert_eq!(delivery_order_for(schedule.clone()).await, schedule);
    }
}
