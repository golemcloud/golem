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
use crate::durable_host::concurrent::{CallHandle, NotCancellable};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::types::SerializableDateTime;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostResponseWallClock, host_functions,
};
use wasmtime_wasi::clocks::WasiClocksView as _;
use wasmtime_wasi::p2::bindings::clocks::wall_clock::{Datetime, Host};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> wasmtime::Result<Datetime> {
        // Re-executable `ReadLocal`: the `CallHandle::run` combinator reads the clock on the live /
        // incomplete-replay paths and replays a recorded value otherwise.
        let handle = CallHandle::<host_functions::WallClockNow, NotCancellable>::start(
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
                    time: SerializableDateTime {
                        seconds: result.seconds as i64,
                        nanoseconds: result.nanoseconds,
                    },
                })
            })
            .await?;

        Ok(result.time.into())
    }

    async fn resolution(&mut self) -> wasmtime::Result<Datetime> {
        let handle = CallHandle::<host_functions::WallClockResolution, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = handle
            .run(self, async |ctx| -> wasmtime::Result<_> {
                let result = {
                    let mut view = ctx.as_wasi_view();
                    Host::resolution(&mut view.clocks()).await?
                };
                Ok(HostResponseWallClock {
                    time: SerializableDateTime {
                        seconds: result.seconds as i64,
                        nanoseconds: result.nanoseconds,
                    },
                })
            })
            .await?;

        Ok(result.time.into())
    }
}
