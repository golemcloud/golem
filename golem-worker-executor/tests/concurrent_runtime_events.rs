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

//! Seam 3 runtime tests for the two concurrent-durability runtime invariants
//! that the delivery-order fixture (see `concurrent_delivery_order.rs`) does not
//! cover: host-call **cancellation** and the **"End appended before set_event"**
//! ordering the durable recorder relies on.
//!
//! Like the delivery-order test these exercise the *actual* forked Wasmtime
//! component-model-async runtime through a bespoke minimal `wit-bindgen`-async
//! guest (`test-components/concurrent-runtime-events`, unrelated to the Golem
//! agent guest), driven by a raw `Linker`. They deliberately do **not** go
//! through `DurableWorkerCtx`/the oplog: they pin down the runtime guarantees
//! the recorder assumes, not the recorder itself.
//!
//! * Cancellation: when the guest drops an in-flight async import, `wit-bindgen`
//!   issues `subtask.cancel`, which the runtime turns into aborting/dropping the
//!   host future. That host-future drop is exactly what later drives the
//!   recorder's `Cancellable` drop -> `Cancelled` oplog entry, so this asserts
//!   the dropped call's future is actually dropped (and the completing one is
//!   not).
//! * End-before-set_event: a host `call`'s body fully runs (where the recorder
//!   would upload the response and then append `End`) before the guest is made
//!   ready to act on the completion event (`set_event`). The guest reports each
//!   completion back via a synchronous `observed` import; the host asserts, per
//!   call, that its response-upload and `End` markers precede the guest's
//!   observation.
//!
//! The committed fixture is built by the
//! `build-concurrent-runtime-events-component` cargo-make task.

use std::sync::{Arc, Mutex};

use test_r::test;
use wasmtime::component::{Accessor, Component, Linker};
use wasmtime::{Config, Engine, Store, StoreContextMut};

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
        "/../test-components/concurrent-runtime-events/concurrent_runtime_events.wasm"
    );
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        panic!(
            "failed to read fixture {path}: {e} \
             (run `cargo make build-concurrent-runtime-events-component`)"
        )
    });
    Component::new(engine, &bytes).expect("failed to load fixture component")
}

// ------------------------------------------------------------------------- //
// Cancellation
// ------------------------------------------------------------------------- //

/// What the host observed while driving `cancel-one`.
#[derive(Default)]
struct CancelShared {
    /// Ids whose `call` future began executing.
    started: Vec<u32>,
    /// Ids whose `call` future ran to normal completion.
    completed: Vec<u32>,
    /// Ids whose `call` future was dropped before completing (cancelled).
    cancelled: Vec<u32>,
}

/// Records a `call`'s id as cancelled iff its host future is dropped before the
/// call completes. A durable recorder would, at this same point, enqueue a
/// `Cancelled` for the call. Crucially this records into a plain
/// `Arc<Mutex<_>>`, never via the wasmtime `Accessor`, mirroring the production
/// constraint that cancellation cleanup cannot touch the store in `Drop`.
struct CancelGuard {
    id: u32,
    completed: bool,
    shared: Arc<Mutex<CancelShared>>,
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.shared.lock().unwrap().cancelled.push(self.id);
        }
    }
}

