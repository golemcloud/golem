mod bindings;

use crate::bindings::exports::golem::it::scheduled_invocation_api::Guest;
use crate::bindings::golem::api::host::schedule_invocation;
use crate::bindings::wasi::clocks::wall_clock::Datetime as WasiDatetime;
use std::cell::RefCell;
use std::env;
use time::{Duration, OffsetDateTime};

use self::bindings::golem::api::host::{get_self_metadata, ComponentId, Uuid, WitValue, WorkerId};

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

        let component_id = match env::var("COMPONENT_ID") {
            Ok(raw_value) => {
                let uuid = uuid::Uuid::parse_str(&raw_value).expect("not a valid uuid");
                let (high_bits, low_bits) = uuid.as_u64_pair();
                let wasi_uuid = Uuid { high_bits, low_bits };
                ComponentId { uuid: wasi_uuid }
            }
            Err(_) => get_self_metadata().worker_id.component_id
        };

        let worker_name = env::var("WORKER_NAME").unwrap_or_else(|_| {
            get_self_metadata().worker_id.worker_name
        });

        let wasi_worker_id = WorkerId {
            component_id,
            worker_name
        };

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
        schedule_invocation(scheduled_wasi_time, &wasi_worker_id, "golem:it/scheduled-invocation-api.{add}()", &vec![value]);
    }
}

bindings::export!(Component with_types_in bindings);
