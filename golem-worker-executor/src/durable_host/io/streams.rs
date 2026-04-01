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

use wasmtime::component::Resource;
use wasmtime_wasi::StreamError;

use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::http::{continue_http_request, end_http_request};
use crate::durable_host::io::{ManagedStdErr, ManagedStdOut};
use crate::durable_host::{
    Durability, DurabilityHost, DurableWorkerCtx, HttpOutputStreamState, HttpRequestCloseOwner,
    PendingFilesystemReservation,
};
use crate::model::event::InternalWorkerEvent;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    HttpTypesIncomingBodyStreamBlockingRead, HttpTypesIncomingBodyStreamBlockingSkip,
    HttpTypesIncomingBodyStreamRead, HttpTypesIncomingBodyStreamSkip,
    HttpTypesOutgoingBodyStreamBlockingFlush, HttpTypesOutgoingBodyStreamBlockingSplice,
    HttpTypesOutgoingBodyStreamCheckWrite, HttpTypesOutgoingBodyStreamFlush,
    HttpTypesOutgoingBodyStreamSplice, HttpTypesOutgoingBodyStreamWrite,
    HttpTypesOutgoingBodyStreamWriteZeroes,
};
use golem_common::model::oplog::types::SerializableStreamError;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestHttpRequest, HostResponseStreamCheckWrite,
    HostResponseStreamChunk, HostResponseStreamSkip, HostResponseStreamWriteResult, OplogIndex,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime_wasi::filesystem::WasiFilesystemView as _;
use wasmtime_wasi::p2::bindings::filesystem::types::{
    Descriptor as FsDescriptor, HostDescriptor as FsHostDescriptor,
};
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
                    .try_trigger_retry(self, &ignore_closed_error(&result), |_| {
                        HostFailureKind::Transient
                    })
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
                    .try_trigger_retry(self, &ignore_closed_error(&result), |_| {
                        HostFailureKind::Transient
                    })
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
                    .try_trigger_retry(self, &ignore_closed_error(&result), |_| {
                        HostFailureKind::Transient
                    })
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
                    .try_trigger_retry(self, &ignore_closed_error(&result), |_| {
                        HostFailureKind::Transient
                    })
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
            if let Some(state) = self.state.open_http_requests.get(&handle)
                && state.close_owner == HttpRequestCloseOwner::InputStreamClosed
            {
                end_http_request(self, handle).await?;
            }
        }

        HostInputStream::drop(self.table(), rep).await
    }
}

impl<Ctx: WorkerCtx> HostOutputStream for DurableWorkerCtx<Ctx> {
    async fn check_write(&mut self, self_: Resource<OutputStream>) -> Result<u64, StreamError> {
        let rep = self_.rep();
        if is_outgoing_http_body_stream(self, rep) {
            let state = get_http_output_stream_state(self, rep)?;
            let durability = Durability::<HttpTypesOutgoingBodyStreamCheckWrite>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(state.begin_index)),
            )
            .await
            .map_err(StreamError::from)?;

            let result = if durability.is_live() {
                let result = HostOutputStream::check_write(self.table(), self_).await;
                durability
                    .persist(
                        self,
                        state.request,
                        HostResponseStreamCheckWrite {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
            .map_err(StreamError::from)?;

            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::output_stream", "check_write");
            let stream_rep = self_.rep();
            let result = HostOutputStream::check_write(self.table(), self_).await;
            if let Ok(permit) = result.as_ref() {
                if *permit > 0 {
                    reconcile_pending_filesystem_stream_reservation(self, stream_rep).await;
                }
            } else {
                reconcile_pending_filesystem_stream_reservation(self, stream_rep).await;
            }
            result
        }
    }

    async fn write(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        let rep = self_.rep();

        if is_outgoing_http_body_stream(self, rep) {
            let state = get_http_output_stream_state(self, rep)?;
            let durability = Durability::<HttpTypesOutgoingBodyStreamWrite>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(state.begin_index)),
            )
            .await
            .map_err(StreamError::from)?;

            let result = if durability.is_live() {
                let result = HostOutputStream::write(self.table(), self_, contents).await;
                durability
                    .persist(
                        self,
                        state.request,
                        HostResponseStreamWriteResult {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
            .map_err(StreamError::from)?;

            result.result.map_err(StreamError::from)
        } else {
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
                let stream_rep = self_.rep();
                let write_len = contents.len() as u64;
                reserve_filesystem_stream_growth(self, stream_rep, write_len).await?;

                let result = HostOutputStream::write(self.table(), self_, contents).await;
                match result {
                    Ok(()) => {
                        mark_filesystem_stream_write_enqueued(self, stream_rep, write_len);
                        Ok(())
                    }
                    Err(err) => {
                        rollback_pending_filesystem_stream_reservation(self, stream_rep).await;
                        Err(err)
                    }
                }
            }
        }
    }

    async fn blocking_write_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        // This is already composed from write + blocking_flush, both of which
        // are individually made durable, so no additional oplog entry needed.
        let self2 = Resource::new_borrow(self_.rep());
        HostOutputStream::write(self, self_, contents).await?;
        self.blocking_flush(self2).await?;
        Ok(())
    }

