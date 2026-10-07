use futures_concurrency::prelude::*;
use golem_rust::{agent_definition, agent_implementation};
use std::thread;
use std::time::Duration;

async fn race_p3_sleeps_impl(secs: Vec<u64>) -> u64 {
    let waits: Vec<_> = secs
        .into_iter()
        .map(|secs| async move {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                secs.saturating_mul(1_000_000_000),
            )
            .await;
            secs
        })
        .collect();
    waits.race().await
}

async fn race_promise_and_p3_sleep_impl(secs: u64) -> String {
    let promise_id = golem_rust::create_promise();
    let promise = async {
        golem_rust::await_promise(&promise_id).await;
        "promise".to_string()
    };
    let timer = async {
        golem_rust::wasip3::clocks::monotonic_clock::wait_for(secs.saturating_mul(1_000_000_000))
            .await;
        "timer".to_string()
    };
    (promise, timer).race().await
}

#[agent_definition]
pub trait Clock {
    fn new(name: String) -> Self;
    fn sleep(&self, secs: u64) -> Result<(), String>;
    async fn sleep_p3(&self, secs: u64) -> bool;
    async fn race_p3_sleeps(&self, secs: Vec<u64>) -> u64;
    async fn race_promise_and_p3_sleep(&self, secs: u64) -> String;
    async fn polling_loop_vs_watchdog(&self) -> String;
    fn healthcheck(&self) -> bool;
    async fn sleep_during_request(&self, secs: u64) -> String;
    async fn sleep_during_parallel_requests(&self, secs: u64) -> String;
    async fn sleep_between_requests(&self, secs: u64, n: u64) -> String;
    async fn jump_during_request(&self) -> String;
    async fn p2_sleep_during_request(&self, secs: u64) -> String;
    fn p2_poll_duplicate_handles(&self, later_millis: u64, early_millis: u64) -> String;
    fn p2_file_pollables(&self, contents: String) -> String;
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

    async fn sleep_p3(&self, secs: u64) -> bool {
        golem_rust::wasip3::clocks::monotonic_clock::wait_for(secs.saturating_mul(1_000_000_000))
            .await;
        true
    }

    async fn race_p3_sleeps(&self, secs: Vec<u64>) -> u64 {
        race_p3_sleeps_impl(secs).await
    }

    async fn race_promise_and_p3_sleep(&self, secs: u64) -> String {
        race_promise_and_p3_sleep_impl(secs).await
    }

    async fn polling_loop_vs_watchdog(&self) -> String {
        let flag = std::cell::Cell::new(false);
        let set_flag = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(3_000_000_000).await;
            flag.set(true);
        };
        let poll = async {
            while !flag.get() {
                golem_rust::wasip3::clocks::monotonic_clock::wait_for(100_000_000).await;
            }
            "flag".to_string()
        };
        let watchdog = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(600_000_000_000).await;
            "watchdog".to_string()
        };
        let (_, result) = (set_flag, (poll, watchdog).race()).join().await;
        result
    }

    fn healthcheck(&self) -> bool {
        true
    }

    async fn sleep_during_request(&self, secs: u64) -> String {
        let response = send_request();
        let timeout = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                secs.saturating_mul(1_000_000_000),
            )
            .await;
            Err("Timeout".to_string())
        };
        let (Ok(result) | Err(result)) = (response, timeout).race().await;
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
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                secs.saturating_mul(1_000_000_000),
            )
            .await;
            Err("Timeout".to_string())
        };
        let (Ok(result) | Err(result)) = (response1, response2, response3, timeout).race().await;
        result
    }

    async fn sleep_between_requests(&self, secs: u64, n: u64) -> String {
        let mut result = String::new();
        for _ in 0..(n as usize) {
            result.push_str(&format!("{:?}\n", send_request().await));
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                secs.saturating_mul(1_000_000_000),
            )
            .await;
        }
        result
    }

    /// Attempts to jump backwards while an HTTP request is still in flight. The jump must be
    /// rejected by the executor because deleting the region would strand the in-flight call's
    /// `Start` entry.
    async fn jump_during_request(&self) -> String {
        let target = golem_rust::get_oplog_index();
        let request = async { send_request().await.unwrap_or_else(|err| err) };
        let jump = async {
            // Give the request future time to start and register its durable call
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(200_000_000).await;
            golem_rust::set_oplog_index(target);
            "jump-completed".to_string()
        };
        let (request_result, jump_result) = (request, jump).join().await;
        format!("{request_result}, {jump_result}")
    }

    /// Blocks in a P2 sleep (`thread::sleep` goes through `wasi:io/poll@0.2.x`) while a P3 HTTP
    /// request is still in flight. The executor must not suspend the worker while the request is
    /// pending: a premature suspend would drop the in-flight call and re-execute the request on
    /// resume.
    async fn p2_sleep_during_request(&self, secs: u64) -> String {
        let request = async { send_request().await.unwrap_or_else(|err| err) };
        let sleep = async {
            // Give the request future time to start and register its durable call
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(200_000_000).await;
            thread::sleep(Duration::from_secs(secs));
            "slept".to_string()
        };
        let (request_result, sleep_result) = (request, sleep).join().await;
        format!("{request_result}, {sleep_result}")
    }

    fn p2_file_pollables(&self, contents: String) -> String {
        use wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

        let (root, _) = wasi::filesystem::preopens::get_directories()
            .into_iter()
            .next()
            .unwrap();
        let file = root
            .open_at(
                PathFlags::empty(),
                "p2-pollables.txt",
                OpenFlags::CREATE,
                DescriptorFlags::READ | DescriptorFlags::WRITE,
            )
            .unwrap();
        let offset = file.stat().unwrap().size;
        let output = file.write_via_stream(offset).unwrap();
        let writable = output.subscribe();
        writable.block();
        let write_ready = writable.ready();
        output
            .blocking_write_and_flush(contents.as_bytes())
            .unwrap();
        drop(writable);
        drop(output);

        let input = file.read_via_stream(0).unwrap();
        let readable = input.subscribe();
        readable.block();
        let read_ready = readable.ready();
        // Read in unequal chunks to exercise stream position independently of write size.
        let mut bytes = input.blocking_read(2).unwrap();
        while bytes.len() < (offset as usize + contents.len()) {
            bytes.extend(input.blocking_read(3).unwrap());
        }
        format!(
            "{write_ready};{read_ready};{}",
            String::from_utf8(bytes).unwrap()
        )
    }

    fn p2_poll_duplicate_handles(&self, later_millis: u64, early_millis: u64) -> String {
        use wasi::clocks::monotonic_clock::subscribe_duration;
        use wasi::io::poll::poll;

        let later = subscribe_duration(later_millis.saturating_mul(1_000_000));
        let early = subscribe_duration(early_millis.saturating_mul(1_000_000));
        let first = poll(&[&later, &early, &early]);
        let early_again = poll(&[&early, &early]);
        drop(early);
        let later_ready = poll(&[&later]);
        let later_again = poll(&[&later]);
        drop(later);
        let replacement = subscribe_duration(0);
        let replacement_ready = poll(&[&replacement]);

        format!("{first:?};{early_again:?};{later_ready:?};{later_again:?};{replacement_ready:?}")
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
    response.into_body().text().await.map_err(|e| e.to_string())
}
