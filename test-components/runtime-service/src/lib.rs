mod bindings;

use crate::bindings::exports::golem::it::api::Guest;
use crate::bindings::golem::api::host::*;
use crate::bindings::wasi;
use crate::bindings::wasi::io::streams::StreamError;
use reqwest::{Client, Response};

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
            multiplier: 1.0,
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

    fn idempotence_flag(enabled: bool) {
        let original = get_idempotence_mode();
        if original != enabled {
            set_idempotence_mode(enabled);
            println!("Changed idempotence mode from {original} to {enabled}");
        }

        let future_response = send_remote_side_effect("1");

        let begin = mark_begin_operation();
        let decision = remote_call(1); // will return false on the 2nd call
        if decision {
            panic!("crash 1");
        }
        mark_end_operation(begin);

        let incoming_response = get_incoming_response(&future_response);
        let body = read_body(&incoming_response);

        println!(
            "Received response from remote side-effect: {} {}",
            incoming_response.status(),
            String::from_utf8(body).unwrap()
        );
    }

    fn persist_nothing() {
        println!("Initial level: {:?}", get_oplog_persistence_level());

        remote_side_effect("1"); // repeated 1x

        set_oplog_persistence_level(PersistenceLevel::PersistNothing);
        remote_side_effect("2"); // repeated 3x
        println!("Changed level: {:?}", get_oplog_persistence_level());
        set_oplog_persistence_level(PersistenceLevel::Smart);

        remote_side_effect("3"); // only performed once

        let begin = mark_begin_operation();
        let decision = remote_call(1); // will return false on the 3rd call
        if decision {
            panic!("crash 1");
        }
        mark_end_operation(begin);

        remote_side_effect("4"); // only performed once
    }

    fn get_workers(
        component_id: ComponentId,
        filter: Option<WorkerAnyFilter>,
        precise: bool,
    ) -> Vec<WorkerMetadata> {
        println!(
            "Get workers component id: {component_id:?}, filter: {filter:?}, precise: {precise}"
        );
        let mut workers: Vec<WorkerMetadata> = Vec::new();
        let getter = GetWorkers::new(component_id, filter.as_ref(), precise);
        loop {
            match getter.get_next() {
                Some(values) => {
                    workers.extend(values);
                }
                None => break,
            }
        }
        workers
    }

    fn get_self_metadata() -> WorkerMetadata {
        println!("Get self metadata");
        bindings::golem::api::host::get_self_metadata()
    }

    fn get_worker_metadata(worker_id: WorkerId) -> Option<WorkerMetadata> {
        println!("Get worker: {worker_id:?} metadata");
        bindings::golem::api::host::get_worker_metadata(&worker_id)
    }

    fn update_worker(worker_id: WorkerId, component_version: ComponentVersion, update_mode: UpdateMode) {
        println!(
            "Update worker worker id: {worker_id:?}, component version: {component_version:?}, update mode: {update_mode:?}"
        );
        bindings::golem::api::host::update_worker(&worker_id, component_version, update_mode);
    }
}

fn remote_call(param: u64) -> bool {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let client = Client::builder().build().unwrap();

    let url = format!("http://localhost:{port}/step/{param}");

    println!("Sending GET {url}");

    let response: Response = client.get(&url).send().expect("Request failed");

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

    let response: Response = client
        .post(&url)
        .body(message.to_string())
        .send()
        .expect("Request failed");

    let status = response.status();

    println!("Received {status}");
}

// Using the low-level wasi-http API to initiate a remote call without reading the response
// useful for failing in the middle of an ongoing request to test idempotence modes
fn send_remote_side_effect(message: &str) -> wasi::http::types::FutureIncomingResponse {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::new();
    let request = wasi::http::types::OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Post)
        .unwrap();
    request.set_path_with_query(Some("/side-effect")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream.write(message.as_bytes()).unwrap();
    drop(request_body_stream);
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_first_byte_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_between_bytes_timeout(Some(5000000000)).unwrap(); // 5s

    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    future_incoming_response
}

fn get_incoming_response(
    future_incoming_response: &wasi::http::types::FutureIncomingResponse,
) -> wasi::http::types::IncomingResponse {
    let incoming_response = match future_incoming_response.get() {
        Some(Ok(Ok(incoming_response))) => {
            println!("Got incoming response");
            incoming_response
        }
        Some(Ok(Err(err))) => {
            println!("Returned with error code: {err:?}");
            panic!("Error: {:?}", err)
        }
        Some(Err(err)) => {
            println!("Returned with error: {err:?}");
            panic!("Error: {:?}", err)
        }
        None => {
            println!("No incoming response yet, polling");
            let pollable = future_incoming_response.subscribe();
            let _ = wasi::io::poll::poll(&[&pollable]);
            get_incoming_response(future_incoming_response)
        }
    };
    incoming_response
}

fn read_body(incoming_response: &wasi::http::types::IncomingResponse) -> Vec<u8> {
    let response_body = incoming_response.consume().unwrap();
    let response_body_stream = response_body.stream().unwrap();
    let mut body = Vec::new();

    let mut eof = false;
    while !eof {
        match response_body_stream.read(u64::MAX) {
            Ok(mut body_chunk) => {
                body.append(&mut body_chunk);
            }
            Err(StreamError::Closed) => {
                eof = true;
            }
            Err(err) => panic!("Error: {:?}", err),
        }
    }
    body
}

bindings::export!(Component with_types_in bindings);
