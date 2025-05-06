#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::it::api::*;
use golem_rust::bindings::golem::api::host::get_self_metadata;
use golem_rust::bindings::wasi;
use golem_rust::bindings::wasi::io::streams::StreamError;
use golem_rust::wasm_rpc::wasi::io::poll;
use golem_rust::*;
use golem_rust_macro::golem_operation;
use reqwest::{Client, Response};
use std::time::Duration;

struct Component;

impl Guest for Component {
    fn fail_with_custom_max_retries(max_retries: u64) {
        let _guard = use_retry_policy(RetryPolicy {
            max_attempts: max_retries as u32,
            min_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(1),
            multiplier: 1.0,
            max_jitter_factor: None,
        });

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

        atomically(|| {
            remote_side_effect("1"); // repeated 3x
            remote_side_effect("2"); // repeated 3x

            let decision = remote_call(1); // will return false on the 3rd call
            if decision {
                panic!("crash 1");
            }

            remote_side_effect("3"); // only performed once
        });

        println!("Finished atomic region");

        remote_side_effect("4"); // only performed once

        atomically(|| {
            remote_side_effect("5"); // repeated 3x
            let decision = remote_call(2); // will return false on the 3rd call
            if decision {
                panic!("crash 2");
            }
        });

        remote_side_effect("6"); // only performed once
    }

    fn idempotence_flag(enabled: bool) {
        let _guard = use_idempotence_mode(enabled);

        let future_response = send_remote_side_effect("1");

        atomically(|| {
            let decision = remote_call(1); // will return false on the 2nd call
            if decision {
                panic!("crash 1");
            }
        });

        let incoming_response = get_incoming_response(&future_response);
        let body = read_body(&incoming_response);

        println!(
            "Received response from remote side-effect: {} {}",
            incoming_response.status(),
            String::from_utf8(body).unwrap()
        );
    }

    fn persist_nothing() {
        remote_side_effect("1"); // repeated 1x

        with_persistence_level(PersistenceLevel::PersistNothing, || {
            remote_side_effect("2"); // repeated 3x
        });

        remote_side_effect("3"); // only performed once

        atomically(|| {
            let decision = remote_call(1); // will return false on the 3rd call
            if decision {
                panic!("crash 1");
            }
        });

        remote_side_effect("4"); // only performed once
    }

    fn fallible_transaction_test() -> u64 {
        fallible_transaction(|tx| {
            tx.transaction_step(1)?;
            tx.transaction_step(2)?;
            if tx.transaction_step(3)? {
                tx.fail("fail after 3".to_string())?
            }
            tx.transaction_step(4)?;
            Ok(11)
        })
        .expect("Transaction failed")
    }

    fn infallible_transaction_test() -> u64 {
        infallible_transaction(|tx| {
            let _ = tx.transaction_step(1);
            let _ = tx.transaction_step(2);
            if tx.transaction_step(3).unwrap() {
                panic!("crash after 3");
            }
            let _ = tx.transaction_step(4);
            11
        })
    }

    fn fork_test(input: String) -> String {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let self_name = get_self_metadata().worker_id.worker_name;
        let client = Client::builder().build().unwrap();

        let url = format!("http://localhost:{port}/fork-test/step1/{self_name}/{input}");
        println!("Sending GET {url}");

        let response: Response = client.get(&url).send().expect("Request failed");
        let part1_raw = response.text().expect("Invalid response");
        println!("Received {part1_raw}");

        let part1: String = serde_json::from_str(&part1_raw).unwrap();

        let url = match fork("forked-worker") {
            ForkResult::Original => {
                format!("http://localhost:{port}/fork-test/step2/{self_name}/original/{input}")
            }
            ForkResult::Forked => {
                format!("http://localhost:{port}/fork-test/step2/{self_name}/forked/{input}")
            }
        };
        let response2: Response = client.get(&url).send().expect("Request failed");
        let part2_raw = response2.text().expect("Invalid response");
        println!("Received {part2_raw}");

        let part2: String = serde_json::from_str(&part2_raw).unwrap();

        format!("{part1}::{part2}")
    }
}

#[golem_operation(compensation = compensation_step)]
fn transaction_step(step: u64) -> Result<bool, String> {
    println!("Step {step}");
    Ok(remote_call(step))
}

fn compensation_step(_: bool, step: u64) -> Result<(), String> {
    println!("Compensating step {step}");
    remote_call_undo(step);
    Ok(())
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

fn remote_call_undo(param: u64) -> bool {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let client = Client::builder().build().unwrap();

    let url = format!("http://localhost:{port}/step/{param}");

    println!("Sending DEL {url}");

    let response: Response = client.delete(&url).send().expect("Request failed");

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
            let _ = poll::poll(&[&pollable]);
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
