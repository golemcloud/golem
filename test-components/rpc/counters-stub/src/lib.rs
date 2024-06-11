#![allow(warnings)]
use golem_wasm_rpc::*;
#[allow(dead_code)]
mod bindings;
pub struct Api {
    rpc: WasmRpc,
}
impl Api {}
pub struct Counter {
    rpc: WasmRpc,
    id: u64,
    uri: golem_wasm_rpc::Uri,
}
impl Counter {
    pub fn from_remote_handle(uri: golem_wasm_rpc::Uri, id: u64) -> Self {
        Self {
            rpc: WasmRpc::new(&uri),
            id,
            uri,
        }
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestApi for Api {
    fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
        let location = golem_wasm_rpc::Uri {
            value: location.value,
        };
        Self {
            rpc: WasmRpc::new(&location),
        }
    }
    fn blocking_inc_global_by(&self, value: u64) -> () {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{inc-global-by}",
                &[WitValue::builder().u64(value)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{inc-global-by}"
                ),
            );
        ()
    }
    fn inc_global_by(&self, value: u64) -> () {
        let result = self
            .rpc
            .invoke(
                "rpc:counters/api.{inc-global-by}",
                &[WitValue::builder().u64(value)],
            )
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{inc-global-by}"
                ),
            );
        ()
    }
    fn get_global_value(&self) -> u64 {
        let result = self
            .rpc
            .invoke_and_await("rpc:counters/api.{get-global-value}", &[])
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{get-global-value}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
    fn get_all_dropped(&self) -> Vec<(String, u64)> {
        let result = self
            .rpc
            .invoke_and_await("rpc:counters/api.{get-all-dropped}", &[])
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{get-all-dropped}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .list_elements(|item| {
                let tuple = item;
                (
                    tuple
                        .tuple_element(0usize)
                        .expect("tuple element not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                    tuple
                        .tuple_element(1usize)
                        .expect("tuple element not found")
                        .u64()
                        .expect("u64 not found"),
                )
            })
            .expect("list not found"))
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestCounter
for Counter {
    fn new(location: crate::bindings::golem::rpc::types::Uri, name: String) -> Self {
        let location = golem_wasm_rpc::Uri {
            value: location.value,
        };
        let rpc = WasmRpc::new(&location);
        let result = rpc
            .invoke_and_await(
                "rpc:counters/api/counter.{new}",
                &[WitValue::builder().string(&name)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api/counter.{new}"
                ),
            );
        ({
            let (uri, id) = result
                .tuple_element(0)
                .expect("tuple not found")
                .handle()
                .expect("handle not found");
            Self { rpc, id, uri }
        })
    }
    fn blocking_inc_by(&self, value: u64) -> () {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api/counter.{inc-by}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder().u64(value),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api/counter.{inc-by}"
                ),
            );
        ()
    }
    fn inc_by(&self, value: u64) -> () {
        let result = self
            .rpc
            .invoke(
                "rpc:counters/api/counter.{inc-by}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder().u64(value),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api/counter.{inc-by}"
                ),
            );
        ()
    }
    fn get_value(&self) -> u64 {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api/counter.{get-value}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api/counter.{get-value}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        self.rpc
            .invoke_and_await(
                "rpc:counters/api/counter.{drop}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect("Failed to invoke remote drop");
    }
}
