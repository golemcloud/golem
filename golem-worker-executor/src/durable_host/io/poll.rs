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

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, SuspendForSleep};
use crate::workerctx::WorkerCtx;
use chrono::{Duration, Utc};
use futures::future::Either;
use futures::pin_mut;
use golem_common::model::oplog::host_functions::{IoPollPoll, IoPollReady};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestPollCount, HostResponsePollReady,
    HostResponsePollResult,
};
use golem_common::model::Timestamp;
use golem_service_base::error::worker_executor::InterruptKind;
use tracing::debug;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::io::poll::{Host, HostPollable, Pollable};
use wasmtime_wasi::IoView as _;

impl<Ctx: WorkerCtx> HostPollable for DurableWorkerCtx<Ctx> {
    async fn ready(&mut self, self_: Resource<Pollable>) -> wasmtime::Result<bool> {
        self.observe_function_call("io::poll:pollable", "ready");
        let durability =
            Durability::<IoPollReady>::new(self, DurableFunctionType::ReadLocal).await?;

        let result = if durability.is_live() {
            let result = {
                let mut view = self.as_wasi_view();
                HostPollable::ready(&mut view.io_data(), self_)
                    .await
                    .map_err(|err| err.to_string())
            };
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponsePollReady { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        result.result.map_err(|err| wasmtime::Error::msg(err))
    }

    async fn block(&mut self, self_: Resource<Pollable>) -> wasmtime::Result<()> {
        self.observe_function_call("io::poll:pollable", "block");
        let in_ = vec![self_];
        let _ = self.poll(in_).await?;

        Ok(())
    }

    fn drop(&mut self, rep: Resource<Pollable>) -> wasmtime::Result<()> {
        self.observe_function_call("io::poll:pollable", "drop");
        let mut view = self.as_wasi_view();
        HostPollable::drop(&mut view.io_data(), rep)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn poll(&mut self, in_: Vec<Resource<Pollable>>) -> wasmtime::Result<Vec<u32>> {
        // check if all pollables are promise backed. In this case we can suspend immediately
        // This check only needs to be done in live mode, as we will never even persist the oplog entry for polling
        // if we suspended in the last pass. Doing it this way also prevents us from initializing the promises until we are actually in live mode.
        if self.durable_execution_state().is_live {
            let promise_backed_pollables = self.state.promise_backed_pollables.read().await;
            let mut all_blocked = true;

            for res in &in_ {
                if let Some(promise_handle) = promise_backed_pollables.get(&res.rep()) {
                    let ready = promise_handle.get_handle().await.is_ready().await;
                    if ready {
                        all_blocked = false;
                        break;
                    }
                } else {
                    all_blocked = false;
                    break;
                }
            }

            if all_blocked {
                debug!("Suspending worker until a promise gets completed");
                return Err(wasmtime::Error::from_anyhow(InterruptKind::Suspend(Timestamp::now_utc()).into()));
            }
        };

        let durability =
            Durability::<IoPollPoll>::new(self, DurableFunctionType::ReadLocal).await?;

        let result: Result<HostResponsePollResult, Duration> = if durability.is_live() {
            let interrupt_signal = self
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();

            let count = in_.len();

            let result = {
                let mut view = self.as_wasi_view();
                let mut io_data = view.io_data();
                let poll = Host::poll(&mut io_data, in_);
                pin_mut!(poll);

                let either_result = futures::future::select(poll, interrupt_signal).await;
                match either_result {
                    Either::Left((result, _)) => result,
                    Either::Right((interrupt_kind, _)) => {
                        tracing::info!("Interrupted while waiting for poll result");
                        return Err(wasmtime::Error::from_anyhow(interrupt_kind.into()));
                    }
                }
            };

            match is_suspend_for_sleep(&result) {
                Some(duration) => Err(duration),
                None => Ok(durability
                    .persist(
                        self,
                        HostRequestPollCount { count },
                        HostResponsePollResult {
                            result: result.map_err(|err| err.to_string()),
                        },
                    )
                    .await?),
            }
        } else {
            Ok(durability.replay(self).await?)
        };

        match result {
            Ok(result) => result.result.map_err(|err| wasmtime::Error::msg(err)),
            Err(duration) => {
                self.state.sleep_until(Utc::now() + duration).await?;
                Err(wasmtime::Error::from_anyhow(InterruptKind::Suspend(Timestamp::now_utc()).into()))
            }
        }
    }
}

fn is_suspend_for_sleep<T>(result: &Result<T, wasmtime::Error>) -> Option<Duration> {
    if let Err(err) = result {
        // Walk the error source chain, since wasmtime::Error may wrap the original error
        let mut current: Option<&dyn std::error::Error> = Some(err.as_ref());
        while let Some(e) = current {
            if let Some(SuspendForSleep(duration)) = e.downcast_ref::<SuspendForSleep>() {
                return Some(Duration::from_std(*duration).unwrap());
            }
            current = e.source();
        }
        None
    } else {
        None
    }
}
