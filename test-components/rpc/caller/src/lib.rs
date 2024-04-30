mod bindings;

use crate::bindings::golem::rpc::types::Uri;
use crate::bindings::rpc::counters_stub::stub_counters::{Api, Counter};
use bindings::*;
use std::env;

struct Component;

struct State {
    counter: Option<Counter>,
}

static mut STATE: State = State { counter: None };

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let result = unsafe { f(&mut STATE) };

    return result;
}

impl Guest for Component {
    fn test1() -> Vec<(String, u64)> {
        println!("Creating, using and dropping counters");
        let component_id = env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let counters_uri = Uri {
            value: format!("worker://{component_id}/counters_test1"),
        };

        create_use_and_drop_counters(&counters_uri);
        println!("All counters dropped, querying result");

        let remote_api = Api::new(&counters_uri);
        remote_api.get_all_dropped()
    }

    fn test2() -> u64 {
        with_state(|state| match &mut state.counter {
            Some(counter) => {
                counter.inc_by(1);
                counter.get_value()
            }
            None => {
                let component_id =
                    env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
                let counters_uri = Uri {
                    value: format!("worker://{component_id}/counters_test2"),
                };
                let counter = Counter::new(&counters_uri, "counter");
                counter.inc_by(1);
                let result = counter.get_value();
                state.counter = Some(counter);
                result
            }
        })
    }

    fn test3() -> u64 {
        let component_id =
            env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let counters_uri = Uri {
            value: format!("worker://{component_id}/counters_test3"),
        };
        let api = Api::new(&counters_uri);
        api.inc_global_by(1);
        api.get_global_value()
    }
}

fn create_use_and_drop_counters(counters_uri: &Uri) {
    let counter1 = Counter::new(counters_uri, "counter1");
    let counter2 = Counter::new(counters_uri, "counter2");
    let counter3 = Counter::new(counters_uri, "counter3");
    counter1.inc_by(1);
    counter1.inc_by(1);
    counter1.inc_by(1);

    counter2.inc_by(2);
    counter2.inc_by(1);

    counter3.inc_by(3);

    let value1 = counter1.get_value();
    let value2 = counter2.get_value();
    let value3 = counter3.get_value();

    println!("Counter1 value: {}", value1);
    println!("Counter2 value: {}", value2);
    println!("Counter3 value: {}", value3);
}
