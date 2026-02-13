use golem_rust::{agent_definition, agent_implementation, description};
use wstd::http::body::IntoBody;
use wstd::http::{Client, Request};

#[agent_definition]
#[description("Counter agent V1")]
pub trait CounterAgent {
    // The agent constructor, it's parameters identify the agent
    fn new(name: String) -> Self;

    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    _name: String,
    count: u32,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            count: 0,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}

#[agent_definition]
pub trait Caller {
    fn new() -> Self;
    async fn call(&self, name: String) -> u32;
}

struct CallerImpl {}

#[agent_implementation]
impl Caller for CallerImpl {
    fn new() -> Self {
        Self {}
    }

    async fn call(&self, name: String) -> u32 {
        let mut client = CounterAgentClient::get(name);
        client.increment().await
    }
}

#[agent_definition]
pub trait UpdateTest {
    fn new() -> Self;
    async fn f1(&mut self, speed_ms: u64) -> u64;
    fn f2(&self) -> u64;
    fn f3(&self) -> u64;
}

struct UpdateTestImpl {
    last: u64,
}

#[agent_implementation]
impl UpdateTest for UpdateTestImpl {
    fn new() -> Self {
        Self { last: 0 }
    }

    async fn f1(&mut self, speed_ms: u64) -> u64 {
        let mut current = self.last;

        println!("Starting to count...");

        for _ in 0..30 {
            current += 10;
            report_f1(current).await;
            wstd::task::sleep(wstd::time::Duration::from_millis(speed_ms)).await;
        }

        self.last = current; // In v2, we replace this to current/2
        current
    }

    fn f2(&self) -> u64 {
        let mut buf = [0u8; 8];
        wstd::rand::get_random_bytes(&mut buf);
        u64::from_le_bytes(buf)
    }

    fn f3(&self) -> u64 {
        std::env::args().collect::<Vec<_>>().len() as u64
            + std::env::vars().collect::<Vec<_>>().len() as u64
    }
}

async fn report_f1(current: u64) {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let url = format!("http://localhost:{port}/f1");

    println!("Sending POST {url}");

    let body = current.to_string().into_body();
    let request = Request::post(&url)
        .body(body)
        .expect("Failed to build request");

    let response = Client::new()
        .send(request)
        .await
        .expect("Request failed");

    let status = response.status();

    println!("Received {status}");
}
