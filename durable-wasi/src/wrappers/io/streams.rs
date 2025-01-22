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

use crate::bindings::exports::wasi::io::error::Error;
use crate::bindings::exports::wasi::io::poll::GuestPollable;
use crate::bindings::exports::wasi::io::streams::{
    GuestInputStream, InputStreamBorrow, Pollable, StreamError,
};
use crate::bindings::golem::durability::durability::{
    observe_function_call, DurableFunctionType, OplogIndex,
};
use crate::durability::Durability;
use crate::wrappers::http::serialized::SerializableHttpRequest;
use crate::wrappers::http::{
    end_http_request, HttpRequestCloseOwner, OPEN_FUNCTION_TABLE, OPEN_HTTP_REQUESTS,
};
use crate::wrappers::io::error::WrappedError;
use crate::wrappers::io::poll::WrappedPollable;
use crate::wrappers::SerializableStreamError;
use std::cell::RefCell;
use std::cmp::min;

impl From<crate::bindings::wasi::io::streams::StreamError> for StreamError {
    fn from(value: crate::bindings::wasi::io::streams::StreamError) -> Self {
        unsafe { std::mem::transmute(value) }
    }
}

pub enum WrappedInputStream {
    Proxied {
        input_stream: crate::bindings::wasi::io::streams::InputStream,
    },
    ProxiedIncomingHttpBodyStream {
        input_stream: crate::bindings::wasi::io::streams::InputStream,
    },
    ReplayedIncomingHttpBodyStream {
        handle: Option<u32>,
    },
    Buffered {
        data: RefCell<Vec<u8>>,
    },
}

impl WrappedInputStream {
    pub fn proxied(input_stream: crate::bindings::wasi::io::streams::InputStream) -> Self {
        WrappedInputStream::Proxied { input_stream }
    }

    pub fn incoming_http_body_stream(
        input_stream: crate::bindings::wasi::io::streams::InputStream,
    ) -> Self {
        WrappedInputStream::ProxiedIncomingHttpBodyStream { input_stream }
    }

    pub fn replayed_incoming_http_body_stream() -> Self {
        WrappedInputStream::ReplayedIncomingHttpBodyStream { handle: None }
    }

    pub fn buffered(data: Vec<u8>) -> Self {
        WrappedInputStream::Buffered {
            data: RefCell::new(data),
        }
    }

    pub fn assign_replay_stream_handle(&mut self, handle: u32) {
        if let WrappedInputStream::ReplayedIncomingHttpBodyStream { handle: ref mut h } = self {
            *h = Some(handle);
        } else {
            panic!("Unexpected call to assign_replay_stream_handle");
        }
    }
}