    async fn flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        let rep = self_.rep();
        if is_outgoing_http_body_stream(self, rep) {
            let state = get_http_output_stream_state(self, rep)?;
            let durability = Durability::<HttpTypesOutgoingBodyStreamFlush>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(state.begin_index)),
            )
            .await
            .map_err(StreamError::from)?;

            let result = if durability.is_live() {
                let result = HostOutputStream::flush(self.table(), self_).await;
                durability
                    .persist(
                        self,
                        state.request,
                        HostResponseStreamWriteResult {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
            .map_err(StreamError::from)?;

            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::output_stream", "flush");
            HostOutputStream::flush(self.table(), self_).await
        }
    }

    async fn blocking_flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        let rep = self_.rep();
        if is_outgoing_http_body_stream(self, rep) {
            let state = get_http_output_stream_state(self, rep)?;
            let durability = Durability::<HttpTypesOutgoingBodyStreamBlockingFlush>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(state.begin_index)),
            )
            .await
            .map_err(StreamError::from)?;

            let result = if durability.is_live() {
                let result = HostOutputStream::blocking_flush(self.table(), self_).await;
                durability
                    .persist(
                        self,
                        state.request,
                        HostResponseStreamWriteResult {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
            .map_err(StreamError::from)?;

            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::output_stream", "blocking_flush");
            HostOutputStream::blocking_flush(self.table(), self_).await
        }
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
        let rep = self_.rep();
        if is_outgoing_http_body_stream(self, rep) {
            let state = get_http_output_stream_state(self, rep)?;
            let durability = Durability::<HttpTypesOutgoingBodyStreamWriteZeroes>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(state.begin_index)),
            )
            .await
            .map_err(StreamError::from)?;

            let result = if durability.is_live() {
                let result = HostOutputStream::write_zeroes(self.table(), self_, len).await;
                durability
                    .persist(
                        self,
                        state.request,
                        HostResponseStreamWriteResult {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
            .map_err(StreamError::from)?;

            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::output_stream", "write_zeroes");

            // Exclude console streams from quota — only file-backed streams consume storage.
            let is_console = {
                let output = self.table().get(&self_)?;
                output.as_any().downcast_ref::<ManagedStdOut>().is_some()
                    || output.as_any().downcast_ref::<ManagedStdErr>().is_some()
            };

            if !is_console {
                let stream_rep = self_.rep();
                reserve_filesystem_stream_growth(self, stream_rep, len).await?;

                let result = HostOutputStream::write_zeroes(self.table(), self_, len).await;
                return match result {
                    Ok(()) => {
                        mark_filesystem_stream_write_enqueued(self, stream_rep, len);
                        Ok(())
                    }
                    Err(err) => {
                        rollback_pending_filesystem_stream_reservation(self, stream_rep).await;
                        Err(err)
                    }
                };
            }

            HostOutputStream::write_zeroes(self.table(), self_, len).await
        }
    }

    async fn blocking_write_zeroes_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        // Composed from write_zeroes + blocking_flush, both of which handle
        // quota enforcement individually — mirrors blocking_write_and_flush.
        let self2 = Resource::new_borrow(self_.rep());
        self.write_zeroes(self_, len).await?;
        self.blocking_flush(self2).await?;
        Ok(())
    }

    async fn splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        let rep = self_.rep();
        if is_outgoing_http_body_stream(self, rep) {
            let state = get_http_output_stream_state(self, rep)?;
            let durability = Durability::<HttpTypesOutgoingBodyStreamSplice>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(state.begin_index)),
            )
            .await
            .map_err(StreamError::from)?;

            let result = if durability.is_live() {
                let result = HostOutputStream::splice(self.table(), self_, src, len).await;
                durability
                    .persist(
                        self,
                        state.request,
                        HostResponseStreamSkip {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
            .map_err(StreamError::from)?;

            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::output_stream", "splice");

            let stream_rep = self_.rep();
            reserve_filesystem_stream_growth(self, stream_rep, len).await?;

            let result = HostOutputStream::splice(self.table(), self_, src, len).await;
            match &result {
                Ok(spliced) => {
                    mark_filesystem_stream_write_enqueued(self, stream_rep, *spliced);
                    reconcile_pending_filesystem_stream_reservation(self, stream_rep).await;
                }
                Err(_) => {
                    rollback_pending_filesystem_stream_reservation(self, stream_rep).await;
                }
            }
            result
        }
    }

    async fn blocking_splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        let rep = self_.rep();
        if is_outgoing_http_body_stream(self, rep) {
            let state = get_http_output_stream_state(self, rep)?;
            let durability = Durability::<HttpTypesOutgoingBodyStreamBlockingSplice>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(state.begin_index)),
            )
            .await
            .map_err(StreamError::from)?;

            let result = if durability.is_live() {
                let result = HostOutputStream::blocking_splice(self.table(), self_, src, len).await;
                durability
                    .persist(
                        self,
                        state.request,
                        HostResponseStreamSkip {
                            result: result.map_err(SerializableStreamError::from),
                        },
                    )
                    .await
            } else {
                durability.replay(self).await
            }
            .map_err(StreamError::from)?;

            result.result.map_err(StreamError::from)
        } else {
            self.observe_function_call("io::streams::output_stream", "blocking_splice");

            let stream_rep = self_.rep();
            reserve_filesystem_stream_growth(self, stream_rep, len).await?;

            let result = HostOutputStream::blocking_splice(self.table(), self_, src, len).await;
            match &result {
                Ok(spliced) => {
                    mark_filesystem_stream_write_enqueued(self, stream_rep, *spliced);
                    reconcile_pending_filesystem_stream_reservation(self, stream_rep).await;
                }
                Err(_) => {
                    rollback_pending_filesystem_stream_reservation(self, stream_rep).await;
                }
            }
            result
        }
    }

    async fn drop(&mut self, rep: Resource<OutputStream>) -> wasmtime::Result<()> {
        let handle = rep.rep();
        self.observe_function_call("io::streams::output_stream", "drop");
        if let Some(request_handle) = self.state.find_request_handle_by_output_stream(handle)
            && let Some(state) = self.state.open_http_requests.get_mut(&request_handle)
        {
            state.output_stream_rep = None;
        }
        let result = HostOutputStream::drop(self.table(), rep).await;
        reconcile_pending_filesystem_stream_reservation(self, handle).await;
        self.state.open_filesystem_output_streams.remove(&handle);
        result
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

fn is_outgoing_http_body_stream<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    stream_rep: u32,
) -> bool {
    ctx.state
        .find_request_handle_by_output_stream(stream_rep)
        .is_some()
}

