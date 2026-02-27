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
use wasmtime_wasi::StreamError;

use crate::durable_host::http::end_http_request;
use crate::durable_host::io::{ManagedStdErr, ManagedStdOut};
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, HttpRequestCloseOwner};
use crate::model::event::InternalWorkerEvent;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    HttpTypesIncomingBodyStreamBlockingRead, HttpTypesIncomingBodyStreamBlockingSkip,
    HttpTypesIncomingBodyStreamRead, HttpTypesIncomingBodyStreamSkip,
};
use golem_common::model::oplog::types::SerializableStreamError;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestHttpRequest, HostResponseStreamChunk, HostResponseStreamSkip,
    OplogIndex,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime_wasi::p2::bindings::io::streams::{
    Host, HostInputStream, HostOutputStream, InputStream, OutputStream, Pollable,
};
use wasmtime_wasi_http::body::{FailingStream, HostIncomingBodyStream};

impl<Ctx: WorkerCtx> HostInputStream for DurableWorkerCtx<Ctx> {
    async fn read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        let handle = self_.rep();
        if is_incoming_http_body_stream(self, &self_) {
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<HttpTypesIncomingBodyStreamRead>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;

            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result = HostInputStream::read(self.table(), self_, len).await;

                durability
                    .try_trigger_retry(self, &ignore_closed_error(&result))
                    .await
                    .map_err(|e| StreamError::Trap(wasmtime::Error::from_anyhow(e)))?;

                durability
                    .persist(
                        self,
                        request,
                        HostResponseStreamChunk {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }?;

            end_http_request_if_closed(self, handle, &result.result).await?;
            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::input_stream", "read");
            HostInputStream::read(self.table(), self_, len).await
        }
    }

    async fn blocking_read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        if is_incoming_http_body_stream(self, &self_) {
            let handle = self_.rep();
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<HttpTypesIncomingBodyStreamBlockingRead>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;
            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result = HostInputStream::blocking_read(self.table(), self_, len).await;

                durability
                    .try_trigger_retry(self, &ignore_closed_error(&result))
                    .await
                    .map_err(|e| StreamError::Trap(wasmtime::Error::from_anyhow(e)))?;
                durability
                    .persist(
                        self,
                        request,
                        HostResponseStreamChunk {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }?;

            end_http_request_if_closed(self, handle, &result.result).await?;
            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::input_stream", "blocking_read");
            HostInputStream::blocking_read(self.table(), self_, len).await
        }
    }

    async fn skip(&mut self, self_: Resource<InputStream>, len: u64) -> Result<u64, StreamError> {
        if is_incoming_http_body_stream(self, &self_) {
            let handle = self_.rep();
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<HttpTypesIncomingBodyStreamSkip>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;
            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result = HostInputStream::skip(self.table(), self_, len).await;
                durability
                    .try_trigger_retry(self, &ignore_closed_error(&result))
                    .await
                    .map_err(|e| StreamError::Trap(wasmtime::Error::from_anyhow(e)))?;
                durability
                    .persist(
                        self,
                        request,
                        HostResponseStreamSkip {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }?;

            end_http_request_if_closed(self, handle, &result.result).await?;
            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::input_stream", "skip");
            HostInputStream::skip(self.table(), self_, len).await
        }
    }

    async fn blocking_skip(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        if is_incoming_http_body_stream(self, &self_) {
            let handle = self_.rep();
            let begin_idx = get_http_request_begin_idx(self, handle)?;

            let durability = Durability::<HttpTypesIncomingBodyStreamBlockingSkip>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            )
            .await?;

            let result = if durability.is_live() {
                let request = get_http_stream_request(self, handle)?;
                let result = HostInputStream::blocking_skip(self.table(), self_, len).await;
                durability
                    .try_trigger_retry(self, &ignore_closed_error(&result))
                    .await
                    .map_err(|e| StreamError::Trap(wasmtime::Error::from_anyhow(e)))?;
                durability
                    .persist(
                        self,
                        request,
                        HostResponseStreamSkip {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }?;
            end_http_request_if_closed(self, handle, &result.result).await?;

            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::input_stream", "blocking_skip");
            HostInputStream::blocking_skip(self.table(), self_, len).await
        }
    }

    fn subscribe(&mut self, self_: Resource<InputStream>) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("io::streams::input_stream", "subscribe");
        HostInputStream::subscribe(self.table(), self_)
    }

    async fn drop(&mut self, rep: Resource<InputStream>) -> wasmtime::Result<()> {
        self.observe_function_call("io::streams::input_stream", "drop");

        if is_incoming_http_body_stream(self, &rep) {
            let handle = rep.rep();
            if let Some(state) = self.state.open_http_requests.get(&handle) {
                if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
                    end_http_request(self, handle).await?;
                }
            }
        }

        HostInputStream::drop(self.table(), rep).await
    }
}

