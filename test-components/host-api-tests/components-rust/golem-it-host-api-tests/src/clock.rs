use golem_rust::{agent_definition, agent_implementation};
use futures_concurrency::prelude::*;
use std::thread;
use std::time::Duration;
use wstd::http::{Client, Request};
use wstd::io::empty;

#[agent_definition]
pub trait Clock {
    fn new(name: String) -> Self;
    fn sleep(&self, secs: u64) -> Result<(), String>;
    fn healthcheck(&self) -> bool;
    fn sleep_during_request(&self, secs: u64) -> String;
    fn sleep_during_parallel_requests(&self, secs: u64) -> String;
    fn sleep_between_requests(&self, secs: u64, n: u64) -> String;
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

    fn sleep_during_request(&self, secs: u64) -> String {
        wstd::runtime::block_on(async {
            let response = send_request();
            let timeout = async {
                wstd::task::sleep(wstd::time::Duration::from_secs(secs)).await;
                Err("Timeout".to_string())
            };
            let (Ok(result) | Err(result)) = ((response, timeout)).race().await;
            result
        })
    }

    fn sleep_during_parallel_requests(&self, secs: u64) -> String {
        wstd::runtime::block_on(async {
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
                wstd::task::sleep(wstd::time::Duration::from_secs(secs)).await;
                Err("Timeout".to_string())
            };
            let (Ok(result) | Err(result)) =
                ((response1, response2, response3, timeout)).race().await;
            result
        })
    }

    fn sleep_between_requests(&self, secs: u64, n: u64) -> String {
        wstd::runtime::block_on(async {
            let mut result = String::new();
            for _ in 0..(n as usize) {
                result.push_str(&format!("{:?}\n", send_request().await));
                wstd::task::sleep(wstd::time::Duration::from_secs(secs)).await;
            }
            result
        })
    }
}

async fn send_request() -> Result<String, String> {
    let port = std::env::var("PORT").expect("Requires a PORT env var set");
    let request = Request::get(format!("http://localhost:{port}/simulated-slow-request"))
        .body(empty())
        .map_err(|e| e.to_string())?;

    let mut response = Client::new()
        .send(request)
        .await
        .map_err(|e| e.to_string())?;
    let body = response.body_mut();
    let bytes = body.bytes().await.map_err(|e| e.to_string())?;
    Ok(String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?)
}
