#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::it::api::Guest;

use async_iterator::Iterator;
use futures_concurrency::future::Join;
use reqwest::*;
use serde::{Deserialize, Serialize};
use wasi::clocks::monotonic_clock::subscribe_duration;
use wasi_async_runtime::{block_on, Reactor};

struct Component;

struct State {
    global: u64,
}

static mut STATE: State = State { global: 0 };

#[allow(static_mut_refs)]
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    unsafe { f(&mut STATE) }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExampleRequest {
    name: String,
    amount: u32,
    comments: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExampleResponse {
    percentage: f64,
    message: Option<String>,
}

async fn async_run(reactor: Reactor) -> String {
    println!("Hello reqwest-wasi!");

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let client = Client::builder(reactor).build().unwrap();
    println!("{:?}", client);

    let request_body = ExampleRequest {
        name: "Something".to_string(),
        amount: 42,
        comments: vec!["Hello".to_string(), "World".to_string()],
    };

    println!("Sending {:?}", request_body);

    let response: Response = client
        .post(&format!("http://localhost:{port}/post-example"))
        .json(&request_body)
        .header("X-Test", "Golem")
        .basic_auth("some", Some("body"))
        .send()
        .await
        .expect("Request failed");

    let status = response.status();
    let body = response
        .json::<ExampleResponse>()
        .await
        .expect("Invalid response");

    println!("Received {:?} {:?}", status, body);

    format!("{:?} {:?}", status, body).to_string()
}

async fn async_run_parallel(reactor: Reactor, n: u16) -> Vec<String> {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let client = Client::builder(reactor).build().unwrap();

    let mut send_futures = Vec::new();
    for i in 0..n {
        let request_body = ExampleRequest {
            name: "Something".to_string(),
            amount: i as u32,
            comments: vec!["Hello".to_string(), "World".to_string()],
        };

        let client_clone = client.clone();
        let port_clone = port.clone();
        let future = async move {
            let response = client_clone
                .post(&format!("http://localhost:{port_clone}/post-example"))
                .json(&request_body)
                .header("X-Test", i.to_string())
                .basic_auth("some", Some("body"))
                .send()
                .await
                .expect("Request failed");

            let status = response.status();
            let body = response
                .json::<ExampleResponse>()
                .await
                .expect("Invalid response");

            println!("Received {:?} {:?}", status, body);
            format!("{:?} {:?}", status, body).to_string()
        };

        send_futures.push(future);
    }

    send_futures.join().await
}

async fn async_slow_body_stream(reactor: Reactor) -> u64 {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let client = Client::builder(reactor).build().unwrap();

    let response: Response = client
        .get(&format!("http://localhost:{port}/big-byte-array"))
        .send()
        .await
        .expect("Request failed");

    let mut total = 0;
    let mut reader = response.async_chunks();
    while let Some(chunk) = reader.next().await {
        match chunk {
            Ok(bytes) => total += bytes.len() as u64,
            Err(err) => {
                println!("Failed to read chunk: {:?}", err);
                break;
            }
        }
    }

    total
}

async fn async_start_polling(reactor: Reactor, until: String) {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let component_id = std::env::var("GOLEM_COMPONENT_ID").unwrap_or("unknown".to_string());
    let worker_name = std::env::var("GOLEM_WORKER_NAME").unwrap_or("unknown".to_string());

    println!("Polling until receiving {until}");

    let client = Client::builder(reactor.clone()).build().unwrap();
    loop {
        println!("Calling the poll endpoint");
        match client.get(format!("http://localhost:{port}/poll?component_id={component_id}&worker_name={worker_name}")).send().await {
            Ok(response) => {
                let s = response.text().await.unwrap_or_else(|err| format!("error receiving body: {err:?}"));
                println!("Received {s}");
                if s == until {
                    println!("Poll loop finished");
                    return;
                } else {
                    let pollable = subscribe_duration(std::time::Duration::from_millis(100).as_nanos() as u64);
                    reactor.wait_for(pollable).await;
                }
            }
            Err(err) => {
                println!("Failed to poll: {err}");
            }
        }
    }
}

impl Guest for Component {
    fn start_polling(until: String) {
        block_on(|reactor| async { async_start_polling(reactor, until).await })
    }

    fn increment() {
        with_state(|s| s.global += 1);
    }

    fn get_count() -> u64 {
        with_state(|s| s.global)
    }

    fn slow_body_stream() -> u64 {
        block_on(|reactor| async { async_slow_body_stream(reactor).await })
    }

    fn run() -> String {
        block_on(|reactor| async { async_run(reactor).await })
    }

    fn run_parallel(n: u16) -> Vec<String> {
        block_on(|reactor| async { async_run_parallel(reactor, n).await })
    }
}

bindings::export!(Component with_types_in bindings);
