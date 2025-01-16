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

use crate::bindings::exports::wasi::io::streams::{InputStreamBorrow, Pollable, StreamError};
use crate::bindings::golem::durability::durability::observe_function_call;

impl From<crate::bindings::wasi::io::streams::StreamError> for StreamError {
    fn from(value: crate::bindings::wasi::io::streams::StreamError) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}

pub struct WrappedInputStream {
    pub input_stream: crate::bindings::wasi::io::streams::InputStream,
    pub is_incoming_http_body_stream: bool,
}

impl crate::bindings::exports::wasi::io::streams::GuestInputStream for WrappedInputStream {
    fn read(&self, len: u64) -> Result<Vec<u8>, StreamError> {
        if self.is_incoming_http_body_stream {
            // let handle = self_.rep();
            // let begin_idx = get_http_request_begin_idx(self, handle)?;
            //
            // let durability = Durability::<Vec<u8>, SerializableStreamError>::new(
            //     self,
            //     "http::types::incoming_body_stream",
            //     "read",
            //     DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            // )
            // .await?;
            //
            // let result = if durability.is_live() {
            //     let request = get_http_stream_request(self, handle)?;
            //     let result = HostInputStream::read(&mut self.as_wasi_view(), self_, len).await;
            //     durability.persist(self, request, result).await
            // } else {
            //     durability.replay(self).await
            // };
            //
            // end_http_request_if_closed(self, handle, &result).await?;
            // result
            todo!()
        } else {
            observe_function_call("io::streams::input_stream", "read");
            Ok(self.input_stream.read(len)?)
        }
    }

    fn blocking_read(&self, len: u64) -> Result<Vec<u8>, StreamError> {
        if self.is_incoming_http_body_stream {
            // let handle = self_.rep();
            // let begin_idx = get_http_request_begin_idx(self, handle)?;
            //
            // let durability = Durability::<Vec<u8>, SerializableStreamError>::new(
            //     self,
            //     "http::types::incoming_body_stream",
            //     "blocking_read",
            //     DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            // )
            // .await?;
            // let result = if durability.is_live() {
            //     let request = get_http_stream_request(self, handle)?;
            //     let result =
            //         HostInputStream::blocking_read(&mut self.as_wasi_view(), self_, len).await;
            //     durability.persist(self, request, result).await
            // } else {
            //     durability.replay(self).await
            // };
            //
            // end_http_request_if_closed(self, handle, &result).await?;
            // result
            todo!()
        } else {
            observe_function_call("io::streams::input_stream", "blocking_read");
            Ok(self.input_stream.blocking_read(len)?)
        }
    }

    fn skip(&self, len: u64) -> Result<u64, StreamError> {
        if self.is_incoming_http_body_stream {
            // let handle = self_.rep();
            // let begin_idx = get_http_request_begin_idx(self, handle)?;
            //
            // let durability = Durability::<u64, SerializableStreamError>::new(
            //     self,
            //     "http::types::incoming_body_stream",
            //     "skip",
            //     DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            // )
            // .await?;
            // let result = if durability.is_live() {
            //     let request = get_http_stream_request(self, handle)?;
            //     let result = HostInputStream::skip(&mut self.as_wasi_view(), self_, len).await;
            //     durability.persist(self, request, result).await
            // } else {
            //     durability.replay(self).await
            // };
            //
            // end_http_request_if_closed(self, handle, &result).await?;
            // result
            todo!()
        } else {
            observe_function_call("io::streams::input_stream", "skip");
            Ok(self.input_stream.skip(len)?)
        }
    }

