use async_trait::async_trait;

use crate::golem_host::{Durability, GolemCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::random::insecure_seed::Host;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn insecure_seed(&mut self) -> anyhow::Result<(u64, u64)> {
        record_host_function_call("random::insecure_seed", "insecure_seed");
        Durability::<Ctx, (u64, u64), SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem random::insecure_seed::insecure_seed",
            |ctx| Box::pin(async { Host::insecure_seed(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }
}
