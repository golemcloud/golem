use anyhow::anyhow;
use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::{Durability, DurableWorkerCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::filesystem::preopens::{Descriptor, Host};

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_directories(&mut self) -> anyhow::Result<Vec<(Resource<Descriptor>, String)>> {
        record_host_function_call("cli_base::preopens", "get_directories");

        let current_dirs1 = Host::get_directories(&mut self.as_wasi_view()).await?;
        let current_dirs2 = Host::get_directories(&mut self.as_wasi_view()).await?;
        Durability::<Ctx, Vec<String>, SerializableError>::custom_wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "cli::preopens::get_directories",
            |_ctx| Box::pin(async move { Ok(current_dirs1) }),
            |_ctx, dirs| {
                // We can only serialize the names
                Ok(dirs
                    .iter()
                    .map(|(_, name)| name.clone())
                    .collect::<Vec<String>>())
            },
            move |_ctx, names| {
                Box::pin(async move {
                    // Filtering the current set of pre-opened directories by the serialized names
                    let filtered = current_dirs2
                        .into_iter()
                        .filter(|(_, name)| names.contains(name))
                        .collect::<Vec<_>>();

                    if filtered.len() == names.len() {
                        // All directories were found
                        Ok(filtered)
                    } else {
                        Err(anyhow!(
                            "Not all previously available pre-opened directories were found"
                        ))
                    }
                })
            },
        )
        .await
    }
}
