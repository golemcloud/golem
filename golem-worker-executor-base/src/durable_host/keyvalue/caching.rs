// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::keyvalue::cache::{
    Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry,
    Host, HostFutureExistsResult, HostFutureGetOrSetResult, HostFutureGetResult, HostFutureResult,
    HostVacancy, IncomingValue, Key, OutgoingValue, Pollable, Vacancy,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureGetResult for DurableWorkerCtx<Ctx> {
    async fn future_get_result_get(
        &mut self,
        _self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Option<Result<Option<Resource<IncomingValue>>, Resource<Error>>>> {
        record_host_function_call("keyvalue::cache::future_get", "future_get_result_get");
        unimplemented!("future_get_result_get")
    }

    async fn listen_to_future_get_result(
        &mut self,
        _self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("keyvalue::cache::future_get", "listen_to_future_get_result");
        unimplemented!("listen_to_future_get_result")
    }

    fn drop(&mut self, _rep: Resource<FutureGetResult>) -> anyhow::Result<()> {
        record_host_function_call("keyvalue::cache::future_get", "drop");
        unimplemented!("drop")
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureExistsResult for DurableWorkerCtx<Ctx> {
    async fn future_exists_result_get(
        &mut self,
        _self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Option<Result<bool, Resource<Error>>>> {
        record_host_function_call("keyvalue::cache::future_exists", "future_exists_result_get");
        unimplemented!("future_exists_result_get")
    }

    async fn listen_to_future_exists_result(
        &mut self,
        _self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call(
            "keyvalue::cache::future_exists",
            "listen_to_future_exists_result",
        );
        unimplemented!("listen_to_future_exists_result")
    }

    fn drop(&mut self, _rep: Resource<FutureExistsResult>) -> anyhow::Result<()> {
        record_host_function_call("keyvalue::cache::future_exists", "drop");
        unimplemented!("drop")
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureResult for DurableWorkerCtx<Ctx> {
    async fn future_result_get(
        &mut self,
        _self_: Resource<FutureResult>,
    ) -> anyhow::Result<Option<Result<(), Resource<Error>>>> {
        record_host_function_call("keyvalue::cache::future_result", "future_result_get");
        unimplemented!("future_result_get")
    }

    async fn listen_to_future_result(
        &mut self,
        _self_: Resource<FutureResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("keyvalue::cache::future_result", "listen_to_future_result");
        unimplemented!("listen_to_future_result")
    }

    fn drop(&mut self, _rep: Resource<FutureResult>) -> anyhow::Result<()> {
        record_host_function_call("keyvalue::cache::future_result", "drop");
        unimplemented!("drop")
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureGetOrSetResult for DurableWorkerCtx<Ctx> {
    async fn future_get_or_set_result_get(
        &mut self,
        _self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Option<Result<GetOrSetEntry, Resource<Error>>>> {
        record_host_function_call(
            "keyvalue::cache::future_get_or_set",
            "future_get_or_set_result_get",
        );
        unimplemented!("future_get_or_set_result_get")
    }

    async fn listen_to_future_get_or_set_result(
        &mut self,
        _self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call(
            "keyvalue::cache::future_get_or_set",
            "listen_to_future_get_or_set_result",
        );
        unimplemented!("listen_to_future_get_or_set_result")
    }

    fn drop(&mut self, _rep: Resource<FutureGetOrSetResult>) -> anyhow::Result<()> {
        record_host_function_call("keyvalue::cache::future_get_or_set", "drop");
        unimplemented!("drop")
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostVacancy for DurableWorkerCtx<Ctx> {
    async fn vacancy_fill(
        &mut self,
        _self_: Resource<Vacancy>,
        _ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<OutgoingValue>> {
        record_host_function_call("keyvalue::cache::vacancy", "vacancy_fill");
        unimplemented!("vacancy_fill")
    }

    fn drop(&mut self, _rep: Resource<Vacancy>) -> anyhow::Result<()> {
        record_host_function_call("keyvalue::cache::vacancy", "drop");
        unimplemented!("drop")
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(&mut self, _k: Key) -> anyhow::Result<Resource<FutureGetResult>> {
        record_host_function_call("keyvalue::cache", "get");
        unimplemented!("get")
    }

    async fn exists(&mut self, _k: Key) -> anyhow::Result<Resource<FutureExistsResult>> {
        record_host_function_call("keyvalue::cache", "exists");
        unimplemented!("exists")
    }

    async fn set(
        &mut self,
        _k: Key,
        _v: Resource<OutgoingValue>,
        _ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<FutureResult>> {
        record_host_function_call("keyvalue::cache", "set");
        unimplemented!("set")
    }

    async fn get_or_set(&mut self, _k: Key) -> anyhow::Result<Resource<FutureGetOrSetResult>> {
        record_host_function_call("keyvalue::cache", "get_or_set");
        unimplemented!("get_or_set")
    }

    async fn delete(&mut self, _k: Key) -> anyhow::Result<Resource<FutureResult>> {
        record_host_function_call("keyvalue::cache", "delete");
        unimplemented!("delete")
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureGetResult for &mut DurableWorkerCtx<Ctx> {
    async fn future_get_result_get(
        &mut self,
        self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Option<Result<Option<Resource<IncomingValue>>, Resource<Error>>>> {
        (*self).future_get_result_get(self_).await
    }

    async fn listen_to_future_get_result(
        &mut self,
        self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        (*self).listen_to_future_get_result(self_).await
    }

    fn drop(&mut self, rep: Resource<FutureGetResult>) -> anyhow::Result<()> {
        HostFutureGetResult::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureExistsResult for &mut DurableWorkerCtx<Ctx> {
    async fn future_exists_result_get(
        &mut self,
        self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Option<Result<bool, Resource<Error>>>> {
        (*self).future_exists_result_get(self_).await
    }

    async fn listen_to_future_exists_result(
        &mut self,
        self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        (*self).listen_to_future_exists_result(self_).await
    }

    fn drop(&mut self, rep: Resource<FutureExistsResult>) -> anyhow::Result<()> {
        HostFutureExistsResult::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureResult for &mut DurableWorkerCtx<Ctx> {
    async fn future_result_get(
        &mut self,
        self_: Resource<FutureResult>,
    ) -> anyhow::Result<Option<Result<(), Resource<Error>>>> {
        (*self).future_result_get(self_).await
    }

    async fn listen_to_future_result(
        &mut self,
        self_: Resource<FutureResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        (*self).listen_to_future_result(self_).await
    }

    fn drop(&mut self, rep: Resource<FutureResult>) -> anyhow::Result<()> {
        HostFutureResult::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureGetOrSetResult for &mut DurableWorkerCtx<Ctx> {
    async fn future_get_or_set_result_get(
        &mut self,
        _self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Option<Result<GetOrSetEntry, Resource<Error>>>> {
        (*self).future_get_or_set_result_get(_self_).await
    }

    async fn listen_to_future_get_or_set_result(
        &mut self,
        _self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        (*self).listen_to_future_get_or_set_result(_self_).await
    }

    fn drop(&mut self, rep: Resource<FutureGetOrSetResult>) -> anyhow::Result<()> {
        HostFutureGetOrSetResult::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostVacancy for &mut DurableWorkerCtx<Ctx> {
    async fn vacancy_fill(
        &mut self,
        _self_: Resource<Vacancy>,
        _ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<OutgoingValue>> {
        (*self).vacancy_fill(_self_, _ttl_ms).await
    }

    fn drop(&mut self, rep: Resource<Vacancy>) -> anyhow::Result<()> {
        HostVacancy::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {
    async fn get(&mut self, k: Key) -> anyhow::Result<Resource<FutureGetResult>> {
        (*self).get(k).await
    }

    async fn exists(&mut self, k: Key) -> anyhow::Result<Resource<FutureExistsResult>> {
        (*self).exists(k).await
    }

    async fn set(
        &mut self,
        k: Key,
        v: Resource<OutgoingValue>,
        ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<FutureResult>> {
        (*self).set(k, v, ttl_ms).await
    }

    async fn get_or_set(&mut self, k: Key) -> anyhow::Result<Resource<FutureGetOrSetResult>> {
        (*self).get_or_set(k).await
    }

    async fn delete(&mut self, k: Key) -> anyhow::Result<Resource<FutureResult>> {
        (*self).delete(k).await
    }
}
