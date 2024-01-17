use async_trait::async_trait;

use crate::golem_host::{Durability, GolemCtx, SerializableDateTime, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::clocks::wall_clock::{Datetime, Host};

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn now(&mut self) -> anyhow::Result<Datetime> {
        record_host_function_call("clocks::wall_clock", "now");
        Durability::<Ctx, SerializableDateTime, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "wall_clock::now",
            |ctx| Box::pin(async { Host::now(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn resolution(&mut self) -> anyhow::Result<Datetime> {
        record_host_function_call("clocks::wall_clock", "resolution");
        Durability::<Ctx, SerializableDateTime, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "wall_clock::resolution",
            |ctx| Box::pin(async { Host::resolution(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }
}
