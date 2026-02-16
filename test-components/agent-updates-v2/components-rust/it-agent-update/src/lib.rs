use golem_rust::{agent_definition, agent_implementation, description};
use wstd::http::body::IntoBody;
use wstd::http::{Client, Request};

#[agent_definition]
#[description("Counter agent V2")]
pub trait CounterAgent {
    // CHANGE from v1: 'name: String' replaced with a 'name: u64'
    fn new(name: u64) -> Self;

    fn increment(&mut self) -> u32;
    fn decrement(&mut self) -> Option<u32>;
}

struct CounterImpl {
    _id: u64,
    count: u32,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: u64) -> Self {
        Self {
            _id: name,
            count: 0,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn decrement(&mut self) -> Option<u32> {
        if self.count > 0 {
            self.count -= 1;
            Some(self.count)
        } else {
            None
        }
    }
}

#[agent_definition]
pub trait NewCaller {
    fn new() -> Self;
    async fn call(&self, name: u64) -> u32;
}

struct NewCallerImpl {}

#[agent_implementation]
impl NewCaller for NewCallerImpl {
    fn new() -> Self {
        Self {}
    }

    async fn call(&self, name: u64) -> u32 {
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
    fn f4(&self) -> u64;
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
            println!("Current count: {}", current);
            wstd::task::sleep(wstd::time::Duration::from_millis(speed_ms)).await;
        }

        println!("Finished to count...");

        self.last = current / 2;
        self.last
    }

    fn f2(&self) -> u64 {
        self.last
    }

    fn f3(&self) -> u64 {
        std::env::args().collect::<Vec<_>>().len() as u64
            + std::env::vars().collect::<Vec<_>>().len() as u64
    }

    fn f4(&self) -> u64 {
        11
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        let mut result = Vec::new();
        result.extend_from_slice(&self.last.to_be_bytes());
        Ok(result)
    }
}

#[agent_definition]
pub trait RevisionEnvAgent {
    fn new() -> Self;
    fn get_revision_from_env_var(&self) -> String;
}

struct RevisionEnvAgentImpl;

#[agent_implementation]
impl RevisionEnvAgent for RevisionEnvAgentImpl {
    fn new() -> Self {
        Self
    }

    fn get_revision_from_env_var(&self) -> String {
        std::env::var("GOLEM_COMPONENT_REVISION").unwrap_or_default()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
        Ok(())
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
