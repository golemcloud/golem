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
pub struct FutureCounterGetvalueResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureCounterGetargsResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureCounterGetenvResult {
    pub future_invoke_result: FutureInvokeResult,
}
struct Component;
impl crate::bindings::exports::rpc::counters_stub::stub_counters::Guest for Component {
    type Api = crate::Api;
    type FutureGetGlobalValueResult = crate::FutureGetGlobalValueResult;
    type Counter = crate::Counter;
    type FutureCounterGetvalueResult = crate::FutureCounterGetvalueResult;
    type FutureCounterGetargsResult = crate::FutureCounterGetargsResult;
    type FutureCounterGetenvResult = crate::FutureCounterGetenvResult;
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureGetGlobalValueResult
for FutureGetGlobalValueResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.take_handle())
        };
        pollable
    }
    fn get(&self) -> Option<u64> {
        self.future_invoke_result
            .get()
            .map(|result| {
                let result = result
                    .expect(
                        &format!(
                            "Failed to invoke remote {}",
                            "rpc:counters/api.{get-global-value}"
                        ),
                    );
                (result
                    .tuple_element(0)
                    .expect("tuple not found")
                    .u64()
                    .expect("u64 not found"))
            })
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
    fn get_global_value(
        &self,
    ) -> crate::bindings::exports::rpc::counters_stub::stub_counters::FutureGetGlobalValueResult {
        let result = self
            .rpc
            .async_invoke_and_await("rpc:counters/api.{get-global-value}", &[]);
        crate::bindings::exports::rpc::counters_stub::stub_counters::FutureGetGlobalValueResult::new(FutureGetGlobalValueResult {
            future_invoke_result: result,
        })
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureCounterGetvalueResult
for FutureCounterGetvalueResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.take_handle())
        };
        pollable
    }
    fn get(&self) -> Option<u64> {
        self.future_invoke_result
            .get()
            .map(|result| {
                let result = result
                    .expect(
                        &format!(
                            "Failed to invoke remote {}",
                            "rpc:counters/api.{counter.getvalue}"
                        ),
                    );
                (result
                    .tuple_element(0)
                    .expect("tuple not found")
                    .u64()
                    .expect("u64 not found"))
            })
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureCounterGetargsResult
for FutureCounterGetargsResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.take_handle())
        };
        pollable
    }
    fn get(&self) -> Option<Vec<String>> {
        self.future_invoke_result
            .get()
            .map(|result| {
                let result = result
                    .expect(
                        &format!(
                            "Failed to invoke remote {}",
                            "rpc:counters/api.{counter.getargs}"
                        ),
                    );
                (result
                    .tuple_element(0)
                    .expect("tuple not found")
                    .list_elements(|item| {
                        item.string().expect("string not found").to_string()
                    })
                    .expect("list not found"))
            })
    }
}
impl crate::bindings::exports::rpc::counters_stub::stub_counters::GuestFutureCounterGetenvResult
for FutureCounterGetenvResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.take_handle())
        };
        pollable
    }
    fn get(&self) -> Option<Vec<(String, String)>> {
        self.future_invoke_result
            .get()
            .map(|result| {
                let result = result
                    .expect(
                        &format!(
                            "Failed to invoke remote {}",
                            "rpc:counters/api.{counter.getenv}"
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
            })
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
    fn blocking_incby(&self, value: u64) -> () {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.incby}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder().u64(value),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.incby}"
                ),
            );
        ()
    }
    fn incby(&self, value: u64) -> () {
        let result = self
            .rpc
            .invoke(
                "rpc:counters/api.{counter.incby}",
                &[
                    WitValue::builder().handle(self.uri.clone(), self.id),
                    WitValue::builder().u64(value),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke remote {}", "rpc:counters/api.{counter.incby}"
                ),
            );
        ()
    }
    fn blocking_getvalue(&self) -> u64 {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.getvalue}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.getvalue}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
    fn getvalue(
        &self,
    ) -> crate::bindings::exports::rpc::counters_stub::stub_counters::FutureCounterGetvalueResult {
        let result = self
            .rpc
            .async_invoke_and_await(
                "rpc:counters/api.{counter.getvalue}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            );
        crate::bindings::exports::rpc::counters_stub::stub_counters::FutureCounterGetvalueResult::new(FutureCounterGetvalueResult {
            future_invoke_result: result,
        })
    }
    fn blocking_getargs(&self) -> Vec<String> {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.getargs}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.getargs}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .list_elements(|item| item.string().expect("string not found").to_string())
            .expect("list not found"))
    }
    fn getargs(
        &self,
    ) -> crate::bindings::exports::rpc::counters_stub::stub_counters::FutureCounterGetargsResult {
        let result = self
            .rpc
            .async_invoke_and_await(
                "rpc:counters/api.{counter.getargs}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            );
        crate::bindings::exports::rpc::counters_stub::stub_counters::FutureCounterGetargsResult::new(FutureCounterGetargsResult {
            future_invoke_result: result,
        })
    }
    fn blocking_getenv(&self) -> Vec<(String, String)> {
        let result = self
            .rpc
            .invoke_and_await(
                "rpc:counters/api.{counter.getenv}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:counters/api.{counter.getenv}"
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
    fn getenv(
        &self,
    ) -> crate::bindings::exports::rpc::counters_stub::stub_counters::FutureCounterGetenvResult {
        let result = self
            .rpc
            .async_invoke_and_await(
                "rpc:counters/api.{counter.getenv}",
                &[WitValue::builder().handle(self.uri.clone(), self.id)],
            );
        crate::bindings::exports::rpc::counters_stub::stub_counters::FutureCounterGetenvResult::new(FutureCounterGetenvResult {
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
bindings::export!(Component with_types_in bindings);
