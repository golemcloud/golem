use golem_rust::{agent_definition, agent_implementation};
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
pub trait HttpClient2 {
    fn new() -> Self;

    async fn run(&self) -> String;
    async fn start_polling(&self, until: String);
    fn increment(&mut self);
    fn get_count(&self) -> u64;
    async fn slow_body_stream(&self) -> u64;
}

struct HttpClient2Impl {
    global: u64,
}

#[agent_implementation]
impl HttpClient2 for HttpClient2Impl {
    fn new() -> Self {
        Self { global: 0 }
    }

    async fn run(&self) -> String {
        use golem_wasi_http::*;

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
            .await
            .expect("Request failed");

        let status = response.status();
        let body = response
            .json::<ExampleResponse>()
            .await
            .expect("Invalid response");

        println!("Received {:?} {:?}", status, body);

        format!("{:?} {:?}", status, body)
    }

    async fn start_polling(&self, until: String) {
        use golem_wasi_http::*;

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
                .await
                .unwrap();
            let s = response
                .text()
                .await
                .unwrap_or_else(|err| format!("error receiving body: {err:?}"));
            println!("Received {s}");
            if s == until {
                println!("Poll loop finished");
                return;
            } else {
                wstd::task::sleep(std::time::Duration::from_millis(100).into()).await;
            }
        }
    }

    fn increment(&mut self) {
        self.global += 1;
    }

    fn get_count(&self) -> u64 {
        self.global
    }

    async fn slow_body_stream(&self) -> u64 {
        use golem_wasi_http::*;

        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let client = Client::builder().build().unwrap();

        let response: Response = client
            .get(&format!("http://localhost:{port}/big-byte-array"))
            .send()
            .await
            .expect("Request failed");

        match response.bytes().await {
            Ok(bytes) => bytes.len() as u64,
            Err(err) => {
                println!("Failed to read response: {:?}", err);
                0
            }
        }
    }
}
