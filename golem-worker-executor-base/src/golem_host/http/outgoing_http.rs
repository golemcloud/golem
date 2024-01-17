use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi_http::bindings::http::types::ErrorCode;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::{
    FutureIncomingResponse, Host, OutgoingRequest, RequestOptions,
};

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn handle(
        &mut self,
        request: Resource<OutgoingRequest>,
        options: Option<Resource<RequestOptions>>,
    ) -> anyhow::Result<Result<Resource<FutureIncomingResponse>, ErrorCode>> {
        record_host_function_call("http::outgoing_handler", "handle");
        // Durability is handled by the WasiHttpView send_request method and the follow-up calls to await/poll the response future
        Host::handle(&mut self.as_wasi_http_view(), request, options)
    }
}
