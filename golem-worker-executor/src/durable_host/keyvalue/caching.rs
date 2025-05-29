// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use wasmtime::component::Resource;

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::cache::{
    Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry,
    Host, HostFutureExistsResult, HostFutureGetOrSetResult, HostFutureGetResult, HostFutureResult,
    HostVacancy, IncomingValue, Key, OutgoingValue, Pollable, Vacancy,
};
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> HostFutureGetResult for DurableWorkerCtx<Ctx> {
    async fn future_get_result_get(
        &mut self,
        _self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Option<Result<Option<Resource<IncomingValue>>, Resource<Error>>>> {
        self.observe_function_call("keyvalue::cache::future_get", "future_get_result_get");
        unimplemented!("future_get_result_get")
    }

    async fn listen_to_future_get_result(
        &mut self,
        _self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call("keyvalue::cache::future_get", "listen_to_future_get_result");
        unimplemented!("listen_to_future_get_result")
    }

    async fn drop(&mut self, _rep: Resource<FutureGetResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_get", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostFutureExistsResult for DurableWorkerCtx<Ctx> {
    async fn future_exists_result_get(
        &mut self,
        _self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Option<Result<bool, Resource<Error>>>> {
        self.observe_function_call("keyvalue::cache::future_exists", "future_exists_result_get");
        unimplemented!("future_exists_result_get")
    }

    async fn listen_to_future_exists_result(
        &mut self,
        _self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call(
            "keyvalue::cache::future_exists",
            "listen_to_future_exists_result",
        );
        unimplemented!("listen_to_future_exists_result")
    }

    async fn drop(&mut self, _rep: Resource<FutureExistsResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_exists", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostFutureResult for DurableWorkerCtx<Ctx> {
    async fn future_result_get(
        &mut self,
        _self_: Resource<FutureResult>,
    ) -> anyhow::Result<Option<Result<(), Resource<Error>>>> {
        self.observe_function_call("keyvalue::cache::future_result", "future_result_get");
        unimplemented!("future_result_get")
    }

    async fn listen_to_future_result(
        &mut self,
        _self_: Resource<FutureResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call("keyvalue::cache::future_result", "listen_to_future_result");
        unimplemented!("listen_to_future_result")
    }

    async fn drop(&mut self, _rep: Resource<FutureResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_result", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostFutureGetOrSetResult for DurableWorkerCtx<Ctx> {
    async fn future_get_or_set_result_get(
        &mut self,
        _self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Option<Result<GetOrSetEntry, Resource<Error>>>> {
        self.observe_function_call(
            "keyvalue::cache::future_get_or_set",
            "future_get_or_set_result_get",
        );
        unimplemented!("future_get_or_set_result_get")
    }

    async fn listen_to_future_get_or_set_result(
        &mut self,
        _self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Resource<Pollable>> {
        self.observe_function_call(
            "keyvalue::cache::future_get_or_set",
            "listen_to_future_get_or_set_result",
        );
        unimplemented!("listen_to_future_get_or_set_result")
    }

    async fn drop(&mut self, _rep: Resource<FutureGetOrSetResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_get_or_set", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostVacancy for DurableWorkerCtx<Ctx> {
    async fn vacancy_fill(
        &mut self,
        _self_: Resource<Vacancy>,
        _ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<OutgoingValue>> {
        self.observe_function_call("keyvalue::cache::vacancy", "vacancy_fill");
        unimplemented!("vacancy_fill")
    }

    async fn drop(&mut self, _rep: Resource<Vacancy>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::vacancy", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get(&mut self, _k: Key) -> anyhow::Result<Resource<FutureGetResult>> {
        self.observe_function_call("keyvalue::cache", "get");
        unimplemented!("get")
    }

    async fn exists(&mut self, _k: Key) -> anyhow::Result<Resource<FutureExistsResult>> {
        self.observe_function_call("keyvalue::cache", "exists");
        unimplemented!("exists")
    }

    async fn set(
        &mut self,
        _k: Key,
        _v: Resource<OutgoingValue>,
        _ttl_ms: Option<u32>,
    ) -> anyhow::Result<Resource<FutureResult>> {
        self.observe_function_call("keyvalue::cache", "set");
        unimplemented!("set")
    }

    async fn get_or_set(&mut self, _k: Key) -> anyhow::Result<Resource<FutureGetOrSetResult>> {
        self.observe_function_call("keyvalue::cache", "get_or_set");
        unimplemented!("get_or_set")
    }

    async fn delete(&mut self, _k: Key) -> anyhow::Result<Resource<FutureResult>> {
        self.observe_function_call("keyvalue::cache", "delete");
        unimplemented!("delete")
    }
}
