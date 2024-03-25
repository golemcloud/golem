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

    fn explicit_commit(replicas: u8) {
        let now = std::time::SystemTime::now();
        println!("Starting commit with {replicas} replicas at {now:?}");
        oplog_commit(replicas);
        println!("Finished commit");
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