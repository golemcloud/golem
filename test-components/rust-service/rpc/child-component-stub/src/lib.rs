#![allow(warnings)]
use golem_wasm_rpc::*;
#[allow(dead_code)]
mod bindings;
pub struct Api {
    rpc: WasmRpc,
}
impl Api {}
pub struct FutureEchoResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureCalculateResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureProcessResult {
    pub future_invoke_result: FutureInvokeResult,
}
struct Component;
impl crate::bindings::exports::golem::it_stub::stub_child_component::Guest
for Component {
    type Api = crate::Api;
    type FutureEchoResult = crate::FutureEchoResult;
    type FutureCalculateResult = crate::FutureCalculateResult;
    type FutureProcessResult = crate::FutureProcessResult;
}
impl crate::bindings::exports::golem::it_stub::stub_child_component::GuestFutureEchoResult
for FutureEchoResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.take_handle())
        };
        pollable
    }
    fn get(&self) -> Option<String> {
        self.future_invoke_result
            .get()
            .map(|result| {
                let result = result
                    .expect(
                        &format!("Failed to invoke remote {}", "golem:it/api.{echo}"),
                    );
                (result
                    .tuple_element(0)
                    .expect("tuple not found")
                    .string()
                    .expect("string not found")
                    .to_string())
            })
    }
}
impl crate::bindings::exports::golem::it_stub::stub_child_component::GuestFutureCalculateResult
for FutureCalculateResult {
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
                            "Failed to invoke remote {}", "golem:it/api.{calculate}"
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
impl crate::bindings::exports::golem::it_stub::stub_child_component::GuestFutureProcessResult
for FutureProcessResult {
    fn subscribe(&self) -> bindings::wasi::io::poll::Pollable {
        let pollable = self.future_invoke_result.subscribe();
        let pollable = unsafe {
            bindings::wasi::io::poll::Pollable::from_handle(pollable.take_handle())
        };
        pollable
    }
    fn get(&self) -> Option<Vec<crate::bindings::golem::it::api::Data>> {
        self.future_invoke_result
            .get()
            .map(|result| {
                let result = result
                    .expect(
                        &format!("Failed to invoke remote {}", "golem:it/api.{process}"),
                    );
                (result
                    .tuple_element(0)
                    .expect("tuple not found")
                    .list_elements(|item| {
                        let record = item;
                        crate::bindings::golem::it::api::Data {
                            id: record
                                .field(0usize)
                                .expect("record field not found")
                                .string()
                                .expect("string not found")
                                .to_string(),
                            name: record
                                .field(1usize)
                                .expect("record field not found")
                                .string()
                                .expect("string not found")
                                .to_string(),
                            desc: record
                                .field(2usize)
                                .expect("record field not found")
                                .string()
                                .expect("string not found")
                                .to_string(),
                            timestamp: record
                                .field(3usize)
                                .expect("record field not found")
                                .u64()
                                .expect("u64 not found"),
                        }
                    })
                    .expect("list not found"))
            })
    }
}
impl crate::bindings::exports::golem::it_stub::stub_child_component::GuestApi for Api {
    fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
        let location = golem_wasm_rpc::Uri {
            value: location.value,
        };
        Self {
            rpc: WasmRpc::new(&location),
        }
    }
    fn blocking_echo(&self, input: String) -> String {
        let result = self
            .rpc
            .invoke_and_await(
                "golem:it/api.{echo}",
                &[WitValue::builder().string(&input)],
            )
            .expect(
                &format!("Failed to invoke-and-await remote {}", "golem:it/api.{echo}"),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .string()
            .expect("string not found")
            .to_string())
    }
    fn echo(
        &self,
        input: String,
    ) -> crate::bindings::exports::golem::it_stub::stub_child_component::FutureEchoResult {
        let result = self
            .rpc
            .async_invoke_and_await(
                "golem:it/api.{echo}",
                &[WitValue::builder().string(&input)],
            );
        crate::bindings::exports::golem::it_stub::stub_child_component::FutureEchoResult::new(FutureEchoResult {
            future_invoke_result: result,
        })
    }
    fn blocking_calculate(&self, input: u64) -> u64 {
        let result = self
            .rpc
            .invoke_and_await(
                "golem:it/api.{calculate}",
                &[WitValue::builder().u64(input)],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}", "golem:it/api.{calculate}"
                ),
            );
        (result.tuple_element(0).expect("tuple not found").u64().expect("u64 not found"))
    }
    fn calculate(
        &self,
        input: u64,
    ) -> crate::bindings::exports::golem::it_stub::stub_child_component::FutureCalculateResult {
        let result = self
            .rpc
            .async_invoke_and_await(
                "golem:it/api.{calculate}",
                &[WitValue::builder().u64(input)],
            );
        crate::bindings::exports::golem::it_stub::stub_child_component::FutureCalculateResult::new(FutureCalculateResult {
            future_invoke_result: result,
        })
    }
    fn blocking_process(
        &self,
        input: Vec<crate::bindings::golem::it::api::Data>,
    ) -> Vec<crate::bindings::golem::it::api::Data> {
        let result = self
            .rpc
            .invoke_and_await(
                "golem:it/api.{process}",
                &[
                    WitValue::builder()
                        .list_fn(
                            &input,
                            |item, item_builder| {
                                item_builder
                                    .record()
                                    .item()
                                    .string(&item.id)
                                    .item()
                                    .string(&item.name)
                                    .item()
                                    .string(&item.desc)
                                    .item()
                                    .u64(item.timestamp)
                                    .finish()
                            },
                        ),
                ],
            )
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}", "golem:it/api.{process}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .list_elements(|item| {
                let record = item;
                crate::bindings::golem::it::api::Data {
                    id: record
                        .field(0usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                    name: record
                        .field(1usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                    desc: record
                        .field(2usize)
                        .expect("record field not found")
                        .string()
                        .expect("string not found")
                        .to_string(),
                    timestamp: record
                        .field(3usize)
                        .expect("record field not found")
                        .u64()
                        .expect("u64 not found"),
                }
            })
            .expect("list not found"))
    }
    fn process(
        &self,
        input: Vec<crate::bindings::golem::it::api::Data>,
    ) -> crate::bindings::exports::golem::it_stub::stub_child_component::FutureProcessResult {
        let result = self
            .rpc
            .async_invoke_and_await(
                "golem:it/api.{process}",
                &[
                    WitValue::builder()
                        .list_fn(
                            &input,
                            |item, item_builder| {
                                item_builder
                                    .record()
                                    .item()
                                    .string(&item.id)
                                    .item()
                                    .string(&item.name)
                                    .item()
                                    .string(&item.desc)
                                    .item()
                                    .u64(item.timestamp)
                                    .finish()
                            },
                        ),
                ],
            );
        crate::bindings::exports::golem::it_stub::stub_child_component::FutureProcessResult::new(FutureProcessResult {
            future_invoke_result: result,
        })
    }
}
bindings::export!(Component with_types_in bindings);
