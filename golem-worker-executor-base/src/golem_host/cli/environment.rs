use async_trait::async_trait;

use crate::golem_host::{Durability, GolemCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::cli::environment::Host;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn get_environment(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        record_host_function_call("cli::environment", "get_environment");
        Durability::<Ctx, Vec<(String, String)>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem_environment::get_environment",
            |ctx| Box::pin(async { Host::get_environment(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn get_arguments(&mut self) -> anyhow::Result<Vec<String>> {
        record_host_function_call("cli::environment", "get_arguments");
        Durability::<Ctx, Vec<String>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem_environment::get_arguments",
            |ctx| Box::pin(async { Host::get_arguments(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }

    async fn initial_cwd(&mut self) -> anyhow::Result<Option<String>> {
        record_host_function_call("cli::environment", "initial_cwd");
        Durability::<Ctx, Option<String>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "golem_environment::get_arguments",
            |ctx| Box::pin(async { Host::initial_cwd(&mut ctx.as_wasi_view()).await }),
        )
        .await
    }
}
