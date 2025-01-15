// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::anyhow;
use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::StreamError;

use crate::durable_host::io::{ManagedStdErr, ManagedStdOut};
use crate::durable_host::DurableWorkerCtx;
use crate::error::GolemError;
use crate::workerctx::WorkerCtx;
use golem_common::model::WorkerEvent;
use wasmtime_wasi::bindings::io::streams::{
    Host, HostInputStream, HostOutputStream, InputStream, OutputStream, Pollable,
};

#[async_trait]
impl<Ctx: WorkerCtx> HostInputStream for DurableWorkerCtx<Ctx> {
    async fn read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        HostInputStream::read(&mut self.as_wasi_view(), self_, len).await
    }

    async fn blocking_read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        HostInputStream::blocking_read(&mut self.as_wasi_view(), self_, len).await
    }

    async fn skip(&mut self, self_: Resource<InputStream>, len: u64) -> Result<u64, StreamError> {
        HostInputStream::skip(&mut self.as_wasi_view(), self_, len).await
    }

    async fn blocking_skip(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        HostInputStream::blocking_skip(&mut self.as_wasi_view(), self_, len).await
    }

    fn subscribe(&mut self, self_: Resource<InputStream>) -> anyhow::Result<Resource<Pollable>> {
        HostInputStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    async fn drop(&mut self, rep: Resource<InputStream>) -> anyhow::Result<()> {
        HostInputStream::drop(&mut self.as_wasi_view(), rep).await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostOutputStream for DurableWorkerCtx<Ctx> {
    fn check_write(&mut self, self_: Resource<OutputStream>) -> Result<u64, StreamError> {
        HostOutputStream::check_write(&mut self.as_wasi_view(), self_)
    }

    async fn write(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        let output = self.table().get(&self_)?;
        let event = if output.as_any().downcast_ref::<ManagedStdOut>().is_some() {
            Some(WorkerEvent::stdout(contents.clone()))
        } else if output.as_any().downcast_ref::<ManagedStdErr>().is_some() {
            Some(WorkerEvent::stderr(contents.clone()))
        } else {
            None
        };

        if let Some(event) = event {
            self.emit_log_event(event).await;
            Ok::<(), StreamError>(())
        } else {
            // Non-stdout writes are non-persistent and always executed
            HostOutputStream::write(&mut self.as_wasi_view(), self_, contents).await
        }
    }

    async fn blocking_write_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        let self2 = Resource::new_borrow(self_.rep());
        self.write(self_, contents).await?;
        self.blocking_flush(self2).await?;
        Ok(())
    }

    async fn flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        HostOutputStream::flush(&mut self.as_wasi_view(), self_).await
    }

    async fn blocking_flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        HostOutputStream::blocking_flush(&mut self.as_wasi_view(), self_).await
    }

    fn subscribe(&mut self, self_: Resource<OutputStream>) -> anyhow::Result<Resource<Pollable>> {
        HostOutputStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    async fn write_zeroes(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        HostOutputStream::write_zeroes(&mut self.as_wasi_view(), self_, len).await
    }

    async fn blocking_write_zeroes_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        HostOutputStream::blocking_write_zeroes_and_flush(&mut self.as_wasi_view(), self_, len)
            .await
    }

    async fn splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        HostOutputStream::splice(&mut self.as_wasi_view(), self_, src, len).await
    }

    async fn blocking_splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        HostOutputStream::blocking_splice(&mut self.as_wasi_view(), self_, src, len).await
    }

    async fn drop(&mut self, rep: Resource<OutputStream>) -> anyhow::Result<()> {
        HostOutputStream::drop(&mut self.as_wasi_view(), rep).await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn convert_stream_error(
        &mut self,
        err: StreamError,
    ) -> anyhow::Result<wasmtime_wasi::bindings::io::streams::StreamError> {
        Host::convert_stream_error(&mut self.as_wasi_view(), err)
    }
}

impl From<GolemError> for StreamError {
    fn from(value: GolemError) -> Self {
        StreamError::Trap(anyhow!(value))
    }
}