fn get_http_output_stream_state<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    stream_rep: u32,
) -> Result<HttpOutputStreamState, StreamError> {
    ctx.state
        .find_request_handle_by_output_stream(stream_rep)
        .and_then(|handle| {
            ctx.state
                .open_http_requests
                .get(&handle)
                .map(|state| HttpOutputStreamState {
                    begin_index: state.begin_index,
                    request: state.request.clone(),
                })
        })
        .ok_or_else(|| {
            StreamError::Trap(wasmtime::Error::msg(
                "No matching HTTP output stream state for resource handle",
            ))
        })
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
    if matches!(result, Err(SerializableStreamError::Closed))
        && let Some(state) = ctx.state.open_http_requests.get(&handle)
        && state.close_owner == HttpRequestCloseOwner::InputStreamClosed
    {
        // If the stream has a recorded body handle, transfer tracking back
        // to the body instead of ending the request. This allows
        // IncomingBody::finish() to later transfer tracking to FutureTrailers,
        // making FutureTrailers::get() durable.
        let body_handle = state.body_handle;
        if let Some(body_handle) = body_handle {
            continue_http_request(
                ctx,
                handle,
                body_handle,
                HttpRequestCloseOwner::IncomingBodyDropOrFinish,
            );
        } else {
            end_http_request(ctx, handle).await?;
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

async fn reserve_filesystem_stream_growth<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    stream_rep: u32,
    write_len: u64,
) -> Result<(), StreamError> {
    let Some(stream_state) = ctx.state.open_filesystem_output_streams.get(&stream_rep) else {
        if write_len > 0 {
            ctx.reserve_filesystem_storage(write_len)
                .await
                .map_err(|e| StreamError::Trap(wasmtime::Error::from_anyhow(e)))?;
        }
        return Ok(());
    };

    if stream_state.pending_reservation.is_some() {
        return Ok(());
    }

    let stream_state = stream_state.clone();

    let current_size = {
        let fd_borrow = Resource::<FsDescriptor>::new_borrow(stream_state.descriptor_rep);
        let mut view = ctx.as_wasi_view();
        match FsHostDescriptor::stat(&mut view.filesystem(), fd_borrow).await {
            Ok(stat) => stat.size,
            Err(_) => {
                if write_len > 0 {
                    ctx.reserve_filesystem_storage(write_len)
                        .await
                        .map_err(|e| StreamError::Trap(wasmtime::Error::from_anyhow(e)))?;
                }
                return Ok(());
            }
        }
    };

    let requested_end = match stream_state.position {
        Some(position) => position.saturating_add(write_len),
        None => current_size.saturating_add(write_len),
    };

    let requested_growth = requested_end.saturating_sub(current_size);

    if requested_growth > 0 {
        ctx.reserve_filesystem_storage(requested_growth)
            .await
            .map_err(|e| StreamError::Trap(wasmtime::Error::from_anyhow(e)))?;
    }

    if let Some(state) = ctx
        .state
        .open_filesystem_output_streams
        .get_mut(&stream_rep)
    {
        state.pending_reservation = Some(PendingFilesystemReservation {
            base_size: current_size,
            reserved_growth: requested_growth,
        });
    }

    Ok(())
}

fn mark_filesystem_stream_write_enqueued<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    stream_rep: u32,
    write_len: u64,
) {
    if let Some(state) = ctx
        .state
        .open_filesystem_output_streams
        .get_mut(&stream_rep)
        && let Some(position) = &mut state.position
    {
        *position = position.saturating_add(write_len);
    }
}

async fn rollback_pending_filesystem_stream_reservation<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    stream_rep: u32,
) {
    let reserved_growth = ctx
        .state
        .open_filesystem_output_streams
        .get_mut(&stream_rep)
        .and_then(|state| state.pending_reservation.take())
        .map(|pending| pending.reserved_growth)
        .unwrap_or(0);

    if reserved_growth > 0 {
        ctx.release_filesystem_storage_space(reserved_growth).await;
    }
}

async fn reconcile_pending_filesystem_stream_reservation<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    stream_rep: u32,
) {
    let Some((descriptor_rep, pending)) = ctx
        .state
        .open_filesystem_output_streams
        .get_mut(&stream_rep)
        .and_then(|state| {
            state
                .pending_reservation
                .take()
                .map(|pending| (state.descriptor_rep, pending))
        })
    else {
        return;
    };

    if pending.reserved_growth == 0 {
        return;
    }

    let actual_growth = {
        let fd_borrow = Resource::<FsDescriptor>::new_borrow(descriptor_rep);
        let mut view = ctx.as_wasi_view();
        match FsHostDescriptor::stat(&mut view.filesystem(), fd_borrow).await {
            Ok(stat) => stat.size.saturating_sub(pending.base_size),
            Err(_) => pending.reserved_growth,
        }
    };

    let over_reserved = pending.reserved_growth.saturating_sub(actual_growth);
    if over_reserved > 0 {
        ctx.release_filesystem_storage_space(over_reserved).await;
    }
}
