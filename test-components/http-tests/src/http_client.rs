use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait HttpClient {
    fn new() -> Self;

    async fn run(&self) -> String;
    async fn run_with_interrupt(&self) -> String;
    async fn send_request(&mut self);
    fn process_response(&mut self) -> String;
}

struct HttpClientImpl {
    response: Option<(u16, String)>,
}

#[agent_implementation]
impl HttpClient for HttpClientImpl {
    fn new() -> Self {
        Self { response: None }
    }

    async fn run(&self) -> String {
        let (status, body) = do_send_request().await;
        format!("{status} {body}")
    }

    async fn run_with_interrupt(&self) -> String {
        let (status, body) = do_send_request().await;

        send_restart_request().await;
        format!("{status} {body}")
    }

    async fn send_request(&mut self) {
        self.response = Some(do_send_request().await);
    }

    fn process_response(&mut self) -> String {
        let (status, body) = self.response.take().unwrap();
        format!("{status} {body}")
    }
}

async fn do_send_request() -> (u16, String) {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .post(&format!("http://localhost:{port}/"))
        .header("X-Test", "test-header")
        .body("test-body")
        .send()
        .await
        .expect("Request failed");

    let status = response.status().as_u16();
    let body = response
        .into_body()
        .text()
        .await
        .expect("Response body read failed");
    println!("Got incoming response");
    (status, body)
}

async fn send_restart_request() {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let _ = wasi_fetch::Client::new()
        .post(&format!("http://localhost:{port}/restart"))
        .header("X-Test", "test-header")
        .body("test-body")
        .send()
        .await
        .expect("Request failed");
}
