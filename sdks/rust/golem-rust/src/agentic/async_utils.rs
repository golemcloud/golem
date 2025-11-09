use crate::agentic::get_reactor;
use golem_wasm::{FutureInvokeResult, RpcError, WitValue};

pub async fn await_async_invoke_result(
    invoke_result: FutureInvokeResult,
) -> Result<WitValue, RpcError> {
    let golem_wasm_pollable = invoke_result.subscribe();

    let pollable_wasi = unsafe { std::mem::transmute(golem_wasm_pollable) };

    let reactor = get_reactor();

    let _ = reactor.wait_for(pollable_wasi).await;

    invoke_result.get().expect("rpc call failed")
}
