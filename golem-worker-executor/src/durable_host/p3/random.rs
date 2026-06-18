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

use crate::durable_host::p3::DurableP3View;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::p3::bindings::random::{insecure, insecure_seed, random};
use wasmtime_wasi::random::WasiRandomView;

impl<Ctx: WorkerCtx> random::Host for DurableP3View<'_, Ctx> {
    fn get_random_bytes(&mut self, len: u64) -> wasmtime::Result<Vec<u8>> {
        random::Host::get_random_bytes(WasiRandomView::random(self.0), len)
    }

    fn get_random_u64(&mut self) -> wasmtime::Result<u64> {
        random::Host::get_random_u64(WasiRandomView::random(self.0))
    }
}

impl<Ctx: WorkerCtx> insecure::Host for DurableP3View<'_, Ctx> {
    fn get_insecure_random_bytes(&mut self, len: u64) -> wasmtime::Result<Vec<u8>> {
        insecure::Host::get_insecure_random_bytes(WasiRandomView::random(self.0), len)
    }

    fn get_insecure_random_u64(&mut self) -> wasmtime::Result<u64> {
        insecure::Host::get_insecure_random_u64(WasiRandomView::random(self.0))
    }
}

impl<Ctx: WorkerCtx> insecure_seed::Host for DurableP3View<'_, Ctx> {
    fn get_insecure_seed(&mut self) -> wasmtime::Result<(u64, u64)> {
        insecure_seed::Host::get_insecure_seed(WasiRandomView::random(self.0))
    }
}
