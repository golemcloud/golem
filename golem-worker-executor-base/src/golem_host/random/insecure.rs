use async_trait::async_trait;

use crate::golem_host::{Durability, GolemCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::random::insecure::Host;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn get_insecure_random_bytes(&mut self, len: u64) -> anyhow::Result<Vec<u8>> {
        record_host_function_call("random::insecure", "get_insecure_random_bytes");
        Durability::<Ctx, Vec<u8>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem random::insecure::get_insecure_random_bytes",
            |ctx| {
                Box::pin(async move {
                    Host::get_insecure_random_bytes(&mut ctx.as_wasi_view(), len).await
                })
            },
        )
        .await
    }

    async fn get_insecure_random_u64(&mut self) -> anyhow::Result<u64> {
        record_host_function_call("random::insecure", "get_insecure_random_u64");
        Durability::<Ctx, u64, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem random::insecure::get_insecure_random_u64",
            |ctx| Box::pin(async { Host::get_insecure_random_u64(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }
}
