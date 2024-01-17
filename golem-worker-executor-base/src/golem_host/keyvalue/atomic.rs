use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::golem_host::GolemCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::keyvalue::atomic::{Bucket, Error, Host, Key};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn increment(
        &mut self,
        _bucket: Resource<Bucket>,
        _key: Key,
        _delta: u64,
    ) -> anyhow::Result<Result<u64, Resource<Error>>> {
        record_host_function_call("keyvalue::atomic", "increment");
        unimplemented!("increment")
    }

    async fn compare_and_swap(
        &mut self,
        _bucket: Resource<Bucket>,
        _key: Key,
        _old: u64,
        _new: u64,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        record_host_function_call("keyvalue::atomic", "compare_and_swap");
        unimplemented!("compare_and_swap")
    }
}
