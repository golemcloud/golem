mod bindings;

use crate::bindings::exports::golem::it::api::Guest;

use reqwest::*;
use serde::{Deserialize, Serialize};

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

impl Guest for Component {
    // sends a http request and constructs a result string from the response
    fn run() -> String {
        println!("Hello reqwest-wasi!");

        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let client = Client::builder().build().unwrap();
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
            .expect("Request failed");

        let status = response.status();
        let body = response
            .json::<ExampleResponse>()
            .expect("Invalid response");

        println!("Received {:?} {:?}", status, body);

        format!("{:?} {:?}", status, body).to_string()
    }

    fn start_polling(until: String) {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let component_id = std::env::var("GOLEM_COMPONENT_ID").unwrap_or("unknown".to_string());
        let worker_name = std::env::var("GOLEM_WORKER_NAME").unwrap_or("unknown".to_string());

        println!("Polling until receiving {until}");

        let client = Client::builder().build().unwrap();
        loop {
            println!("Calling the poll endpoint");
            match client.get(format!("http://localhost:{port}/poll?component_id={component_id}&worker_name={worker_name}")).send() {
                Ok(response) => {
                    let s = response.text().unwrap_or_else(|err| format!("error receiving body: {err:?}"));
                    println!("Received {s}");
                    if s == until {
                        println!("Poll loop finished");
                        return;
                    } else {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                    }
                }
                Err(err) => {
                    println!("Failed to poll: {err}");
                }
            }
        }
    }

    fn increment() {
        with_state(|s| s.global += 1);
    }

    fn get_count() -> u64 {
        with_state(|s| s.global)
    }

    fn slow_body_stream() -> u64 {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let client = Client::builder().build().unwrap();

        let response: Response = client
            .get(&format!("http://localhost:{port}/big-byte-array"))
            .send()
            .expect("Request failed");

        let total = match response.bytes() {
            Ok(bytes) => bytes.len() as u64,
            Err(err) => {
                println!("Failed to read response: {:?}", err);
                0
            }
        };

        total
    }
}

bindings::export!(Component with_types_in bindings);
