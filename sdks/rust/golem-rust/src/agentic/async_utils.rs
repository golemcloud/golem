use crate::agentic::{get_reactor, get_state};
use golem_wasm::{FutureInvokeResult, RpcError, WitValue};
use wasi::clocks::monotonic_clock::subscribe_duration;

pub async fn await_async_invoke_result(
    invoke_result: FutureInvokeResult,
) -> Result<WitValue, RpcError> {
    let golem_wasm_pollable = invoke_result.subscribe();

    let pollable_wasi = unsafe {
        std::mem::transmute(golem_wasm_pollable)
    };

    let reactor =  get_state().async_runtime.borrow().reactor.clone().unwrap();

    let _ = reactor.wait_for(pollable_wasi).await;

    invoke_result.get().expect("rpc call failed")
}
