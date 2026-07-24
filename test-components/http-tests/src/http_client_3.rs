use futures_concurrency::future::Join;
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
pub trait HttpClient3 {
    fn new() -> Self;

    async fn run(&self) -> String;
    async fn run_parallel(&self, n: u16) -> Vec<String>;
    async fn start_polling(&self, until: String);
    fn increment(&mut self);
    fn get_count(&self) -> u64;
    async fn slow_body_stream(&self) -> u64;
}

struct HttpClient3Impl {
    global: u64,
}

#[agent_implementation]
impl HttpClient3 for HttpClient3Impl {
    fn new() -> Self {
        Self { global: 0 }
    }

    async fn run(&self) -> String {
        println!("Hello wasi-fetch!");

        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let request_body = ExampleRequest {
            name: "Something".to_string(),
            amount: 42,
            comments: vec!["Hello".to_string(), "World".to_string()],
        };

        println!("Sending {:?}", request_body);

        let response = wasi_fetch::Client::new()
            .post(&format!("http://localhost:{port}/post-example"))
            .json(&request_body)
            .header("X-Test", "Golem")
            // Basic auth for user "some" and password "body"
            .header("Authorization", "Basic c29tZTpib2R5")
            .send()
            .await
            .expect("Request failed");

        let status = response.status();
        let body = response
            .into_body()
            .json::<ExampleResponse>()
            .await
            .expect("Invalid response");

        println!("Received {:?} {:?}", status, body);

        format!("{:?} {:?}", status, body)
    }

    async fn run_parallel(&self, n: u16) -> Vec<String> {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let mut send_futures = Vec::new();
        for i in 0..n {
            let request_body = ExampleRequest {
                name: "Something".to_string(),
                amount: i as u32,
                comments: vec!["Hello".to_string(), "World".to_string()],
            };

            let port_clone = port.clone();
            let future = async move {
                let response = wasi_fetch::Client::new()
                    .post(&format!("http://localhost:{port_clone}/post-example"))
                    .json(&request_body)
                    .header("X-Test", i.to_string())
                    // Basic auth for user "some" and password "body"
                    .header("Authorization", "Basic c29tZTpib2R5")
                    .send()
                    .await
                    .expect("Request failed");

                let status = response.status();
                let body = response
                    .into_body()
                    .json::<ExampleResponse>()
                    .await
                    .expect("Invalid response");

                println!("Received {:?} {:?}", status, body);
                format!("{:?} {:?}", status, body)
            };

            send_futures.push(future);
        }

        send_futures.join().await
    }

    async fn start_polling(&self, until: String) {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let component_id = std::env::var("GOLEM_COMPONENT_ID").unwrap_or("unknown".to_string());
        let worker_name = std::env::var("GOLEM_WORKER_NAME").unwrap_or("unknown".to_string());

        println!("Polling until receiving {until}");

        loop {
            println!("Calling the poll endpoint");
            match wasi_fetch::Client::new()
                .get(&format!(
                    "http://localhost:{port}/poll?component_id={component_id}&worker_name={worker_name}"
                ))
                .send()
                .await
            {
                Ok(response) => {
                    let s = response
                        .into_body()
                        .text()
                        .await
                        .unwrap_or_else(|err| format!("error receiving body: {err:?}"));
                    println!("Received {s}");
                    if s == until {
                        println!("Poll loop finished");
                        return;
                    } else {
                        golem_rust::wasip3::clocks::monotonic_clock::wait_for(100_000_000).await;
                    }
                }
                Err(err) => {
                    println!("Failed to poll: {err:?}");
                }
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
        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let response = wasi_fetch::Client::new()
            .get(&format!("http://localhost:{port}/big-byte-array"))
            .send()
            .await
            .expect("Request failed");

        response.into_body().bytes().await.len() as u64
    }
}
