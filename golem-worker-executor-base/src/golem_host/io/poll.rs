use async_trait::async_trait;
use chrono::{Duration, Utc};
use crate::model::InterruptKind;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::bindings::wasi::io::poll::{Host, HostPollable, Pollable};

use crate::golem_host::{GolemCtx, SuspendForSleep};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostPollable for GolemCtx<Ctx> {
    async fn ready(&mut self, self_: Resource<Pollable>) -> anyhow::Result<bool> {
        record_host_function_call("io::poll:pollable", "ready");
        HostPollable::ready(&mut self.as_wasi_view(), self_).await
    }

    async fn block(&mut self, self_: Resource<Pollable>) -> anyhow::Result<()> {
        record_host_function_call("io::poll:pollable", "block");
        let in_ = vec![self_];
        let _ = self.poll(in_).await?;
        Ok(())
    }

    fn drop(&mut self, rep: Resource<Pollable>) -> anyhow::Result<()> {
        record_host_function_call("io::poll:pollable", "drop");
        HostPollable::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    async fn poll(&mut self, in_: Vec<Resource<Pollable>>) -> anyhow::Result<Vec<u32>> {
        record_host_function_call("io::poll", "poll");
        let result = Host::poll(&mut self.as_wasi_view(), in_).await;

        match is_suspend_for_sleep(&result) {
            Some(duration) => {
                self.private_state
                    .sleep_until(Utc::now() + duration)
                    .await?;
                Err(InterruptKind::Suspend.into())
            }
            None => result,
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
