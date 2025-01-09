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
use wasmtime_wasi::{ResourceTable, StreamError};

use crate::durable_host::http::end_http_request;
use crate::durable_host::http::serialized::SerializableHttpRequest;
use crate::durable_host::io::{ManagedStdErr, ManagedStdOut};
use crate::durable_host::serialized::SerializableStreamError;
use crate::durable_host::{Durability, DurableWorkerCtx, HttpRequestCloseOwner};
use crate::error::GolemError;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{OplogIndex, WrappedFunctionType};
use golem_common::model::WorkerEvent;
use wasmtime_wasi::bindings::io::streams::{
    Host, HostInputStream, HostOutputStream, InputStream, OutputStream, Pollable,
};
use wasmtime_wasi_http::body::{FailingStream, HostIncomingBodyStream};

#[async_trait]
impl<Ctx: WorkerCtx> HostInputStream for DurableWorkerCtx<Ctx> {
    async fn read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        if is_incoming_http_body_stream(self.table(), &self_) {
            let handle = self_.rep();
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<Ctx, Vec<u8>, SerializableStreamError>::new(
                self,
                "http::types::incoming_body_stream",
                "read",
                WrappedFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;

            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result = HostInputStream::read(&mut self.as_wasi_view(), self_, len).await;
                durability.persist(self, request, result).await
            } else {
                durability.replay(self).await
            };

            end_http_request_if_closed(self, handle, &result).await?;
            result
        } else {
            record_host_function_call("io::streams::input_stream", "read");
            HostInputStream::read(&mut self.as_wasi_view(), self_, len).await
        }
    }

    async fn blocking_read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        if is_incoming_http_body_stream(self.table(), &self_) {
            let handle = self_.rep();
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<Ctx, Vec<u8>, SerializableStreamError>::new(
                self,
                "http::types::incoming_body_stream",
                "blocking_read",
                WrappedFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;
            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result =
                    HostInputStream::blocking_read(&mut self.as_wasi_view(), self_, len).await;
                durability.persist(self, request, result).await
            } else {
                durability.replay(self).await
            };

            end_http_request_if_closed(self, handle, &result).await?;
            result
        } else {
            record_host_function_call("io::streams::input_stream", "blocking_read");
            HostInputStream::blocking_read(&mut self.as_wasi_view(), self_, len).await
        }
    }

    async fn skip(&mut self, self_: Resource<InputStream>, len: u64) -> Result<u64, StreamError> {
        if is_incoming_http_body_stream(self.table(), &self_) {
            let handle = self_.rep();
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<Ctx, u64, SerializableStreamError>::new(
                self,
                "http::types::incoming_body_stream",
                "skip",
                WrappedFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;
            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result = HostInputStream::skip(&mut self.as_wasi_view(), self_, len).await;
                durability.persist(self, request, result).await
            } else {
                durability.replay(self).await
            };

            end_http_request_if_closed(self, handle, &result).await?;
            result
        } else {
            record_host_function_call("io::streams::input_stream", "skip");
            HostInputStream::skip(&mut self.as_wasi_view(), self_, len).await
        }
    }

    async fn blocking_skip(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        if is_incoming_http_body_stream(self.table(), &self_) {
            let handle = self_.rep();
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<Ctx, u64, SerializableStreamError>::new(
                self,
                "http::types::incoming_body_stream",
                "blocking_skip",
                WrappedFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;

            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result =
                    HostInputStream::blocking_skip(&mut self.as_wasi_view(), self_, len).await;
                durability.persist(self, request, result).await
            } else {
                durability.replay(self).await
            };
            end_http_request_if_closed(self, handle, &result).await?;
            result
        } else {
            record_host_function_call("io::streams::input_stream", "blocking_skip");
            HostInputStream::blocking_skip(&mut self.as_wasi_view(), self_, len).await
        }
    }

    fn subscribe(&mut self, self_: Resource<InputStream>) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("io::streams::input_stream", "subscribe");
        HostInputStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    async fn drop(&mut self, rep: Resource<InputStream>) -> anyhow::Result<()> {
        record_host_function_call("io::streams::input_stream", "drop");

        if is_incoming_http_body_stream(self.table(), &rep) {
            let handle = rep.rep();
            if let Some(state) = self.state.open_http_requests.get(&handle) {
                if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
                    end_http_request(self, handle).await?;
                }
            }
        }

        HostInputStream::drop(&mut self.as_wasi_view(), rep).await
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostOutputStream for DurableWorkerCtx<Ctx> {
    fn check_write(&mut self, self_: Resource<OutputStream>) -> Result<u64, StreamError> {
        record_host_function_call("io::streams::output_stream", "check_write");
        HostOutputStream::check_write(&mut self.as_wasi_view(), self_)
    }

    async fn write(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "write");

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
        record_host_function_call("io::streams::output_stream", "flush");
        HostOutputStream::flush(&mut self.as_wasi_view(), self_).await
    }

    async fn blocking_flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "blocking_flush");
        HostOutputStream::blocking_flush(&mut self.as_wasi_view(), self_).await
    }

    fn subscribe(&mut self, self_: Resource<OutputStream>) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("io::streams::output_stream", "subscribe");
        HostOutputStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    async fn write_zeroes(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "write_zeroeas");
        HostOutputStream::write_zeroes(&mut self.as_wasi_view(), self_, len).await
    }

    async fn blocking_write_zeroes_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        record_host_function_call(
            "io::streams::output_stream",
            "blocking_write_zeroes_and_flush",
        );
        HostOutputStream::blocking_write_zeroes_and_flush(&mut self.as_wasi_view(), self_, len)
            .await
    }

    async fn splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        record_host_function_call("io::streams::output_stream", "splice");
        HostOutputStream::splice(&mut self.as_wasi_view(), self_, src, len).await
    }

    async fn blocking_splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        record_host_function_call("io::streams::output_stream", "blocking_splice");
        HostOutputStream::blocking_splice(&mut self.as_wasi_view(), self_, src, len).await
    }

    async fn drop(&mut self, rep: Resource<OutputStream>) -> anyhow::Result<()> {
        record_host_function_call("io::streams::output_stream", "drop");
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

fn is_incoming_http_body_stream(table: &ResourceTable, stream: &Resource<InputStream>) -> bool {
    let stream = table.get::<InputStream>(stream).unwrap();
    stream
        .as_any()
        .downcast_ref::<HostIncomingBodyStream>()
        .is_some()
        || stream.as_any().downcast_ref::<FailingStream>().is_some()
}

impl From<GolemError> for StreamError {
    fn from(value: GolemError) -> Self {
        StreamError::Trap(anyhow!(value))
    }
}

async fn end_http_request_if_closed<Ctx: WorkerCtx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
    result: &Result<T, StreamError>,
) -> Result<(), GolemError> {
    if matches!(result, Err(StreamError::Closed)) {
        if let Some(state) = ctx.state.open_http_requests.get(&handle) {
            if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
                end_http_request(ctx, handle).await?;
            }
        }
    }
    Ok(())
}

fn get_http_request_begin_idx<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> Result<OplogIndex, StreamError> {
    let request_state = ctx.state.open_http_requests.get(&handle).ok_or_else(|| {
        StreamError::Trap(anyhow!(
            "No matching HTTP request is associated with resource handle"
        ))
    })?;
    let begin_idx = *ctx
        .state
        .open_function_table
        .get(&request_state.root_handle)
        .ok_or_else(|| {
            StreamError::Trap(anyhow!(
                "No matching BeginRemoteWrite index was found for the open HTTP request"
            ))
        })?;
    Ok(begin_idx)
}

fn get_http_stream_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> Result<SerializableHttpRequest, StreamError> {
    let request_state = ctx.state.open_http_requests.get(&handle).ok_or_else(|| {
        StreamError::Trap(anyhow!(
            "No matching HTTP request is associated with resource handle"
        ))
    })?;
    Ok(request_state.request.clone())
}