    fn blocking_skip(&self, len: u64) -> Result<u64, StreamError> {
        if self.is_incoming_http_body_stream {
            // let handle = self_.rep();
            // let begin_idx = get_http_request_begin_idx(self, handle)?;
            //
            // let durability = Durability::<u64, SerializableStreamError>::new(
            //     self,
            //     "http::types::incoming_body_stream",
            //     "blocking_skip",
            //     DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
            // )
            // .await?;
            //
            // let result = if durability.is_live() {
            //     let request = get_http_stream_request(self, handle)?;
            //     let result =
            //         HostInputStream::blocking_skip(&mut self.as_wasi_view(), self_, len).await;
            //     durability.persist(self, request, result).await
            // } else {
            //     durability.replay(self).await
            // };
            // end_http_request_if_closed(self, handle, &result).await?;
            // result
            todo!()
        } else {
            observe_function_call("io::streams::input_stream", "blocking_skip");
            Ok(self.input_stream.blocking_skip(len)?)
        }
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call("io::streams::input_stream", "subscribe");
        let pollable = self.input_stream.subscribe();
        Pollable::new(crate::wrappers::io::poll::WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedInputStream {
    fn drop(&mut self) {
        observe_function_call("io::streams::input_stream", "drop");

        if self.is_incoming_http_body_stream {
            // let handle = rep.rep();
            // if let Some(state) = self.state.open_http_requests.get(&handle) {
            //     if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
            //         end_http_request(self, handle).await?;
            //     }
            // }
            todo!()
        }
    }
}

pub struct WrappedOutputStream {
    pub output_stream: crate::bindings::wasi::io::streams::OutputStream,
}

impl crate::bindings::exports::wasi::io::streams::GuestOutputStream for WrappedOutputStream {
    fn check_write(&self) -> Result<u64, StreamError> {
        observe_function_call("io::streams::output_stream", "check_write");
        Ok(self.output_stream.check_write()?)
    }

    fn write(&self, contents: Vec<u8>) -> Result<(), StreamError> {
        observe_function_call("io::streams::output_stream", "write");
        Ok(self.output_stream.write(&contents)?)
    }

    fn blocking_write_and_flush(&self, contents: Vec<u8>) -> Result<(), StreamError> {
        observe_function_call("io::streams::output_stream", "blocking_write_and_flush");
        Ok(self.output_stream.blocking_write_and_flush(&contents)?)
    }

    fn flush(&self) -> Result<(), StreamError> {
        observe_function_call("io::streams::output_stream", "flush");
        Ok(self.output_stream.flush()?)
    }

    fn blocking_flush(&self) -> Result<(), StreamError> {
        observe_function_call("io::streams::output_stream", "blocking_flush");
        Ok(self.output_stream.blocking_flush()?)
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call("io::streams::output_stream", "subscribe");
        let pollable = self.output_stream.subscribe();
        Pollable::new(crate::wrappers::io::poll::WrappedPollable::Proxy(pollable))
    }

    fn write_zeroes(&self, len: u64) -> Result<(), StreamError> {
        observe_function_call("io::streams::output_stream", "write_zeroes");
        Ok(self.output_stream.write_zeroes(len)?)
    }

    fn blocking_write_zeroes_and_flush(&self, len: u64) -> Result<(), StreamError> {
        observe_function_call(
            "io::streams::output_stream",
            "blocking_write_zeroes_and_flush",
        );
        Ok(self.output_stream.blocking_write_zeroes_and_flush(len)?)
    }

    fn splice(&self, src: InputStreamBorrow<'_>, len: u64) -> Result<u64, StreamError> {
        observe_function_call("io::streams::output_stream", "splice");
        let input_stream: &WrappedInputStream = src.get();
        Ok(self.output_stream.splice(&input_stream.input_stream, len)?)
    }

    fn blocking_splice(&self, src: InputStreamBorrow<'_>, len: u64) -> Result<u64, StreamError> {
        observe_function_call("io::streams::output_stream", "blocking_splice");
        let input_stream: &WrappedInputStream = src.get();
        Ok(self
            .output_stream
            .blocking_splice(&input_stream.input_stream, len)?)
    }
}

impl Drop for WrappedOutputStream {
    fn drop(&mut self) {
        observe_function_call("io::streams::output_stream", "drop");
    }
}

impl crate::bindings::exports::wasi::io::streams::Guest for crate::Component {
    type InputStream = WrappedInputStream;
    type OutputStream = WrappedOutputStream;
}
//
// fn end_http_request_if_closed<Ctx: WorkerCtx, T>(
//     ctx: &mut DurableWorkerCtx<Ctx>,
//     handle: u32,
//     result: &Result<T, StreamError>,
// ) -> Result<(), GolemError> {
//     if matches!(result, Err(StreamError::Closed)) {
//         if let Some(state) = ctx.state.open_http_requests.get(&handle) {
//             if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
//                 end_http_request(ctx, handle).await?;
//             }
//         }
//     }
//     Ok(())
// }
//
// fn get_http_request_begin_idx<Ctx: WorkerCtx>(
//     ctx: &mut DurableWorkerCtx<Ctx>,
//     handle: u32,
// ) -> Result<OplogIndex, StreamError> {
//     let request_state = ctx.state.open_http_requests.get(&handle).ok_or_else(|| {
//         StreamError::Trap(anyhow!(
//             "No matching HTTP request is associated with resource handle"
//         ))
//     })?;
//     let begin_idx = *ctx
//         .state
//         .open_function_table
//         .get(&request_state.root_handle)
//         .ok_or_else(|| {
//             StreamError::Trap(anyhow!(
//                 "No matching BeginRemoteWrite index was found for the open HTTP request"
//             ))
//         })?;
//     Ok(begin_idx)
// }
//
// fn get_http_stream_request<Ctx: WorkerCtx>(
//     ctx: &mut DurableWorkerCtx<Ctx>,
//     handle: u32,
// ) -> Result<SerializableHttpRequest, StreamError> {
//     let request_state = ctx.state.open_http_requests.get(&handle).ok_or_else(|| {
//         StreamError::Trap(anyhow!(
//             "No matching HTTP request is associated with resource handle"
//         ))
//     })?;
//     Ok(request_state.request.clone())
// }
