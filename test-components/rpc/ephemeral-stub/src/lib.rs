#![allow(warnings)]
use golem_wasm_rpc::*;
#[allow(dead_code)]
mod bindings;
pub struct Api {
    rpc: WasmRpc,
}
impl Api {}
pub struct FutureGetWorkerNameResult {
    pub future_invoke_result: FutureInvokeResult,
}
pub struct FutureGetIdempotencyKeyResult {
    pub future_invoke_result: FutureInvokeResult,
}
struct Component;
impl crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::Guest for Component {
    type Api = crate::Api;
    type FutureGetWorkerNameResult = crate::FutureGetWorkerNameResult;
    type FutureGetIdempotencyKeyResult = crate::FutureGetIdempotencyKeyResult;
}
impl crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::GuestFutureGetWorkerNameResult
for FutureGetWorkerNameResult {
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
                        &format!(
                            "Failed to invoke remote {}",
                            "rpc:ephemeral/api.{get-worker-name}"
                        ),
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
impl crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::GuestFutureGetIdempotencyKeyResult
for FutureGetIdempotencyKeyResult {
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
                        &format!(
                            "Failed to invoke remote {}",
                            "rpc:ephemeral/api.{get-idempotency-key}"
                        ),
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
impl crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::GuestApi for Api {
    fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
        let location = golem_wasm_rpc::Uri {
            value: location.value,
        };
        Self {
            rpc: WasmRpc::new(&location),
        }
    }
    fn blocking_get_worker_name(&self) -> String {
        let result = self
            .rpc
            .invoke_and_await("rpc:ephemeral/api.{get-worker-name}", &[])
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:ephemeral/api.{get-worker-name}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .string()
            .expect("string not found")
            .to_string())
    }
    fn get_worker_name(
        &self,
    ) -> crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::FutureGetWorkerNameResult {
        let result = self
            .rpc
            .async_invoke_and_await("rpc:ephemeral/api.{get-worker-name}", &[]);
        crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::FutureGetWorkerNameResult::new(FutureGetWorkerNameResult {
            future_invoke_result: result,
        })
    }
    fn blocking_get_idempotency_key(&self) -> String {
        let result = self
            .rpc
            .invoke_and_await("rpc:ephemeral/api.{get-idempotency-key}", &[])
            .expect(
                &format!(
                    "Failed to invoke-and-await remote {}",
                    "rpc:ephemeral/api.{get-idempotency-key}"
                ),
            );
        (result
            .tuple_element(0)
            .expect("tuple not found")
            .string()
            .expect("string not found")
            .to_string())
    }
    fn get_idempotency_key(
        &self,
    ) -> crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::FutureGetIdempotencyKeyResult {
        let result = self
            .rpc
            .async_invoke_and_await("rpc:ephemeral/api.{get-idempotency-key}", &[]);
        crate::bindings::exports::rpc::ephemeral_stub::stub_ephemeral::FutureGetIdempotencyKeyResult::new(FutureGetIdempotencyKeyResult {
            future_invoke_result: result,
        })
    }
}
bindings::export!(Component with_types_in bindings);
