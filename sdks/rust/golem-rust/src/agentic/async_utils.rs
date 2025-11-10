use crate::agentic::get_reactor;
use crate::wasm_rpc::golem_rpc_0_2_x::types::Pollable;
use golem_wasm::{FutureInvokeResult, RpcError, WitValue};

pub async fn await_invoke_result(invoke_result: FutureInvokeResult) -> Result<WitValue, RpcError> {
    let golem_wasm_pollable = invoke_result.subscribe();

    await_pollable(golem_wasm_pollable).await;

    invoke_result
        .get()
        .expect("RPC invoke completed, but no result available")
}

pub async fn await_pollable(pollable: Pollable) {
    let reactor = get_reactor();

    let pollable_wasi: wasi::io::poll::Pollable = unsafe { std::mem::transmute(pollable) };

    reactor.wait_for(pollable_wasi).await;
}
