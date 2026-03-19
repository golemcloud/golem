use golem_rust::{agent_definition, agent_implementation};
use golem_wasi_http::{Client, Response};
use serde::{Deserialize, Serialize};

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

#[agent_definition]
pub trait GolemWasiHttp {
    fn new(name: String) -> Self;
    fn run(&self) -> String;
    fn start_polling(&self, until: String);
    fn increment(&mut self);
    fn get_count(&self) -> u64;
    fn slow_body_stream(&self) -> u64;
}

pub struct GolemWasiHttpImpl {
    _name: String,
    global: u64,
}

#[agent_implementation]
impl GolemWasiHttp for GolemWasiHttpImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            global: 0,
        }
    }

    fn run(&self) -> String {
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

    fn start_polling(&self, until: String) {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let component_id =
            std::env::var("GOLEM_COMPONENT_ID").unwrap_or("unknown".to_string());
        let worker_name =
            std::env::var("GOLEM_WORKER_NAME").unwrap_or("unknown".to_string());

        println!("Polling until receiving {until}");

        let client = Client::builder().build().unwrap();
        loop {
            println!("Calling the poll endpoint");
            let response = client
                .get(format!(
                    "http://localhost:{port}/poll?component_id={component_id}&worker_name={worker_name}"
                ))
                .send()
                .unwrap();
            let s = response
                .text()
                .unwrap_or_else(|err| format!("error receiving body: {err:?}"));
            println!("Received {s}");
            if s == until {
                println!("Poll loop finished");
                return;
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }

    fn increment(&mut self) {
        self.global += 1;
    }

    fn get_count(&self) -> u64 {
        self.global
    }

    fn slow_body_stream(&self) -> u64 {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let client = Client::builder().build().unwrap();

        let response: Response = client
            .get(&format!("http://localhost:{port}/big-byte-array"))
            .send()
            .expect("Request failed");

        match response.bytes() {
            Ok(bytes) => bytes.len() as u64,
            Err(err) => {
                println!("Failed to read response: {:?}", err);
                0
            }
        }
    }
}
