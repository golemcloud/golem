#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::benchmark::direct_rust_exports::benchmark_direct_rust_api::Guest;
use std::time::Duration;

struct Component;

impl Guest for Component {
    fn cpu_intensive(length: f64) -> u32 {
        common_lib::cpu_intensive(length as u32)
    }

    fn echo(input: String) -> String {
        common_lib::echo(input)
    }

    fn large_input(input: Vec<u8>) -> u32 {
        common_lib::large_input(input)
    }

    fn oplog_heavy(length: u32, persistence_on: bool, commit: bool) -> u32 {
        common_lib::oplog_heavy(length, persistence_on, commit)
    }

    fn sleep(millis: u64) -> bool {
        let duration = Duration::from_millis(millis);
        std::thread::sleep(duration);
        true
    }
}

bindings::export!(Component with_types_in bindings);
