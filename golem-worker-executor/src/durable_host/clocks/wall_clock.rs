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

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::{DurableCallSession, NotCancellable};
use crate::preview2::p2_monotonic_clock::wasi::clocks0_2_6::wall_clock::{Datetime, HostWithStore};
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostResponseWallClock, host_functions,
};
use wasmtime::component::{Accessor, HasSelf};
use wasmtime_wasi::clocks::WasiClocksView as _;
use wasmtime_wasi::p2::bindings::clocks::wall_clock::{Datetime as WasiDatetime, Host};

impl From<golem_common::model::oplog::types::SerializableDateTime> for Datetime {
    fn from(value: golem_common::model::oplog::types::SerializableDateTime) -> Self {
        let value: WasiDatetime = value.into();
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) async fn durable_wall_clock_now(&mut self) -> wasmtime::Result<WasiDatetime> {
        #[cfg(feature = "test-utils")]
        if self.test_should_skip_wall_clock_now_durability() {
            let mut view = self.as_wasi_view();
            return Host::now(&mut view.clocks()).await;
        }
        #[cfg(feature = "test-utils")]
        self.owner_execution.test_before_wall_clock_now().await;

        // Re-executable `ReadLocal`: the `DurableCallSession::run` combinator reads the clock on the live /
        // incomplete-replay paths and replays a recorded value otherwise.
        let handle = DurableCallSession::<host_functions::WallClockNow, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let result = {
                    let mut view = ctx.as_wasi_view();
                    Host::now(&mut view.clocks()).await?
                };
                Ok(HostResponseWallClock {
                    time: result.into(),
                })
            })
            .await?;

        Ok(result.time.into())
    }
}

impl<Ctx: WorkerCtx> crate::preview2::p2_monotonic_clock::wasi::clocks0_2_6::wall_clock::Host
    for DurableWorkerCtx<Ctx>
{
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn now(accessor: &Accessor<U, Self>) -> anyhow::Result<Datetime> {
        #[cfg(feature = "test-utils")]
        {
            if accessor.with(|mut access| access.get().test_should_skip_wall_clock_now_durability())
            {
                return accessor.with(|mut access| {
                    let mut view = access.get().as_wasi_view();
                    let value = Host::now(&mut view.clocks())
                        .now_or_never()
                        .expect("the WASI wall clock must be immediately ready")?;
                    Ok(Datetime {
                        seconds: value.seconds,
                        nanoseconds: value.nanoseconds,
                    })
                });
            }
            let owner = accessor.with(|mut access| access.get().owner_execution.clone());
            owner.test_before_wall_clock_now().await;
        }

        let result =
            DurableCallSession::<host_functions::WallClockNow, NotCancellable>::invoke_access(
                accessor,
                accessor.getter(),
                HostRequestNoInput {},
                DurableFunctionType::ReadLocal,
                async || {
                    let time = accessor.with(|mut access| {
                        let mut view = access.get().as_wasi_view();
                        Host::now(&mut view.clocks())
                            .now_or_never()
                            .expect("the WASI wall clock must be immediately ready")
                    })?;
                    Ok::<_, anyhow::Error>(HostResponseWallClock { time: time.into() })
                },
            )
            .await?;
        Ok(result.time.into())
    }

    async fn resolution(accessor: &Accessor<U, Self>) -> anyhow::Result<Datetime> {
        let result = DurableCallSession::<host_functions::WallClockResolution, NotCancellable>::invoke_access(
            accessor,
            accessor.getter(),
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
            async || {
                let time = accessor.with(|mut access| {
                    let mut view = access.get().as_wasi_view();
                    Host::resolution(&mut view.clocks())
                        .now_or_never()
                        .expect("the WASI wall clock must be immediately ready")
                })?;
                Ok::<_, anyhow::Error>(HostResponseWallClock { time: time.into() })
            },
        ).await?;
        Ok(result.time.into())
    }
}
