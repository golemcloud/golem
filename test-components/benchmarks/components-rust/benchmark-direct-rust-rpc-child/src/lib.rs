#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::benchmark::direct_rust_rpc_child_exports::benchmark_direct_rust_rpc_child_api::Guest;


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
}

bindings::export!(Component with_types_in bindings);
