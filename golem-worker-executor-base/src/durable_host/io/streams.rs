// Copyright 2024 Golem Cloud
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

use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::{ResourceTable, StreamError};

use crate::durable_host::io::{ManagedStdErr, ManagedStdOut};
use crate::durable_host::serialized::SerializableStreamError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::io::streams::{
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
        record_host_function_call("io::streams::input_stream", "read");
        if is_incoming_http_body_stream(&self.table, &self_) {
            Durability::<Ctx, Vec<u8>, SerializableStreamError>::wrap(
                self,
                WrappedFunctionType::ReadRemote,
                "http::types::incoming_body_stream::read",
                |ctx| {
                    Box::pin(async move {
                        HostInputStream::read(&mut ctx.as_wasi_view(), self_, len).await
                    })
                },
            )
            .await
        } else {
            HostInputStream::read(&mut self.as_wasi_view(), self_, len).await
        }
    }

    async fn blocking_read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        record_host_function_call("io::streams::input_stream", "blocking_read");
        if is_incoming_http_body_stream(&self.table, &self_) {
            Durability::<Ctx, Vec<u8>, SerializableStreamError>::wrap(
                self,
                WrappedFunctionType::ReadRemote,
                "http::types::incoming_body_stream::blocking_read",
                |ctx| {
                    Box::pin(async move {
                        HostInputStream::blocking_read(&mut ctx.as_wasi_view(), self_, len).await
                    })
                },
            )
            .await
        } else {
            HostInputStream::blocking_read(&mut self.as_wasi_view(), self_, len).await
        }
    }

    async fn skip(&mut self, self_: Resource<InputStream>, len: u64) -> Result<u64, StreamError> {
        record_host_function_call("io::streams::input_stream", "skip");
        if is_incoming_http_body_stream(&self.table, &self_) {
            Durability::<Ctx, u64, SerializableStreamError>::wrap(
                self,
                WrappedFunctionType::ReadRemote,
                "http::types::incoming_body_stream::skip",
                |ctx| {
                    Box::pin(async move {
                        HostInputStream::skip(&mut ctx.as_wasi_view(), self_, len).await
                    })
                },
            )
            .await
        } else {
            HostInputStream::skip(&mut self.as_wasi_view(), self_, len).await
        }
    }

    async fn blocking_skip(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        record_host_function_call("io::streams::input_stream", "blocking_skip");
        if is_incoming_http_body_stream(&self.table, &self_) {
            Durability::<Ctx, u64, SerializableStreamError>::wrap(
                self,
                WrappedFunctionType::ReadRemote,
                "http::types::incoming_body_stream::blocking_skip",
                |ctx| {
                    Box::pin(async move {
                        HostInputStream::blocking_skip(&mut ctx.as_wasi_view(), self_, len).await
                    })
                },
            )
            .await
        } else {
            HostInputStream::blocking_skip(&mut self.as_wasi_view(), self_, len).await
        }
    }

    fn subscribe(&mut self, self_: Resource<InputStream>) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("io::streams::input_stream", "subscribe");
        HostInputStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    fn drop(&mut self, rep: Resource<InputStream>) -> anyhow::Result<()> {
        record_host_function_call("io::streams::input_stream", "drop");
        HostInputStream::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostOutputStream for DurableWorkerCtx<Ctx> {
    fn check_write(&mut self, self_: Resource<OutputStream>) -> Result<u64, StreamError> {
        record_host_function_call("io::streams::output_stream", "check_write");
        HostOutputStream::check_write(&mut self.as_wasi_view(), self_)
    }

    fn write(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "write");

        let event_service = &self.public_state.event_service;

        let mut is_std = false;
        let is_live = self.is_live();
        let output = self.table.get(&self_)?;
        if output.as_any().downcast_ref::<ManagedStdOut>().is_some() {
            if is_live {
                event_service.emit_stdout(contents.clone());
            }
            is_std = true;
        } else if output.as_any().downcast_ref::<ManagedStdErr>().is_some() {
            if is_live {
                event_service.emit_stderr(contents.clone());
            }
            is_std = true;
        }

        if !is_std || is_live {
            HostOutputStream::write(&mut self.as_wasi_view(), self_, contents)
        } else {
            Ok(())
        }
    }

    async fn blocking_write_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "blocking_write_and_flush");

        let event_service = &self.public_state.event_service;

        let mut is_std = false;
        let is_live = self.is_live();
        let output = self.table.get(&self_)?;
        if output.as_any().downcast_ref::<ManagedStdOut>().is_some() {
            if is_live {
                event_service.emit_stdout(contents.clone());
            }
            is_std = true;
        } else if output.as_any().downcast_ref::<ManagedStdErr>().is_some() {
            if is_live {
                event_service.emit_stderr(contents.clone());
            }
            is_std = true;
        }

        if !is_std || is_live {
            HostOutputStream::blocking_write_and_flush(&mut self.as_wasi_view(), self_, contents)
                .await
        } else {
            Ok(())
        }
    }

    fn flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "flush");
        HostOutputStream::flush(&mut self.as_wasi_view(), self_)
    }

    async fn blocking_flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "blocking_flush");
        HostOutputStream::blocking_flush(&mut self.as_wasi_view(), self_).await
    }

    fn subscribe(&mut self, self_: Resource<OutputStream>) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("io::streams::output_stream", "subscribe");
        HostOutputStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    fn write_zeroes(&mut self, self_: Resource<OutputStream>, len: u64) -> Result<(), StreamError> {
        record_host_function_call("io::streams::output_stream", "write_zeroeas");
        HostOutputStream::write_zeroes(&mut self.as_wasi_view(), self_, len)
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

    fn drop(&mut self, rep: Resource<OutputStream>) -> anyhow::Result<()> {
        record_host_function_call("io::streams::output_stream", "drop");
        HostOutputStream::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn convert_stream_error(
        &mut self,
        err: StreamError,
    ) -> anyhow::Result<wasmtime_wasi::preview2::bindings::wasi::io::streams::StreamError> {
        Host::convert_stream_error(&mut self.as_wasi_view(), err)
    }
}

fn is_incoming_http_body_stream(table: &ResourceTable, stream: &Resource<InputStream>) -> bool {
    let stream = table.get::<InputStream>(stream).unwrap();
    match stream {
        InputStream::Host(host_input_stream) => {
            host_input_stream
                .as_any()
                .downcast_ref::<HostIncomingBodyStream>()
                .is_some()
                || host_input_stream
                    .as_any()
                    .downcast_ref::<FailingStream>()
                    .is_some()
        }
        InputStream::File(_) => false,
    }
}
