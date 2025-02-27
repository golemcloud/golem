mod bindings;

use crate::bindings::golem::api::host::resolve_worker_id;
use self::bindings::exports::it::scheduled_invocation_stubless_client_exports::client_api::Guest;
use self::bindings::golem::api::host::worker_uri;
use self::bindings::it::scheduled_invocation_stubless_server_client::server_client::{ServerApi, WasiClocksDatetime};
use self::bindings::wasi::clocks::wall_clock::now;

const NANOS_PER_SECOND: u32 = 1_000_000_000;

struct Component;


impl Guest for Component {
    fn test1() {
        let worker_id = resolve_worker_id("scheduled_invocation_stubless_server", "worker_1").unwrap();
        let server_api = ServerApi::new(&worker_uri(&worker_id));

        let scheduled_for = increment_datetime(now(), 200_000_000);
        server_api.schedule_inc_global_by(1, scheduled_for);
        ()
    }

    fn test2() {
        let worker_id = resolve_worker_id("scheduled_invocation_stubless_server", "worker_1").unwrap();
        let server_api = ServerApi::new(&worker_uri(&worker_id));

        let scheduled_for = increment_datetime(now(), 200_000_000);
        server_api.schedule_inc_global_by(1, scheduled_for).cancel();
        ()
    }
}

fn increment_datetime(start: WasiClocksDatetime, nanos: u32) -> WasiClocksDatetime {
    let total_nanos = start.nanoseconds + nanos;
    let seconds = start.seconds + (total_nanos / NANOS_PER_SECOND) as u64;
    let nanoseconds = total_nanos % NANOS_PER_SECOND;
    WasiClocksDatetime { seconds, nanoseconds }
}

bindings::export!(Component with_types_in bindings);
