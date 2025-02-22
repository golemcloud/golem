mod bindings;

use crate::bindings::exports::golem::it::component_resolve_api::Guest;
use crate::bindings::wasi::clocks::wall_clock::Datetime as WasiDatetime;
use std::cell::RefCell;
use std::env;
use time::{Duration, OffsetDateTime};

use self::bindings::golem::api::host::{resolve_component_id, resolve_worker_id, resolve_worker_id_strict, ComponentId, Uuid, WorkerId};
use self::bindings::golem::rpc::types::{Uri, WasmRpc, WitValue};

struct State {
    total: u64,
}

thread_local! {
    /// This holds the state of our application.
    static STATE: RefCell<State> = RefCell::new(State {
        total: 0,
    });
}

const SCHEDULE_DELAY_SECONDS: i64 = 1;

struct Component;

impl Guest for Component {

    fn run() -> (Option<ComponentId>, Option<WorkerId>, Option<WorkerId>) {
        println!("resolving component");

        let component_id = resolve_component_id("component-resolve-target");
        let worker_id = resolve_worker_id("component-resolve-target", "counter-1");
        let strict_worker_id = resolve_worker_id_strict("component-resolve-target", "counter-2");
        (component_id, worker_id, strict_worker_id)
    }
}

bindings::export!(Component with_types_in bindings);
