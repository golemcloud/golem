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

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::services::oplog::CommitLevel;
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{
    host_functions, DurableFunctionType, HostRequestMonotonicClockDuration, HostRequestNoInput,
    HostResponseMonotonicClockTimestamp,
};
use wasmtime_wasi::clocks::WasiClocksView as _;
use wasmtime_wasi::p2::bindings::clocks::monotonic_clock::{Duration, Host, Instant, Pollable};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn now(&mut self) -> wasmtime::Result<Instant> {
        let durability = Durability::<host_functions::MonotonicClockNow>::new(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = if durability.is_live() {
            let nanos = {
                let mut view = self.as_wasi_view();
                Host::now(&mut view.clocks()).await?
            };
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseMonotonicClockTimestamp { nanos },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.nanos)
    }

    async fn resolution(&mut self) -> wasmtime::Result<Instant> {
        let durability = Durability::<host_functions::MonotonicClockResolution>::new(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let result = if durability.is_live() {
            let nanos = {
                let mut view = self.as_wasi_view();
                Host::resolution(&mut view.clocks()).await?
            };
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseMonotonicClockTimestamp { nanos },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        Ok(result.nanos)
    }

    async fn subscribe_instant(&mut self, when: Instant) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("monotonic_clock", "subscribe_instant");
        let mut view = self.as_wasi_view();
        Host::subscribe_instant(&mut view.clocks(), when).await
    }

    async fn subscribe_duration(
        &mut self,
        duration_in_nanos: Duration,
    ) -> wasmtime::Result<Resource<Pollable>> {
        let durability = Durability::<host_functions::MonotonicClockSubscribeDuration>::new(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await?;

        let now = {
            if durability.is_live() {
                let nanos = {
                    let mut view = self.as_wasi_view();
                    Host::now(&mut view.clocks()).await?
                };
                durability
                    .persist(
                        self,
                        HostRequestMonotonicClockDuration { duration_in_nanos },
                        HostResponseMonotonicClockTimestamp { nanos },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
        }?;

        self.public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await;
        let when = now.nanos.saturating_add(duration_in_nanos);
        let mut view = self.as_wasi_view();
        Host::subscribe_instant(&mut view.clocks(), when).await
    }
}
