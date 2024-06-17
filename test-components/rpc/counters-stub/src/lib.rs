#![allow(warnings)]
use golem_wasm_rpc::*;
#[allow(dead_code)]
mod bindings;
pub struct Api {
    rpc: WasmRpc,
}
impl Api {}
pub struct FutureGetGlobalValueResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureGetAllDroppedResult {
    pub future_invoke_result: FutureInvokeResult,
}
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
pub struct FutureGetValueResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureGetArgsResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureGetEnvResult {
    pub future_invoke_result: FutureInvokeResult,
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureGetGlobalValueResult
for FutureGetGlobalValueResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> u64 {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{get-global-value}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureGetAllDroppedResult
for FutureGetAllDroppedResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> Vec<(String, u64)> {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{get-all-dropped}"
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
    fn blocking_get_global_value(&self) -> u64 {
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
    fn get_global_value(&self) -> wit_bindgen::rt::Resource<FutureGetGlobalValueResult> {
        let result = self
            .rpc
            .async_invoke_and_await("rpc:counters/api.{get-global-value}", &[]);
        wit_bindgen::rt::Resource::new(FutureGetGlobalValueResult {
            future_invoke_result: result,
        })
    }
    fn blocking_get_all_dropped(&self) -> Vec<(String, u64)> {
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
    fn get_all_dropped(&self) -> wit_bindgen::rt::Resource<FutureGetAllDroppedResult> {
        let result = self
            .rpc
            .async_invoke_and_await("rpc:counters/api.{get-all-dropped}", &[]);
        wit_bindgen::rt::Resource::new(FutureGetAllDroppedResult {
            future_invoke_result: result,
        })
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureGetValueResult
for FutureGetValueResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> u64 {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{counter.get-value}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureGetArgsResult
for FutureGetArgsResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> Vec<String> {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{counter.get-args}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .list_elements(|item| item.string().expect("string not found").to_string())
            .expect("list not found"))
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureGetEnvResult
for FutureGetEnvResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.into_handle())
        };
        pollable
    }
    fn get(&self) -> Vec<(String, String)> {
        let result = self
            .future_invoke_result
            .get()
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{counter.get-env}"
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
                        .string()
                        .expect("string not found")
                        .to_string(),
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
                "rpc:counters/api.{counter.new}",
                &[WitValue::builder().string(&name)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.new}"
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
                "rpc:counters/api.{counter.inc-by}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder().u64(value),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.inc-by}"
                ),
            );
        ()
    }
    fn inc_by(&self, value: u64) -> () {
        let result = self
            .rpc
            .invoke(
                "rpc:counters/api.{counter.inc-by}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder().u64(value),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{counter.inc-by}"
                ),
            );
        ()
    }
    fn blocking_get_value(&self) -> u64 {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.get-value}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.get-value}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
    fn get_value(&self) -> wit_bindgen::rt::Resource<FutureGetValueResult> {
        let result = self
            .rpc
            .async_invoke_and_await(
                "rpc:counters/api.{counter.get-value}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            );
        wit_bindgen::rt::Resource::new(FutureGetValueResult {
            future_invoke_result: result,
        })
    }
    fn blocking_get_args(&self) -> Vec<String> {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.get-args}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.get-args}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .list_elements(|item| item.string().expect("string not found").to_string())
            .expect("list not found"))
    }
    fn get_args(&self) -> wit_bindgen::rt::Resource<FutureGetArgsResult> {
        let result = self
            .rpc
            .async_invoke_and_await(
                "rpc:counters/api.{counter.get-args}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            );
        wit_bindgen::rt::Resource::new(FutureGetArgsResult {
            future_invoke_result: result,
        })
    }
    fn blocking_get_env(&self) -> Vec<(String, String)> {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.get-env}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.get-env}"
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
                        .string()
                        .expect("string not found")
                        .to_string(),
                )
            })
            .expect("list not found"))
    }
    fn get_env(&self) -> wit_bindgen::rt::Resource<FutureGetEnvResult> {
        let result = self
            .rpc
            .async_invoke_and_await(
                "rpc:counters/api.{counter.get-env}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            );
        wit_bindgen::rt::Resource::new(FutureGetEnvResult {
            future_invoke_result: result,
        })
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        self.rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.drop}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect("Failed to invoke remote drop");
    }
}
