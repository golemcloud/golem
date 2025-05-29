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
use crate::preview2::wasi::keyvalue::atomic::{Bucket, Error, Host, Key};
use crate::workerctx::WorkerCtx;

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn increment(
        &mut self,
        _bucket: Resource<Bucket>,
        _key: Key,
        _delta: u64,
    ) -> anyhow::Result<Result<u64, Resource<Error>>> {
        self.observe_function_call("keyvalue::atomic", "increment");
        unimplemented!("increment")
    }

    async fn compare_and_swap(
        &mut self,
        _bucket: Resource<Bucket>,
        _key: Key,
        _old: u64,
        _new: u64,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        self.observe_function_call("keyvalue::atomic", "compare_and_swap");
        unimplemented!("compare_and_swap")
    }
}