impl crate::bindings::exports::wasi::io::streams::GuestInputStream for WrappedInputStream {
    fn read(&self, len: u64) -> Result<Vec<u8>, StreamError> {
        match self {
            WrappedInputStream::Proxied { input_stream } => {
                observe_function_call("io::streams::input_stream", "read");
                Ok(input_stream.read(len)?)
            }
            WrappedInputStream::ProxiedIncomingHttpBodyStream { input_stream } => {
                let begin_idx = get_http_request_begin_idx(input_stream.handle());
                let durability = Durability::<Vec<u8>, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "read",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                assert!(durability.is_live());

                let request = get_http_stream_request(input_stream.handle());
                let result = input_stream.read(len).map_err(|e| e.into());
                let result = durability.persist(request, result);

                end_http_request_if_closed(input_stream.handle(), &result);

                result
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream {
                handle: Some(handle),
            } => {
                let begin_idx = get_http_request_begin_idx(*handle);
                let durability = Durability::<Vec<u8>, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "read",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                if durability.is_live() {
                    let error = Error::new(WrappedError::message(
                        "Body stream was interrupted due to a restart",
                    ));
                    Err(StreamError::LastOperationFailed(error))
                } else {
                    let result = durability.replay();

                    end_http_request_if_closed(*handle, &result);

                    result
                }
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream { handle: None } => {
                panic!("No handle associated with replayed incoming HTTP body stream")
            }
            WrappedInputStream::Buffered { data } => {
                let mut data = data.borrow_mut();
                let len = min(len as usize, data.len());
                let result = data.drain(..len).collect();
                Ok(result)
            }
        }
    }

    fn blocking_read(&self, len: u64) -> Result<Vec<u8>, StreamError> {
        match self {
            WrappedInputStream::Proxied { input_stream } => {
                observe_function_call("io::streams::input_stream", "blocking_read");
                Ok(input_stream.blocking_read(len)?)
            }
            WrappedInputStream::ProxiedIncomingHttpBodyStream { input_stream } => {
                let begin_idx = get_http_request_begin_idx(input_stream.handle());
                let durability = Durability::<Vec<u8>, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "blocking_read",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                assert!(durability.is_live());

                let request = get_http_stream_request(input_stream.handle());
                let result = input_stream.blocking_read(len).map_err(|e| e.into());
                let result = durability.persist(request, result);

                end_http_request_if_closed(input_stream.handle(), &result);

                result
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream {
                handle: Some(handle),
            } => {
                let begin_idx = get_http_request_begin_idx(*handle);
                let durability = Durability::<Vec<u8>, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "blocking_read",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                if durability.is_live() {
                    let error = Error::new(WrappedError::message(
                        "Body stream was interrupted due to a restart",
                    ));
                    Err(StreamError::LastOperationFailed(error))
                } else {
                    let result = durability.replay();

                    end_http_request_if_closed(*handle, &result);

                    result
                }
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream { handle: None } => {
                panic!("No handle associated with replayed incoming HTTP body stream")
            }
            WrappedInputStream::Buffered { data } => {
                let mut data = data.borrow_mut();
                let len = min(len as usize, data.len());
                let result = data.drain(..len).collect();
                Ok(result)
            }
        }
    }

    fn skip(&self, len: u64) -> Result<u64, StreamError> {
        match self {
            WrappedInputStream::Proxied { input_stream } => {
                observe_function_call("io::streams::input_stream", "skip");
                Ok(input_stream.skip(len)?)
            }
            WrappedInputStream::ProxiedIncomingHttpBodyStream { input_stream } => {
                let begin_idx = get_http_request_begin_idx(input_stream.handle());
                let durability = Durability::<u64, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "skip",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                assert!(durability.is_live());

                let request = get_http_stream_request(input_stream.handle());
                let result = input_stream.skip(len).map_err(|e| e.into());
                let result = durability.persist(request, result);

                end_http_request_if_closed(input_stream.handle(), &result);

                result
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream {
                handle: Some(handle),
            } => {
                let begin_idx = get_http_request_begin_idx(*handle);
                let durability = Durability::<u64, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "skip",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                if durability.is_live() {
                    let error = Error::new(WrappedError::message(
                        "Body stream was interrupted due to a restart",
                    ));
                    Err(StreamError::LastOperationFailed(error))
                } else {
                    let result = durability.replay();

                    end_http_request_if_closed(*handle, &result);

                    result
                }
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream { handle: None } => {
                panic!("No handle associated with replayed incoming HTTP body stream")
            }
            WrappedInputStream::Buffered { data } => {
                let mut data = data.borrow_mut();
                let len = min(len as usize, data.len());
                data.drain(..len);
                Ok(len as u64)
            }
        }
    }

    fn blocking_skip(&self, len: u64) -> Result<u64, StreamError> {
        match self {
            WrappedInputStream::Proxied { input_stream } => {
                observe_function_call("io::streams::input_stream", "blocking_skip");
                Ok(input_stream.blocking_skip(len)?)
            }
            WrappedInputStream::ProxiedIncomingHttpBodyStream { input_stream } => {
                let begin_idx = get_http_request_begin_idx(input_stream.handle());
                let durability = Durability::<u64, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "blocking_skip",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                assert!(durability.is_live());

                let request = get_http_stream_request(input_stream.handle());
                let result = input_stream.blocking_skip(len).map_err(|e| e.into());
                let result = durability.persist(request, result);

                end_http_request_if_closed(input_stream.handle(), &result);

                result
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream {
                handle: Some(handle),
            } => {
                let begin_idx = get_http_request_begin_idx(*handle);
                let durability = Durability::<u64, SerializableStreamError>::new(
                    "http::types::incoming_body_stream",
                    "blocking_skip",
                    DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                );

                if durability.is_live() {
                    let error = Error::new(WrappedError::message(
                        "Body stream was interrupted due to a restart",
                    ));
                    Err(StreamError::LastOperationFailed(error))
                } else {
                    let result = durability.replay();

                    end_http_request_if_closed(*handle, &result);

                    result
                }
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream { handle: None } => {
                panic!("No handle associated with replayed incoming HTTP body stream")
            }
            WrappedInputStream::Buffered { data } => {
                let mut data = data.borrow_mut();
                let len = min(len as usize, data.len());
                data.drain(..len);
                Ok(len as u64)
            }
        }
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call("io::streams::input_stream", "subscribe");
        match self {
            WrappedInputStream::Proxied { input_stream }
            | WrappedInputStream::ProxiedIncomingHttpBodyStream { input_stream } => {
                let pollable = input_stream.subscribe();
                Pollable::new(WrappedPollable::Proxy(pollable))
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream { .. } => {
                Pollable::new(WrappedPollable::Ready)
            }
            WrappedInputStream::Buffered { .. } => Pollable::new(WrappedPollable::Ready),
        }
    }
}

impl Drop for WrappedInputStream {
    fn drop(&mut self) {
        observe_function_call("io::streams::input_stream", "drop");

        match self {
            WrappedInputStream::Proxied { .. } => {}
            WrappedInputStream::ProxiedIncomingHttpBodyStream { input_stream } => {
                OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
                    if let Some(state) = open_http_requests.get(&input_stream.handle()) {
                        if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
                            end_http_request(input_stream.handle());
                        }
                    }
                })
            }
            WrappedInputStream::ReplayedIncomingHttpBodyStream {
                handle: Some(handle),
            } => OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
                if let Some(state) = open_http_requests.get(handle) {
                    if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
                        end_http_request(*handle);
                    }
                }
            }),
            WrappedInputStream::ReplayedIncomingHttpBodyStream { handle: None } => {}
            WrappedInputStream::Buffered { .. } => {}
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
        let len = min(len, self.check_write()?);
        let data = src.get::<WrappedInputStream>().read(len)?;
        let data_len = data.len() as u64;
        self.write(data)?;
        Ok(data_len)
    }

    fn blocking_splice(&self, src: InputStreamBorrow<'_>, len: u64) -> Result<u64, StreamError> {
        observe_function_call("io::streams::output_stream", "blocking_splice");
        let pollable = self.subscribe();
        pollable.get::<WrappedPollable>().block();
        let len = min(len, self.check_write()?);
        let data = src.get::<WrappedInputStream>().read(len)?;
        let data_len = data.len() as u64;
        self.write(data)?;
        Ok(data_len)
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

fn end_http_request_if_closed<T>(handle: u32, result: &Result<T, StreamError>) {
    if matches!(result, Err(StreamError::Closed)) {
        OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
            if let Some(state) = open_http_requests.get(&handle) {
                if state.close_owner == HttpRequestCloseOwner::InputStreamClosed {
                    end_http_request(handle);
                }
            }
        })
    }
}

fn get_http_request_begin_idx(handle: u32) -> OplogIndex {
    OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
        let request_state = open_http_requests
            .get(&handle)
            .expect("No matching HTTP request is associated with resource handle");
        OPEN_FUNCTION_TABLE.with_borrow(|open_function_table| {
            let begin_idx = open_function_table
                .get(&request_state.root_handle)
                .expect("No matching BeginRemoteWrite index was found for the open HTTP request");
            *begin_idx
        })
    })
}

fn get_http_stream_request(handle: u32) -> SerializableHttpRequest {
    OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
        let request_state = open_http_requests
            .get(&handle)
            .expect("No matching HTTP request is associated with resource handle");
        request_state.request.clone()
    })
}
