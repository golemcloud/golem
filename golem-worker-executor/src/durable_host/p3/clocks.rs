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

use crate::durable_host::p3::{DurableP3, DurableP3View, wasi_clocks_view};
use crate::workerctx::WorkerCtx;
use wasmtime::component::Accessor;
use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView};
use wasmtime_wasi::p3::bindings::clocks::{monotonic_clock, system_clock, types};

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> system_clock::Host for DurableP3View<'_, Ctx> {
    fn now(&mut self) -> wasmtime::Result<system_clock::Instant> {
        system_clock::Host::now(&mut WasiClocksView::clocks(self.0))
    }

    fn get_resolution(&mut self) -> wasmtime::Result<types::Duration> {
        system_clock::Host::get_resolution(&mut WasiClocksView::clocks(self.0))
    }
}

impl<Ctx: WorkerCtx> monotonic_clock::Host for DurableP3View<'_, Ctx> {
    fn now(&mut self) -> wasmtime::Result<monotonic_clock::Mark> {
        monotonic_clock::Host::now(&mut WasiClocksView::clocks(self.0))
    }

    fn get_resolution(&mut self) -> wasmtime::Result<types::Duration> {
        monotonic_clock::Host::get_resolution(&mut WasiClocksView::clocks(self.0))
    }
}

impl<Ctx: WorkerCtx> monotonic_clock::HostWithStore for DurableP3<Ctx> {
    async fn wait_until<U: Send>(
        store: &Accessor<U, Self>,
        when: monotonic_clock::Mark,
    ) -> wasmtime::Result<()> {
        let store = store.with_getter::<WasiClocks>(wasi_clocks_view::<Ctx, U>);
        <WasiClocks as monotonic_clock::HostWithStore>::wait_until(&store, when).await
    }

    async fn wait_for<U: Send>(
        store: &Accessor<U, Self>,
        how_long: types::Duration,
    ) -> wasmtime::Result<()> {
        let store = store.with_getter::<WasiClocks>(wasi_clocks_view::<Ctx, U>);
        <WasiClocks as monotonic_clock::HostWithStore>::wait_for(&store, how_long).await
    }
}