impl<Ctx: WorkerCtx> HostOutputStream for DurableWorkerCtx<Ctx> {
    fn check_write(&mut self, self_: Resource<OutputStream>) -> Result<u64, StreamError> {
        self.observe_function_call("io::streams::output_stream", "check_write");
        HostOutputStream::check_write(self.table(), self_)
    }

    async fn write(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        self.observe_function_call("io::streams::output_stream", "write");

        let output = self.table().get(&self_)?;
        let event = if output.as_any().downcast_ref::<ManagedStdOut>().is_some() {
            Some(InternalWorkerEvent::stdout(contents.clone()))
        } else if output.as_any().downcast_ref::<ManagedStdErr>().is_some() {
            Some(InternalWorkerEvent::stderr(contents.clone()))
        } else {
            None
        };

        if let Some(event) = event {
            self.emit_log_event(event).await;
            Ok::<(), StreamError>(())
        } else {
            // Non-stdout writes are non-persistent and always executed
            HostOutputStream::write(self.table(), self_, contents).await
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
        self.observe_function_call("io::streams::output_stream", "flush");
        HostOutputStream::flush(self.table(), self_).await
    }

    async fn blocking_flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        self.observe_function_call("io::streams::output_stream", "blocking_flush");
        HostOutputStream::blocking_flush(self.table(), self_).await
    }

    fn subscribe(&mut self, self_: Resource<OutputStream>) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("io::streams::output_stream", "subscribe");
        HostOutputStream::subscribe(self.table(), self_)
    }

    async fn write_zeroes(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        self.observe_function_call("io::streams::output_stream", "write_zeroeas");
        HostOutputStream::write_zeroes(self.table(), self_, len).await
    }

    async fn blocking_write_zeroes_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        self.observe_function_call(
            "io::streams::output_stream",
            "blocking_write_zeroes_and_flush",
        );
        HostOutputStream::blocking_write_zeroes_and_flush(self.table(), self_, len).await
    }

    async fn splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        self.observe_function_call("io::streams::output_stream", "splice");
        HostOutputStream::splice(self.table(), self_, src, len).await
    }

    async fn blocking_splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        self.observe_function_call("io::streams::output_stream", "blocking_splice");
        HostOutputStream::blocking_splice(self.table(), self_, src, len).await
    }

    async fn drop(&mut self, rep: Resource<OutputStream>) -> wasmtime::Result<()> {
        self.observe_function_call("io::streams::output_stream", "drop");
        HostOutputStream::drop(self.table(), rep).await
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn convert_stream_error(
        &mut self,
        err: StreamError,
    ) -> wasmtime::Result<wasmtime_wasi::p2::bindings::io::streams::StreamError> {
        Host::convert_stream_error(self.table(), err)
    }
}

fn is_incoming_http_body_stream<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    stream: &Resource<InputStream>,
) -> bool {
    // incoming-body is used for both incoming http bodies (which don't need durability),
    // and response bodies. Only in the second case will there be an associated open http request.
    if !ctx.state.open_http_requests.contains_key(&stream.rep()) {
        return false;
    };

    let stream = ctx.table().get::<InputStream>(stream).unwrap();
    stream
        .as_any()
        .downcast_ref::<HostIncomingBodyStream>()
        .is_some()
        || stream.as_any().downcast_ref::<FailingStream>().is_some()
}

async fn end_http_request_if_closed<Ctx: WorkerCtx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
    result: &Result<T, SerializableStreamError>,
) -> Result<(), WorkerExecutorError> {
    if matches!(result, Err(SerializableStreamError::Closed)) {
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
        StreamError::Trap(wasmtime::Error::msg(
            "No matching HTTP request is associated with resource handle",
        ))
    })?;
    Ok(request_state.begin_index)
}

fn get_http_stream_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> Result<HostRequestHttpRequest, StreamError> {
    let request_state = ctx.state.open_http_requests.get(&handle).ok_or_else(|| {
        StreamError::Trap(wasmtime::Error::msg(
            "No matching HTTP request is associated with resource handle",
        ))
    })?;
    Ok(request_state.request.clone())
}

fn ignore_closed_error<T>(result: &Result<T, StreamError>) -> Result<(), &StreamError> {
    if let Err(StreamError::Closed) = result {
        Ok(())
    } else if let Err(err) = result {
        Err(err)
    } else {
        Ok(())
    }
}
