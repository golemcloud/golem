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
use anyhow::anyhow;
use chrono::{Duration, Utc};
use futures::future::Either;
use futures::pin_mut;
use golem_common::model::Timestamp;
use golem_common::model::oplog::host_functions::{IoPollPoll, IoPollReady};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestPollCount, HostResponsePollReady,
    HostResponsePollResult,
};
use golem_service_base::error::worker_executor::InterruptKind;
use tracing::debug;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::io::poll::{Host, HostPollable, Pollable};

impl<Ctx: WorkerCtx> HostPollable for DurableWorkerCtx<Ctx> {
    async fn ready(&mut self, self_: Resource<Pollable>) -> anyhow::Result<bool> {
        self.observe_function_call("io::poll:pollable", "ready");
        let durability =
            Durability::<IoPollReady>::new(self, DurableFunctionType::ReadLocal).await?;

        let result = if durability.is_live() {
            let result = HostPollable::ready(&mut self.as_wasi_view().0, self_)
                .await
                .map_err(|err| err.to_string());
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

        result.result.map_err(|err| anyhow!(err))
    }

    async fn block(&mut self, self_: Resource<Pollable>) -> anyhow::Result<()> {
        self.observe_function_call("io::poll:pollable", "block");
        let in_ = vec![self_];
        let _ = self.poll(in_).await?;

        Ok(())
    }

    fn drop(&mut self, rep: Resource<Pollable>) -> anyhow::Result<()> {
        self.observe_function_call("io::poll:pollable", "drop");
        HostPollable::drop(&mut self.as_wasi_view().0, rep)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn poll(&mut self, in_: Vec<Resource<Pollable>>) -> anyhow::Result<Vec<u32>> {
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
                return Err(InterruptKind::Suspend(Timestamp::now_utc()).into());
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
                let view = &mut self.as_wasi_view().0;
                let poll = Host::poll(view, in_);
                pin_mut!(poll);

                let either_result = futures::future::select(poll, interrupt_signal).await;
                match either_result {
                    Either::Left((result, _)) => result,
                    Either::Right((interrupt_kind, _)) => {
                        tracing::info!("Interrupted while waiting for poll result");
                        return Err(interrupt_kind.into());
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
            Ok(result) => result.result.map_err(|err| anyhow!(err)),
            Err(duration) => {
                self.state.sleep_until(Utc::now() + duration).await?;
                Err(InterruptKind::Suspend(Timestamp::now_utc()).into())
            }
        }
    }
}

fn is_suspend_for_sleep<T>(result: &Result<T, anyhow::Error>) -> Option<Duration> {
    if let Err(err) = result {
        if let Some(SuspendForSleep(duration)) = err.root_cause().downcast_ref::<SuspendForSleep>()
        {
            Some(Duration::from_std(*duration).unwrap())
        } else {
            None
        }
    } else {
        None
    }
}
