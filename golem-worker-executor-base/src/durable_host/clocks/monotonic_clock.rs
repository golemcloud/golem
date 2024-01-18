use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::{Durability, DurableWorkerCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::clocks::monotonic_clock::{
    Duration, Host, Instant, Pollable,
};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> anyhow::Result<Instant> {
        record_host_function_call("clocks::monotonic_clock", "now");
        Durability::<Ctx, Instant, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "monotonic_clock::now",
            |ctx| Box::pin(async { Host::now(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn resolution(&mut self) -> anyhow::Result<Instant> {
        record_host_function_call("clocks::monotonic_clock", "resolution");
        Durability::<Ctx, Instant, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "monotonic_clock::resolution",
            |ctx| Box::pin(async { Host::resolution(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn subscribe_instant(&mut self, when: Instant) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("clocks::monotonic_clock", "subscribe_instant");
        Host::subscribe_instant(&mut self.as_wasi_view(), when).await
    }

    async fn subscribe_duration(&mut self, when: Duration) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("clocks::monotonic_clock", "subscribe_duration");
        let now = self.now().await?;
        self.commit_oplog().await;
        let when = now.saturating_add(when);
        Host::subscribe_instant(&mut self.as_wasi_view(), when).await
    }
}
