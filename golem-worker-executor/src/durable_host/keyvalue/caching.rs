// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use wasmtime::component::{Accessor, HasSelf, Resource};

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::cache::{
    Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry,
    Host, HostFutureExistsResult, HostFutureExistsResultWithStore, HostFutureGetOrSetResult,
    HostFutureGetOrSetResultWithStore, HostFutureGetResult, HostFutureGetResultWithStore,
    HostFutureResult, HostFutureResultWithStore, HostVacancy, IncomingValue, Key, OutgoingValue,
    Vacancy,
};
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> HostFutureGetResult for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, _rep: Resource<FutureGetResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_get", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostFutureGetResultWithStore for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get<T: Send>(
        _accessor: &Accessor<T, Self>,
        _self_: Resource<FutureGetResult>,
    ) -> anyhow::Result<Result<Option<Resource<IncomingValue>>, Resource<Error>>> {
        unimplemented!("future_get_result_get")
    }
}

impl<Ctx: WorkerCtx> HostFutureExistsResult for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, _rep: Resource<FutureExistsResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_exists", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostFutureExistsResultWithStore for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get<T: Send>(
        _accessor: &Accessor<T, Self>,
        _self_: Resource<FutureExistsResult>,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        unimplemented!("future_exists_result_get")
    }
}

impl<Ctx: WorkerCtx> HostFutureResult for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, _rep: Resource<FutureResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_result", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostFutureResultWithStore for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get<T: Send>(
        _accessor: &Accessor<T, Self>,
        _self_: Resource<FutureResult>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        unimplemented!("future_result_get")
    }
}

impl<Ctx: WorkerCtx> HostFutureGetOrSetResult for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, _rep: Resource<FutureGetOrSetResult>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::cache::future_get_or_set", "drop");
        unimplemented!("drop")
    }
}

impl<Ctx: WorkerCtx> HostFutureGetOrSetResultWithStore for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn get<T: Send>(
        _accessor: &Accessor<T, Self>,
        _self_: Resource<FutureGetOrSetResult>,
    ) -> anyhow::Result<Result<GetOrSetEntry, Resource<Error>>> {
        unimplemented!("future_get_or_set_result_get")
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
