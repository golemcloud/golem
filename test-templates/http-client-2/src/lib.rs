cargo_component_bindings::generate!();

use crate::bindings::exports::golem::it::api::Guest;

use reqwest::*;
use serde::{Deserialize, Serialize};

struct Component;

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

        let client = Client::builder().build().unwrap();
        println!("{:?}", client);

        let request_body = ExampleRequest {
            name: "Something".to_string(),
            amount: 42,
            comments: vec!["Hello".to_string(), "World".to_string()],
        };

        println!("Sending {:?}", request_body);

        let response: Response = client.post("http://localhost:9999/post-example")
            .json(&request_body)
            .header("X-Test", "Golem")
            .basic_auth("some", Some("body"))
            .send()
            .expect("Request failed");

        let status = response.status();
        let body = response.json::<ExampleResponse>().expect("Invalid response");

        println!("Received {:?} {:?}", status, body);

        format!("{:?} {:?}", status, body).to_string()
    }

    fn start_polling(until: String) {
        println!("Polling until receiving {until}");

        let client = Client::builder().build().unwrap();
        loop {
            println!("Calling the poll endpoint");
            match client.get("http://localhost:9999/poll").send() {
                Ok(response) => {
                    let s = response.text().unwrap();
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
}

