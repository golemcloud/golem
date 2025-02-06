mod bindings;

use crate::bindings::exports::golem::it::scheduled_invocation_api::Guest;
use crate::bindings::wasi::clocks::wall_clock::Datetime as WasiDatetime;
use std::cell::RefCell;
use std::env;
use time::{Duration, OffsetDateTime};

use self::bindings::golem::api::host::{get_self_metadata, Uuid};
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
    fn add(value: u64) {
        println!("adding to state");
        STATE.with_borrow_mut(|state| state.total += value);
    }

    fn get() -> u64 {
        println!("returning state");
        STATE.with_borrow(|state| state.total)
    }

    fn run() -> () {
        println!("scheduling invocation");

        let env_vars = env::var("COMPONENT_ID").and_then(|e1| env::var("WORKER_NAME").map(|e2| (e1, e2)));

        let uri = match env_vars {
            Ok((component_id, worker_name)) => Uri { value: format!("urn:worker:{component_id}/{worker_name}") },
            Err(_) => {
                // fall back to calling ourselves
                let worker_id = get_self_metadata().worker_id;
                let Uuid { high_bits, low_bits } = worker_id.component_id.uuid;
                let component_id = uuid::Uuid::from_u64_pair(high_bits, low_bits);
                Uri { value: format!("urn:worker:{}/{}", component_id, worker_id.worker_name) }
            }
        };

        let wasi_rpc = WasmRpc::new(&uri);

        let now = OffsetDateTime::now_utc();

        let scheduled_time = now.saturating_add(Duration::seconds(SCHEDULE_DELAY_SECONDS));
        let scheduled_wasi_time = WasiDatetime {
            seconds: scheduled_time.unix_timestamp() as u64,
            nanoseconds: scheduled_time.nanosecond()
        };

        let value: WitValue = {
            use self::bindings::golem::rpc::types::*;
            WitValue {
                nodes: vec![WitNode::PrimU64(1)]
            }
        };
        wasi_rpc.schedule_invocation(scheduled_wasi_time, "golem:it/scheduled-invocation-api.{add}()", &vec![value]);
    }
}

bindings::export!(Component with_types_in bindings);
