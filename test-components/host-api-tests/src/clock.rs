use futures_concurrency::prelude::*;
use golem_rust::{agent_definition, agent_implementation};
use std::thread;
use std::time::Duration;

#[agent_definition]
pub trait Clock {
    fn new(name: String) -> Self;
    fn sleep(&self, secs: u64) -> Result<(), String>;
    fn healthcheck(&self) -> bool;
    async fn sleep_during_request(&self, secs: u64) -> String;
    async fn sleep_during_parallel_requests(&self, secs: u64) -> String;
    async fn sleep_between_requests(&self, secs: u64, n: u64) -> String;
}

pub struct ClockImpl {
    _name: String,
}

#[agent_implementation]
impl Clock for ClockImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn sleep(&self, secs: u64) -> Result<(), String> {
        thread::sleep(Duration::from_secs(secs));
        Ok(())
    }

    fn healthcheck(&self) -> bool {
        true
    }

    async fn sleep_during_request(&self, secs: u64) -> String {
        let response = send_request();
        let timeout = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(secs.saturating_mul(1_000_000_000)).await;
            Err("Timeout".to_string())
        };
        let (Ok(result) | Err(result)) = ((response, timeout)).race().await;
        result
    }

    async fn sleep_during_parallel_requests(&self, secs: u64) -> String {
        let response1 = async {
            let mut result = String::new();
            for _ in 0..5 {
                result.push_str(&format!("{:?}\n", send_request().await))
            }
            Ok(result)
        };
        let response2 = async {
            let mut result = String::new();
            for _ in 0..5 {
                result.push_str(&format!("{:?}\n", send_request().await))
            }
            Ok(result)
        };
        let response3 = async {
            let mut result = String::new();
            for _ in 0..5 {
                result.push_str(&format!("{:?}\n", send_request().await))
            }
            Ok(result)
        };
        let timeout = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(secs.saturating_mul(1_000_000_000)).await;
            Err("Timeout".to_string())
        };
        let (Ok(result) | Err(result)) = ((response1, response2, response3, timeout)).race().await;
        result
    }

    async fn sleep_between_requests(&self, secs: u64, n: u64) -> String {
        let mut result = String::new();
        for _ in 0..(n as usize) {
            result.push_str(&format!("{:?}\n", send_request().await));
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(secs.saturating_mul(1_000_000_000)).await;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sleep_timeout_seconds_to_nanoseconds_must_not_overflow() {
        let overflowing_secs = std::hint::black_box(36_028_797_018_963_968u64);

        let _duration_nanos = overflowing_secs.saturating_mul(1_000_000_000);
    }
}

async fn send_request() -> Result<String, String> {
    let port = std::env::var("PORT").expect("Requires a PORT env var set");
    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/simulated-slow-request"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    response
        .into_body()
        .text()
        .await
        .map_err(|e| e.to_string())
}
