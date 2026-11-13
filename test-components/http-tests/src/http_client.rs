use golem_rust::http::{self, Request};
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait HttpClient {
    fn new() -> Self;

    async fn run(&self) -> String;
    async fn run_with_interrupt(&self) -> String;
    async fn send_with_random_key(&self, explicit_key: bool) -> String;
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

    async fn send_with_random_key(&self, explicit_key: bool) -> String {
        let port = std::env::var("PORT").unwrap();
        let key = format!(
            "{:016x}{:016x}:reserve_inventory",
            golem_rust::wasip3::random::random::get_random_u64(),
            golem_rust::wasip3::random::random::get_random_u64(),
        );
        let mut request = Request::post(format!("http://localhost:{port}/reservations"))
            .header("x-reservation-id", &key);
        if explicit_key {
            request = request.header("idempotency-key", &key);
        }
        let response = http::send(request.body(Vec::new()).unwrap())
            .await
            .expect("Reservation failed");
        assert_eq!(response.status().as_u16(), 204);
        key
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

    let request = Request::post(format!("http://localhost:{port}/"))
        .header("X-Test", "test-header")
        .header("Content-Length", "9")
        .body(b"test-body".to_vec())
        .expect("Invalid request");
    let response = http::send(request).await.expect("Request failed");

    let status = response.status().as_u16();
    let body = http::response_text(response)
        .await
        .expect("Response body read failed");
    println!("Got incoming response");
    (status, body)
}

async fn send_restart_request() {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let request = Request::post(format!("http://localhost:{port}/restart"))
        .header("X-Test", "test-header")
        .body(b"test-body".to_vec())
        .expect("Invalid request");
    let _ = http::send(request).await.expect("Request failed");
}