/// Drives the fixture's `cancel-one` export: two concurrent `call`s are started,
/// only id `0` is ever allowed to complete (id `1` blocks forever), and the
/// guest drops id `1` once id `0` has completed.
async fn run_cancel_one() -> CancelShared {
    let engine = engine();
    let component = fixture_component(&engine);

    let shared = Arc::new(Mutex::new(CancelShared::default()));

    let mut linker = Linker::<()>::new(&engine);
    let mut host = linker.instance("golem:cmtest/host").expect("host instance");

    {
        let shared = shared.clone();
        host.func_wrap_concurrent("call", move |_accessor: &Accessor<()>, (id,): (u32,)| {
            let shared = shared.clone();
            Box::pin(async move {
                let mut guard = {
                    let mut state = shared.lock().unwrap();
                    state.started.push(id);
                    CancelGuard {
                        id,
                        completed: false,
                        shared: shared.clone(),
                    }
                };

                if id == 0 {
                    // Only complete once both calls have started, so id `1` is
                    // guaranteed to be genuinely in flight when it is cancelled.
                    loop {
                        let both_started = {
                            let state = shared.lock().unwrap();
                            state.started.contains(&0) && state.started.contains(&1)
                        };
                        if both_started {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                    guard.completed = true;
                    shared.lock().unwrap().completed.push(id);
                    Ok((id,))
                } else {
                    // Never completes normally: the only way out is the guest
                    // dropping (cancelling) this in-flight call, which drops this
                    // future and runs `CancelGuard::drop`.
                    std::future::pending::<()>().await;
                    unreachable!("a cancelled call must never complete")
                }
            })
        })
        .expect("register golem:cmtest/host#call");
    }

    // `observed` is imported by the world but unused by `cancel-one`; it still
    // has to be linked for instantiation to succeed.
    host.func_wrap(
        "observed",
        |_store: StoreContextMut<()>, (_id, _value): (u32, u32)| Ok(()),
    )
    .expect("register golem:cmtest/host#observed");

    let mut store = Store::new(&engine, ());
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .expect("instantiate fixture");
    let cancel_one = instance
        .get_typed_func::<(), (Vec<u32>,)>(&mut store, "cancel-one")
        .expect("`cancel-one` export");
    let (winner,) = cancel_one
        .call_async(&mut store, ())
        .await
        .expect("call `cancel-one`");

    assert_eq!(winner, vec![0], "id 0 is the only call allowed to complete");

    let state = shared.lock().unwrap();
    CancelShared {
        started: state.started.clone(),
        completed: state.completed.clone(),
        cancelled: state.cancelled.clone(),
    }
}

/// Dropping an in-flight async import must drop the host future (so the recorder
/// can later record a `Cancelled`), while the call that did complete must not be
/// reported as cancelled.
#[test]
async fn dropping_inflight_call_cancels_host_future() {
    let mut shared = run_cancel_one().await;

    shared.started.sort();
    assert_eq!(shared.started, vec![0, 1], "both calls must start");
    assert_eq!(shared.completed, vec![0], "only id 0 completes");
    assert_eq!(
        shared.cancelled,
        vec![1],
        "the dropped in-flight call's host future must be cancelled"
    );
}

// ------------------------------------------------------------------------- //
// End-before-set_event
// ------------------------------------------------------------------------- //

#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    /// A `call`'s host future began executing.
    Start(u32),
    /// The point in a `call`'s host future where the recorder would upload the
    /// response payload, just before appending `End`.
    ResponseUploaded(u32, u32),
    /// The point in a `call`'s host future where the recorder would append the
    /// `End` oplog entry, just before the future returns.
    End(u32),
    /// The guest reported (via the synchronous `observed` import) that it acted
    /// on this `call`'s completion event.
    GuestObserved(u32, u32),
}

/// Host state for `observe-completions`: a deterministic completion `schedule`
/// plus a globally-sequenced event log.
#[derive(Clone)]
struct ObserveShared {
    schedule: Vec<u32>,
    step: usize,
    started: Vec<u32>,
    seq: u64,
    log: Vec<(u64, Event)>,
}

impl ObserveShared {
    fn new(schedule: Vec<u32>) -> Self {
        Self {
            schedule,
            step: 0,
            started: Vec::new(),
            seq: 0,
            log: Vec::new(),
        }
    }

    fn record(&mut self, event: Event) {
        let seq = self.seq;
        self.seq += 1;
        self.log.push((seq, event));
    }

    fn seq_of(&self, event: &Event) -> u64 {
        let mut matches = self.log.iter().filter(|(_, e)| e == event);
        let (seq, _) = matches
            .next()
            .unwrap_or_else(|| panic!("missing event {event:?} in log {:?}", self.log));
        assert!(
            matches.next().is_none(),
            "duplicate event {event:?} in log {:?}",
            self.log
        );
        *seq
    }

    fn count(&self, kind: fn(&Event) -> bool) -> usize {
        self.log.iter().filter(|(_, e)| kind(e)).count()
    }
}

/// Drives the fixture's `observe-completions(count)` export, completing the host
/// calls in `schedule` order and returning both the guest's observed delivery
/// order and the host event log.
async fn run_observe_completions(schedule: Vec<u32>) -> (Vec<u32>, ObserveShared) {
    let count = schedule.len() as u32;
    let engine = engine();
    let component = fixture_component(&engine);

    let shared = Arc::new(Mutex::new(ObserveShared::new(schedule)));

    let mut linker = Linker::<()>::new(&engine);
    let mut host = linker.instance("golem:cmtest/host").expect("host instance");

    {
        let shared = shared.clone();
        host.func_wrap_concurrent("call", move |_accessor: &Accessor<()>, (id,): (u32,)| {
            let shared = shared.clone();
            Box::pin(async move {
                {
                    let mut state = shared.lock().unwrap();
                    state.started.push(id);
                    state.record(Event::Start(id));
                }

                // Complete strictly in `schedule` order, and only once every
                // call has started, so each call first goes through the runtime's
                // waitable/event path (never an immediate-ready completion).
                let value = loop {
                    let ready = {
                        let mut state = shared.lock().unwrap();
                        let all_started = state.started.len() == state.schedule.len();
                        if all_started && state.schedule.get(state.step) == Some(&id) {
                            state.step += 1;
                            let value = id + 1000;
                            // Response upload happens before `End` is appended,
                            // and both happen before this future returns (i.e.
                            // before the runtime can `set_event` for this call).
                            state.record(Event::ResponseUploaded(id, value));
                            state.record(Event::End(id));
                            Some(value)
                        } else {
                            None
                        }
                    };
                    if let Some(value) = ready {
                        break value;
                    }
                    tokio::task::yield_now().await;
                };

                Ok((value,))
            })
        })
        .expect("register golem:cmtest/host#call");
    }

    {
        let shared = shared.clone();
        host.func_wrap(
            "observed",
            move |_store: StoreContextMut<()>, (id, value): (u32, u32)| {
                shared
                    .lock()
                    .unwrap()
                    .record(Event::GuestObserved(id, value));
                Ok(())
            },
        )
        .expect("register golem:cmtest/host#observed");
    }

    let mut store = Store::new(&engine, ());
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .expect("instantiate fixture");
    let observe = instance
        .get_typed_func::<(u32,), (Vec<u32>,)>(&mut store, "observe-completions")
        .expect("`observe-completions` export");
    let (order,) = observe
        .call_async(&mut store, (count,))
        .await
        .expect("call `observe-completions`");

    let shared = shared.lock().unwrap().clone();
    (order, shared)
}

/// For every call the recorder ordering chain must hold at runtime:
/// `ResponseUploaded(id)` < `End(id)` < `GuestObserved(id)`. The last inequality
/// is the "End appended before set_event" invariant: the host future fully
/// returns (where `End` is appended) before the guest is made ready to observe
/// the completion. The guest's delivery order must also equal the completion
/// schedule.
#[test]
async fn end_is_recorded_before_guest_observes_completion() {
    for schedule in [
        vec![0, 1, 2],
        vec![2, 0, 1],
        vec![3, 1, 4, 0, 2],
        vec![5, 2, 4, 0, 3, 1],
    ] {
        let count = schedule.len();
        let (order, shared) = run_observe_completions(schedule.clone()).await;

        assert_eq!(
            order, schedule,
            "guest delivery order must equal the completion schedule"
        );

        for &id in &schedule {
            let value = id + 1000;
            let upload = shared.seq_of(&Event::ResponseUploaded(id, value));
            let end = shared.seq_of(&Event::End(id));
            let observed = shared.seq_of(&Event::GuestObserved(id, value));

            assert!(
                upload < end,
                "response upload must precede End for id {id} (upload={upload}, end={end})"
            );
            assert!(
                end < observed,
                "End must be appended before the guest observes the completion (set_event) \
                 for id {id} (end={end}, observed={observed})"
            );
        }

        assert_eq!(shared.count(|e| matches!(e, Event::Start(_))), count);
        assert_eq!(
            shared.count(|e| matches!(e, Event::ResponseUploaded(_, _))),
            count
        );
        assert_eq!(shared.count(|e| matches!(e, Event::End(_))), count);
        assert_eq!(
            shared.count(|e| matches!(e, Event::GuestObserved(_, _))),
            count
        );
    }
}
