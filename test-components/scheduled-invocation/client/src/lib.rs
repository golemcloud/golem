#[allow(static_mut_refs)]
mod bindings;

use std::borrow::BorrowMut;
use once_cell::sync::Lazy;
use crate::bindings::golem::api::host::resolve_worker_id;
use self::bindings::exports::it::scheduled_invocation_client_exports::client_api::Guest;
use self::bindings::golem::api::host::{get_self_metadata, worker_uri};
use self::bindings::it::scheduled_invocation_client_client::client_client::ClientApi;
use self::bindings::it::scheduled_invocation_server_client::server_client::{ServerApi, WasiClocksDatetime};
use self::bindings::wasi::clocks::wall_clock::now;

const NANOS_PER_SECOND: u32 = 1_000_000_000;

struct State {
    global: u64,
}

static mut STATE: Lazy<State> = Lazy::new(|| State { global: 0 } );

struct Component;

impl Guest for Component {
    fn test1(component_name: String, worker_name: String) {
        let worker_id = resolve_worker_id(&component_name, &worker_name).unwrap();
        let server_api = ServerApi::new(&worker_uri(&worker_id));

        let scheduled_for = increment_datetime(now(), 200_000_000);
        server_api.schedule_inc_global_by(1, scheduled_for);
        ()
    }

    fn test2(component_name: String, worker_name: String) {
        let worker_id = resolve_worker_id(&component_name, &worker_name).unwrap();
        let server_api = ServerApi::new(&worker_uri(&worker_id));

        let scheduled_for = increment_datetime(now(), 200_000_000);
        server_api.schedule_inc_global_by(1, scheduled_for).cancel();
        ()
    }

    fn test3() {
        let worker_id = get_self_metadata().worker_id;
        let client_api = ClientApi::new(&worker_uri(&worker_id));

        let scheduled_for = increment_datetime(now(), 200_000_000);
        client_api.schedule_inc_global_by(1, scheduled_for);
        ()
    }

    fn get_global_value() -> u64 {
        with_state(|state| state.global)
    }

    fn inc_global_by(value: u64) {
        with_state(|state| {
            state.global += value;
        });
    }
}

fn increment_datetime(start: WasiClocksDatetime, nanos: u32) -> WasiClocksDatetime {
    let total_nanos = start.nanoseconds + nanos;
    let seconds = start.seconds + (total_nanos / NANOS_PER_SECOND) as u64;
    let nanoseconds = total_nanos % NANOS_PER_SECOND;
    WasiClocksDatetime { seconds, nanoseconds }
}

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = unsafe { STATE.borrow_mut() };
    f(&mut state)
}

bindings::export!(Component with_types_in bindings);
