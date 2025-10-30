#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::benchmark::direct_rust_rpc_child_client::benchmark_direct_rust_rpc_child_client::BenchmarkDirectRustRpcChildApi;
use crate::bindings::exports::benchmark::direct_rust_rpc_parent_exports::benchmark_direct_rust_rpc_parent_api::Guest;

struct Component;

impl Guest for Component {
    fn cpu_intensive(length: f64) -> u32 {
        let client = create_client();
        client.blocking_cpu_intensive(length)
    }

    fn echo(input: String) -> String {
        let client = create_client();
        client.blocking_echo(&input)
    }

    fn large_input(input: Vec<u8>) -> u32 {
        let client = create_client();
        client.blocking_large_input(&input)
    }
}

fn create_client() -> BenchmarkDirectRustRpcChildApi {
    let worker_name = std::env::var("GOLEM_WORKER_NAME").expect("Missing GOLEM_WORKER_NAME");
    BenchmarkDirectRustRpcChildApi::new(&worker_name)
}

bindings::export!(Component with_types_in bindings);
