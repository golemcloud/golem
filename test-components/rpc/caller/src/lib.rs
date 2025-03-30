#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::rpc::caller_exports::caller_inline_functions::{Guest, TimelineNode};
use crate::bindings::rpc::counters_client::counters_client::{Api, Counter};
use crate::bindings::rpc::ephemeral_client::ephemeral_client::Api as EphemeralApi;
use golem_rust::wasm_rpc::{ComponentId, WorkerId};
use std::env;
use uuid::Uuid;

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
        let component_id =
            env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
        let worker_id = WorkerId {
            component_id: component_id.into(),
            worker_name: "counters_test1".to_string(),
        };

        create_use_and_drop_counters(&worker_id);
        println!("All counters dropped, querying result");

        let remote_api = Api::custom(&worker_id);
        remote_api.blocking_get_all_dropped()
    }

    fn test2() -> u64 {
        with_state(|state| match &mut state.counter {
            Some(counter) => {
                counter.blocking_inc_by(1);
                counter.blocking_get_value()
            }
            None => {
                let component_id =
                    env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
                let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
                let worker_id = WorkerId {
                    component_id: component_id.into(),
                    worker_name: "counters_test2".to_string(),
                };

                let counter = Counter::custom(&worker_id, "counter");
                counter.inc_by(1);
                let result = counter.blocking_get_value();
                state.counter = Some(counter);
                result
            }
        })
    }

    fn test3() -> u64 {
        let component_id =
            env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
        let worker_id = WorkerId {
            component_id: component_id.into(),
            worker_name: "counters_test3".to_string(),
        };

        let api = Api::custom(&worker_id);
        api.blocking_inc_global_by(1);
        api.blocking_get_global_value()
    }

    fn test4() -> (Vec<String>, Vec<(String, String)>) {
        let component_id =
            env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
        let worker_id = WorkerId {
            component_id: component_id.into(),
            worker_name: "counters_test4".to_string(),
        };
        let counter = Counter::custom(&worker_id, "counter-test4");
        (counter.blocking_get_args(), counter.blocking_get_env())
    }

    fn test5() -> Vec<u64> {
        println!("Creating, using and dropping counters in parallel");
        let component_id =
            env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
        let component_id: ComponentId = component_id.into();

        let results = create_use_and_drop_counters_non_blocking(&component_id);
        results.to_vec()
    }

    fn bug_wasm_rpc_i32(in_: TimelineNode) -> TimelineNode {
        println!("Reproducer for wasm-rpc issue #32");
        let component_id =
            env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
        let worker_id = WorkerId {
            component_id: component_id.into(),
            worker_name: "bug32".to_string(),
        };
        let api = Api::custom(&worker_id);
        api.blocking_bug_wasm_rpc_i32(in_)
    }

    fn ephemeral_test1() -> Vec<(String, String)> {
        let component_id =
            env::var("EPHEMERAL_COMPONENT_ID").expect("EPHEMERAL_COMPONENT_ID not set");
        let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
        let component_id: ComponentId = component_id.into();
        let api1 = EphemeralApi::custom(component_id);
        let name1: String = api1.blocking_get_worker_name();
        let key1 = api1.blocking_get_idempotency_key();
        let name2 = api1.blocking_get_worker_name();
        let key2 = api1.blocking_get_idempotency_key();

        let api2 = EphemeralApi::custom(component_id);
        let name3: String = api2.blocking_get_worker_name();
        let key3: String = api2.blocking_get_idempotency_key();

        vec![(name1, key1), (name2, key2), (name3, key3)]
    }

    fn bug_golem1265(s: String) -> Result<(), String> {
        println!("Reproducer for golem issue #1265");
        let component_id =
            env::var("COUNTERS_COMPONENT_ID").expect("COUNTERS_COMPONENT_ID not set");
        let component_id: Uuid = Uuid::parse_str(&component_id).unwrap();
        let worker_id = WorkerId {
            component_id: component_id.into(),
            worker_name: "bug1265".to_string(),
        };
        let api = Api::custom(&worker_id);
        api.blocking_bug_golem1265(&s)
    }
}

fn create_use_and_drop_counters(worker_id: &WorkerId) {
    let counter1 = Counter::custom(worker_id, "counter1");
    let counter2 = Counter::custom(worker_id, "counter2");
    let counter3 = Counter::custom(worker_id, "counter3");
    counter1.blocking_inc_by(1);
    counter1.blocking_inc_by(1);
    counter1.blocking_inc_by(1);

    counter2.blocking_inc_by(2);
    counter2.blocking_inc_by(1);

    counter3.blocking_inc_by(3);

    let value1 = counter1.blocking_get_value();
    let value2 = counter2.blocking_get_value();
    let value3 = counter3.blocking_get_value();

    println!("Counter1 value: {}", value1);
    println!("Counter2 value: {}", value2);
    println!("Counter3 value: {}", value3);
}

fn create_use_and_drop_counters_non_blocking(component_id: &ComponentId) -> [u64; 3] {
    let counter1 = Counter::custom(
        &WorkerId {
            component_id: component_id.clone(),
            worker_name: "counters_test51".to_string(),
        },
        "counter",
    );
    let counter2 = Counter::custom(
        &WorkerId {
            component_id: component_id.clone(),
            worker_name: "counters_test52".to_string(),
        },
        "counter2",
    );
    let counter3 = Counter::custom(
        &WorkerId {
            component_id: component_id.clone(),
            worker_name: "counters_test53".to_string(),
        },
        "counter3",
    );
    counter1.inc_by(1);
    counter1.inc_by(1);
    counter1.inc_by(1);

    counter2.inc_by(2);
    counter2.inc_by(1);

    counter3.inc_by(3);

    let future_value1 = counter1.get_value();
    let future_value2 = counter2.get_value();
    let future_value3 = counter3.get_value();

    let futures = &[&future_value1, &future_value2, &future_value3];

    let poll_value1 = future_value1.subscribe();
    let poll_value2 = future_value2.subscribe();
    let poll_value3 = future_value3.subscribe();

    let mut values = [0u64; 3];
    let mut remaining = vec![&poll_value1, &poll_value2, &poll_value3];
    let mut mapping = vec![0, 1, 2];

    while !remaining.is_empty() {
        let poll_result = golem_rust::wasm_rpc::wasi::io::poll::poll(&remaining);
        println!("Got poll result: {:?}", poll_result);
        for idx in &poll_result {
            let counter_idx = mapping[*idx as usize];
            println!("Got result for counter {}", counter_idx + 1);
            let future = futures[counter_idx];
            let value = future
                .get()
                .expect("future did not return a value because after marked as completed");
            values[counter_idx] = value;
        }

        remaining = remaining
            .into_iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                if poll_result.contains(&(idx as u32)) {
                    None
                } else {
                    Some(item)
                }
            })
            .collect();
        mapping = mapping
            .into_iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                if poll_result.contains(&(idx as u32)) {
                    None
                } else {
                    Some(item)
                }
            })
            .collect();

        println!("mapping at the end of the loop: {:?}", mapping);
    }

    println!("Counter1 value: {}", values[0]);
    println!("Counter2 value: {}", values[1]);
    println!("Counter3 value: {}", values[2]);

    values
}

bindings::export!(Component with_types_in bindings);
