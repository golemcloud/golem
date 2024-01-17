use async_trait::async_trait;
use bincode::{Decode, Encode};
use crate::error::GolemError;
use serde::{Deserialize, Serialize};
use tracing::debug;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::{StreamError, Table};

use crate::golem_host::io::{ManagedStdErr, ManagedStdOut};
use crate::golem_host::{Durability, GolemCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::wasi::io::streams::{
    Host, HostInputStream, HostOutputStream, InputStream, OutputStream, Pollable,
};
use wasmtime_wasi_http::body::{FailingStream, HostIncomingBodyStream};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostInputStream for GolemCtx<Ctx> {
    async fn read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        record_host_function_call("io::streams::input_stream", "read");
        if is_incoming_http_body_stream(&self.table, &self_) {
            debug!("read from incoming http body stream");
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
            debug!("read from arbitrary stream (non durable)");
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
impl<Ctx: WorkerCtx> HostOutputStream for GolemCtx<Ctx> {
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
impl<Ctx: WorkerCtx> Host for GolemCtx<Ctx> {
    fn convert_stream_error(
        &mut self,
        err: StreamError,
    ) -> anyhow::Result<wasmtime_wasi::preview2::bindings::wasi::io::streams::StreamError> {
        Host::convert_stream_error(&mut self.as_wasi_view(), err)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
enum SerializableStreamError {
    Closed,
    LastOperationFailed(SerializableError),
    Trap(SerializableError),
}

impl From<&StreamError> for SerializableStreamError {
    fn from(value: &StreamError) -> Self {
        match value {
            StreamError::Closed => Self::Closed,
            StreamError::LastOperationFailed(e) => Self::LastOperationFailed(e.into()),
            StreamError::Trap(e) => Self::Trap(e.into()),
        }
    }
}

impl From<SerializableStreamError> for StreamError {
    fn from(value: SerializableStreamError) -> Self {
        match value {
            SerializableStreamError::Closed => Self::Closed,
            SerializableStreamError::LastOperationFailed(e) => Self::LastOperationFailed(e.into()),
            SerializableStreamError::Trap(e) => Self::Trap(e.into()),
        }
    }
}

impl From<GolemError> for SerializableStreamError {
    fn from(value: GolemError) -> Self {
        Self::Trap(value.into())
    }
}

fn is_incoming_http_body_stream(table: &Table, stream: &Resource<InputStream>) -> bool {
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
