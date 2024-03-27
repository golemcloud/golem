mod bindings;

use reqwest::{Client, Response};
use crate::bindings::exports::golem::it::api::Guest;
use crate::bindings::golem::api::host::*;

struct Component;

impl Guest for Component {
    fn get_self_uri(function_name: String) -> String {
        get_self_uri(&function_name).value
    }

    fn jump() -> u64 {
        let mut state = 0;

        println!("started: {state}"); // 'started 1'
        state += 1;

        let state1 = get_oplog_index();

        state += 1;
        println!("second: {state}"); // 'second 2'

        if remote_call(state) {
            set_oplog_index(state1); // we resume from state 1, so we emit 'second 2' again but not 'started 1'
        }

        state += 1;
        println!("third: {state}"); // 'third 3'

        let state2 = get_oplog_index();

        state += 1;
        println!("fourth: {state}"); // 'fourth 4'

        if remote_call(state) {
            set_oplog_index(state2); // we resume from state 2, so emit 'fourth 4' again but not the rest
        }

        state += 1;
        println!("fifth: {state}"); // 'fifth 5'

        // Expected final output:
        // started 1
        // second 2
        // second 2
        // third 3
        // fourth 4
        // fourth 4
        // fifth 5

        state // final value is 5
    }

    fn fail_with_custom_max_retries(max_retries: u64) {
        let initial_retry_policy = get_retry_policy();
        println!("Initial retry policy: {initial_retry_policy:?}");
        set_retry_policy(RetryPolicy {
            max_attempts: max_retries as u32,
            min_delay: 1000000000, // 1s
            max_delay: 1000000000, // 1s
            multiplier: 1,
        });
        let overridden_retry_policy = get_retry_policy();
        println!("Overridden retry policy: {overridden_retry_policy:?}");

        panic!("Fail now");
    }

    fn explicit_commit(replicas: u8) {
        let now = std::time::SystemTime::now();
        println!("Starting commit with {replicas} replicas at {now:?}");
        oplog_commit(replicas);
        println!("Finished commit");
    }

    fn atomic_region() {
        let now = std::time::SystemTime::now();
        println!("Starting atomic region at {now:?}");

        let begin = mark_begin_operation();

        remote_side_effect("1"); // repeated 3x
        remote_side_effect("2"); // repeated 3x

        let decision = remote_call(1); // will return false on the 3rd call
        if decision {
            panic!("crash 1");
        }

        remote_side_effect("3"); // only performed once

        mark_end_operation(begin);
        println!("Finished atomic region");

        remote_side_effect("4"); // only performed once

        let begin = mark_begin_operation();
        remote_side_effect("5"); // repeated 3x
        let decision = remote_call(2); // will return false on the 3rd call
        if decision {
            panic!("crash 2");
        }
        mark_end_operation(begin);

        remote_side_effect("6"); // only performed once
    }
}

fn remote_call(param: u64) -> bool {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let client = Client::builder().build().unwrap();

    let url = format!("http://localhost:{port}/step/{param}");

    println!("Sending GET {url}");

    let response: Response = client.get(&url)
        .send()
        .expect("Request failed");

    let status = response.status();
    let body = response.json::<bool>().expect("Invalid response");

    println!("Received {status} {body}");
    body
}

fn remote_side_effect(message: &str) {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let client = Client::builder().build().unwrap();

    let url = format!("http://localhost:{port}/side-effect");

    println!("Sending POST {url}");

    let response: Response = client.post(&url)
        .body(message.to_string())
        .send()
        .expect("Request failed");

    let status = response.status();

    println!("Received {status}");
}