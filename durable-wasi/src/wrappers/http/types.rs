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

use crate::bindings::exports::wasi::http::types::{
    Duration, ErrorCode, FieldKey, FieldValue, Fields, FutureTrailers, HeaderError, Headers,
    IncomingBody, IncomingResponse, InputStream, IoErrorBorrow, Method, OutgoingBody,
    OutgoingResponse, OutputStream, Pollable, ResponseOutparam, Scheme, StatusCode, Trailers,
};
use crate::bindings::golem::durability::durability::{
    current_durable_execution_state, observe_function_call, persist_durable_function_invocation,
    read_persisted_durable_function_invocation, DurableFunctionType, PersistenceLevel,
};
use crate::durability::Durability;
use crate::wrappers::http::serialized::{
    SerializableErrorCode, SerializableResponse, SerializableResponseHeaders,
};
use crate::wrappers::http::{
    continue_http_request, end_http_request, HttpRequestCloseOwner, OPEN_FUNCTION_TABLE,
    OPEN_HTTP_REQUESTS,
};
use crate::wrappers::io::poll::WrappedPollable;
use crate::wrappers::io::streams::{WrappedInputStream, WrappedOutputStream};
use crate::wrappers::SerializableError;
use golem_common::serialization::{serialize, try_deserialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::transmute;
use crate::bindings::wasi::http::types::http_error_code;
use crate::wrappers::io::error::WrappedError;

impl From<crate::bindings::wasi::http::types::HeaderError> for HeaderError {
    fn from(value: crate::bindings::wasi::http::types::HeaderError) -> Self {
        unsafe { transmute(value) }
    }
}

impl From<crate::bindings::wasi::http::types::ErrorCode> for ErrorCode {
    fn from(value: crate::bindings::wasi::http::types::ErrorCode) -> Self {
        unsafe { transmute(value) }
    }
}

struct WrappedFields {
    fields: crate::bindings::wasi::http::types::Fields,
}

impl crate::bindings::exports::wasi::http::types::GuestFields for WrappedFields {
    fn new() -> Self {
        observe_function_call("http::types::fields", "new");
        let fields = crate::bindings::wasi::http::types::Fields::new();
        WrappedFields { fields }
    }

    fn from_list(entries: Vec<(FieldKey, FieldValue)>) -> Result<Fields, HeaderError> {
        observe_function_call("http::types::fields", "from_list");
        let fields = crate::bindings::wasi::http::types::Fields::from_list(&entries)?;
        Ok(Fields::new(WrappedFields { fields }))
    }

    fn get(&self, name: FieldKey) -> Vec<FieldValue> {
        observe_function_call("http::types::fields", "get");
        self.fields.get(&name)
    }

    fn has(&self, name: FieldKey) -> bool {
        observe_function_call("http::types::fields", "has");
        self.fields.has(&name)
    }

    fn set(&self, name: FieldKey, value: Vec<FieldValue>) -> Result<(), HeaderError> {
        observe_function_call("http::types::fields", "set");
        Ok(self.fields.set(&name, &value)?)
    }

    fn delete(&self, name: FieldKey) -> Result<(), HeaderError> {
        observe_function_call("http::types::fields", "delete");
        Ok(self.fields.delete(&name)?)
    }

    fn append(&self, name: FieldKey, value: FieldValue) -> Result<(), HeaderError> {
        observe_function_call("http::types::fields", "append");
        Ok(self.fields.append(&name, &value)?)
    }

    fn entries(&self) -> Vec<(FieldKey, FieldValue)> {
        observe_function_call("http::types::fields", "entries");
        self.fields.entries()
    }

    fn clone(&self) -> Fields {
        observe_function_call("http::types::fields", "clone");
        Fields::new(WrappedFields {
            fields: self.fields.clone(),
        })
    }
}

impl Drop for WrappedFields {
    fn drop(&mut self) {
        observe_function_call("http::types::fields", "drop");
    }
}

struct WrappedIncomingRequest {
    request: crate::bindings::wasi::http::types::IncomingRequest,
}

impl crate::bindings::exports::wasi::http::types::GuestIncomingRequest for WrappedIncomingRequest {
    fn method(&self) -> Method {
        observe_function_call("http::types::incoming_request", "method");
        let method = self.request.method();
        let method = unsafe { transmute(method) };
        method
    }

    fn path_with_query(&self) -> Option<String> {
        observe_function_call("http::types::incoming_request", "path_with_query");
        self.request.path_with_query()
    }

    fn scheme(&self) -> Option<Scheme> {
        observe_function_call("http::types::incoming_request", "scheme");
        let scheme = self.request.scheme();
        let scheme = unsafe { transmute(scheme) };
        scheme
    }

    fn authority(&self) -> Option<String> {
        observe_function_call("http::types::incoming_request", "authority");
        self.request.authority()
    }

    fn headers(&self) -> Headers {
        observe_function_call("http::types::incoming_request", "headers");
        let headers = self.request.headers();
        Headers::new(WrappedFields { fields: headers })
    }

    fn consume(&self) -> Result<IncomingBody, ()> {
        observe_function_call("http::types::incoming_request", "consume");
        let body = self.request.consume()?;
        Ok(IncomingBody::new(WrappedIncomingBody { body }))
    }
}

impl Drop for WrappedIncomingRequest {
    fn drop(&mut self) {
        observe_function_call("http::types::incoming_request", "drop");
    }
}

struct WrappedOutgoingRequest {
    request: crate::bindings::wasi::http::types::OutgoingRequest,
}

impl crate::bindings::exports::wasi::http::types::GuestOutgoingRequest for WrappedOutgoingRequest {
    fn new(headers: Headers) -> Self {
        observe_function_call("http::types::outgoing_request", "new");
        let headers = headers.into_inner::<WrappedFields>().fields;
        let request = crate::bindings::wasi::http::types::OutgoingRequest::new(headers);
        WrappedOutgoingRequest { request }
    }

    fn body(&self) -> Result<OutgoingBody, ()> {
        observe_function_call("http::types::outgoing_request", "body");
        let body = self.request.body()?;
        Ok(OutgoingBody::new(WrappedOutgoingBody { body }))
    }

    fn method(&self) -> Method {
        observe_function_call("http::types::outgoing_request", "method");
        let method = self.request.method();
        let method = unsafe { transmute(method) };
        method
    }

    fn set_method(&self, method: Method) -> Result<(), ()> {
        observe_function_call("http::types::outgoing_request", "set_method");
        let method = unsafe { transmute(method) };
        Ok(self.request.set_method(method)?)
    }

    fn path_with_query(&self) -> Option<String> {
        observe_function_call("http::types::outgoing_request", "path_with_query");
        self.request.path_with_query()
    }

    fn set_path_with_query(&self, path_with_query: Option<String>) -> Result<(), ()> {
        observe_function_call("http::types::outgoing_request", "set_path_with_query");
        Ok(self
            .request
            .set_path_with_query(path_with_query.map(|s| s.as_str()))?)
    }

    fn scheme(&self) -> Option<Scheme> {
        observe_function_call("http::types::outgoing_request", "scheme");
        let scheme = self.request.scheme();
        let scheme = unsafe { transmute(scheme) };
        scheme
    }

    fn set_scheme(&self, scheme: Option<Scheme>) -> Result<(), ()> {
        observe_function_call("http::types::outgoing_request", "set_scheme");
        let scheme = unsafe { transmute(scheme) };
        Ok(self.request.set_scheme(scheme)?)
    }

    fn authority(&self) -> Option<String> {
        observe_function_call("http::types::outgoing_request", "authority");
        self.request.authority()
    }

    fn set_authority(&self, authority: Option<String>) -> Result<(), ()> {
        observe_function_call("http::types::outgoing_request", "set_authority");
        Ok(self.request.set_authority(authority.map(|s| s.as_str()))?)
    }

    fn headers(&self) -> Headers {
        observe_function_call("http::types::outgoing_request", "headers");
        let headers = self.request.headers();
        Headers::new(WrappedFields { fields: headers })
    }
}

impl Drop for WrappedOutgoingRequest {
    fn drop(&mut self) {
        observe_function_call("http::types::outgoing_request", "drop");
    }
}

struct WrappedRequestOptions {
    options: crate::bindings::wasi::http::types::RequestOptions,
}

impl crate::bindings::exports::wasi::http::types::GuestRequestOptions for WrappedRequestOptions {
    fn new() -> Self {
        observe_function_call("http::types::request_options", "new");
        let options = crate::bindings::wasi::http::types::RequestOptions::new();
        WrappedRequestOptions { options }
    }

    fn connect_timeout(&self) -> Option<Duration> {
        observe_function_call("http::types::request_options", "connect_timeout_ms");
        self.options.connect_timeout()
    }

    fn set_connect_timeout(&self, duration: Option<Duration>) -> Result<(), ()> {
        observe_function_call("http::types::request_options", "set_connect_timeout_ms");
        Ok(self.options.set_connect_timeout(duration)?)
    }

    fn first_byte_timeout(&self) -> Option<Duration> {
        observe_function_call("http::types::request_options", "first_byte_timeout_ms");
        self.options.first_byte_timeout()
    }

    fn set_first_byte_timeout(&self, duration: Option<Duration>) -> Result<(), ()> {
        observe_function_call("http::types::request_options", "set_first_byte_timeout_ms");
        Ok(self.options.set_first_byte_timeout(duration)?)
    }

    fn between_bytes_timeout(&self) -> Option<Duration> {
        observe_function_call("http::types::request_options", "between_bytes_timeout_ms");
        self.options.between_bytes_timeout()
    }

    fn set_between_bytes_timeout(&self, duration: Option<Duration>) -> Result<(), ()> {
        observe_function_call(
            "http::types::request_options",
            "set_between_bytes_timeout_ms",
        );
        Ok(self.options.set_between_bytes_timeout(duration)?)
    }
}

impl Drop for WrappedRequestOptions {
    fn drop(&mut self) {
        observe_function_call("http::types::request_options", "drop");
    }
}

struct WrappedResponseOutparam {
    outparam: crate::bindings::wasi::http::types::ResponseOutparam,
}

impl crate::bindings::exports::wasi::http::types::GuestResponseOutparam
    for WrappedResponseOutparam
{
    fn set(param: ResponseOutparam, response: Result<OutgoingResponse, ErrorCode>) {
        observe_function_call("http::types::response_outparam", "set");
        let param = param.into_inner::<WrappedResponseOutparam>().outparam;
        let response = response
            .map(|r| r.into_inner::<WrappedOutgoingResponse>().response)
            .map_err(|err| unsafe { transmute(err) });
        crate::bindings::wasi::http::types::ResponseOutparam::set(param, response)
    }
}

impl Drop for WrappedResponseOutparam {
    fn drop(&mut self) {
        observe_function_call("http::types::response_outparam", "drop");
    }
}

struct WrappedIncomingResponse {
    state: RefCell<WrappedIncomingResponseState>,
    headers: RefCell<Option<crate::bindings::wasi::http::types::Headers>>,
}

impl WrappedIncomingResponse {
    pub fn proxied(response: crate::bindings::wasi::http::types::IncomingResponse) -> Self {
        WrappedIncomingResponse {
            state: RefCell::new(WrappedIncomingResponseState::Proxy { response }),
            headers: RefCell::new(None),
        }
    }

    pub fn replayed(serializable_response_headers: SerializableResponseHeaders) -> Self {
        WrappedIncomingResponse {
            state: RefCell::new(WrappedIncomingResponseState::Replayed {
                serializable_response_headers,
            }),
            headers: RefCell::new(None),
        }
    }
}

enum WrappedIncomingResponseState {
    Proxy {
        response: crate::bindings::wasi::http::types::IncomingResponse,
    },
    Replayed {
        serializable_response_headers: SerializableResponseHeaders,
    },
}

impl crate::bindings::exports::wasi::http::types::GuestIncomingResponse
    for WrappedIncomingResponse
{
    fn status(&self) -> StatusCode {
        observe_function_call("http::types::incoming_response", "status");
        match self.state.borrow() {
            WrappedIncomingResponseState::Proxy { response } => response.status(),
            WrappedIncomingResponseState::Replayed {
                serializable_response_headers,
            } => serializable_response_headers.status,
        }
    }

    fn headers(&self) -> Headers {
        observe_function_call("http::types::incoming_response", "headers");
        let headers = self.headers.borrow_mut().get_or_insert(|| {
            let state = self.state.borrow_mut();
            match &state {
                WrappedIncomingResponseState::Proxy { response } => response.headers(),
                WrappedIncomingResponseState::Replayed {
                    serializable_response_headers,
                } => {
                    let entries = serializable_response_headers
                        .headers
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>();
                    crate::bindings::wasi::http::types::Fields::from_list(&entries)
                }
            }
        });
        Headers::new(WrappedFields { fields: headers })
    }

    fn consume(&self) -> Result<IncomingBody, ()> {
        observe_function_call("http::types::incoming_response", "consume");
        match self.state.borrow() {
            WrappedIncomingResponseState::Proxy { response } => {
                let body = response.consume()?;
                continue_http_request(
                    response.handle(),
                    body.handle(),
                    HttpRequestCloseOwner::IncomingBodyDropOrFinish,
                );
                Ok(IncomingBody::new(WrappedIncomingBody { body }))
            }
            WrappedIncomingResponseState::Replayed { .. } => {
                // TODO: continue_http_request
                Ok(IncomingBody::new(WrappedIncomingBody::replayed()))
            }
        }
    }
}

impl Drop for WrappedIncomingResponse {
    fn drop(&mut self) {
        observe_function_call("http::types::incoming_response", "drop");

        match self.state.borrow() {
            WrappedIncomingResponseState::Proxy { response } => {
                let handle = response.handle();
                OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
                    if let Some(state) = open_http_requests.get(&handle) {
                        if state.close_owner == HttpRequestCloseOwner::IncomingResponseDrop {
                            end_http_request(handle);
                        }
                    }
                });
            }
            WrappedIncomingResponseState::Replayed { .. } => {
                todo!()
            }
        }
    }
}

enum WrappedIncomingBody {
    Proxied {
        body: crate::bindings::wasi::http::types::IncomingBody,
    },
    Replayed,
}

impl WrappedIncomingBody {
    pub fn proxied(body: crate::bindings::wasi::http::types::IncomingBody) -> Self {
        WrappedIncomingBody::Proxied { body }
    }

    pub fn replayed() -> Self {
        WrappedIncomingBody::Replayed
    }
}

impl crate::bindings::exports::wasi::http::types::GuestIncomingBody for WrappedIncomingBody {
    fn stream(&self) -> Result<InputStream, ()> {
        observe_function_call("http::types::incoming_body", "stream");

        match self {
            Self::Proxied { body } => {
                let stream = body.stream()?;

                continue_http_request(
                    body.handle(),
                    stream.handle(),
                    HttpRequestCloseOwner::InputStreamClosed,
                );

                Ok(InputStream::new(WrappedInputStream {
                    input_stream: stream,
                    is_incoming_http_body_stream: true,
                }))
            }
            Self::Replayed => {
                todo!()
            }
        }
    }

    fn finish(this: IncomingBody) -> FutureTrailers {
        observe_function_call("http::types::incoming_body", "finish");

        let this = this.into_inner::<WrappedIncomingBody>();

        match this {
            Self::Proxied { body } => {
                OPEN_HTTP_REQUESTS.with_borrow_mut(|open_http_requests| {
                    let handle = body.handle();
                    if let Some(state) = open_http_requests.get(&handle) {
                        if state.close_owner == HttpRequestCloseOwner::IncomingBodyDropOrFinish {
                            end_http_request(handle);
                        }
                    }
                });

                let future_trailers =
                    crate::bindings::wasi::http::types::IncomingBody::finish(body);
                FutureTrailers::new(WrappedFutureTrailers {
                    trailers: future_trailers,
                })
            }
            Self::Replayed => {
                todo!()
            }
        }
    }
}

impl Drop for WrappedIncomingBody {
    fn drop(&mut self) {
        observe_function_call("http::types::incoming_body", "drop");

        match self {
            Self::Proxied { body } => {
                OPEN_HTTP_REQUESTS.with_borrow_mut(|open_http_requests| {
                    let handle = body.handle();
                    if let Some(state) = open_http_requests.get(&handle) {
                        if state.close_owner == HttpRequestCloseOwner::IncomingBodyDropOrFinish {
                            end_http_request(handle);
                        }
                    }
                });
            }
            Self::Replayed => {
                todo!()
            }
        }
    }
}

struct WrappedFutureTrailers {
    trailers: crate::bindings::wasi::http::types::FutureTrailers,
}

impl crate::bindings::exports::wasi::http::types::GuestFutureTrailers for WrappedFutureTrailers {
    fn subscribe(&self) -> Pollable {
        observe_function_call("http::types::future_trailers", "subscribe");
        let pollable = self.trailers.subscribe();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }

    fn get(&self) -> Option<Result<Result<Option<Trailers>, ErrorCode>, ()>> {
        observe_function_call("http::types::future_trailers", "get");

        OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
            OPEN_FUNCTION_TABLE.with_borrow(|open_function_table| {
                let handle = unsafe { self.trailers.handle() };
                let request_state = open_http_requests.get(&handle).unwrap_or_else(|| {
                    panic!("No matching HTTP request is associated with resource handle");
                });
                let begin_idx = open_function_table.get(&request_state.root_handle).unwrap_or_else(|| {
                    panic!("No matching BeginRemoteWrite index was found for the open HTTP request");
                });
                let request = request_state.request.clone();

                let durability = Durability::<
                    Option<Result<Result<Option<HashMap<String, Vec<u8>>>, SerializableErrorCode>, ()>>,
                    SerializableError,
                >::new(
                    "golem http::types::future_trailers",
                    "get",
                    DurableFunctionType::WriteRemoteBatched(Some(*begin_idx)),
                );

                if durability.is_live() {
                    let result = self.trailers.get();
                    let (to_serialize, result) = match &result {
                        Some(Ok(Ok(None))) => (Some(Ok(Ok(None))), Some(Ok(Ok(None)))),
                        Some(Ok(Ok(Some(trailers)))) => {
                            let mut serialized_trailers = HashMap::new();

                            for (key, value) in trailers.get_fields()? {
                                serialized_trailers
                                    .insert(key.as_str().to_string(), value.as_bytes().to_vec());
                            }

                            let trailers = Fields::new(WrappedFields { fields });
                            (
                                Some(Ok(Ok(Some(serialized_trailers)))),
                                Some(Ok(Ok(Some(trailers))))
                            )
                        }
                        Some(Ok(Err(error_code))) => (Some(Ok(Err(error_code.into()))), Some(Ok(Err(error_code)))),
                        Some(Err(_)) => (Some(Err(())), Some(Err(()))),
                        None => (None, None),
                    };
                    let _ = durability.persist_serializable(request, &Ok(to_serialize));
                    result
                } else {
                    let serialized = durability.replay_serializable();
                    match serialized {
                        Ok(Some(Ok(Ok(None)))) => Some(Ok(Ok(None))),
                        Ok(Some(Ok(Ok(Some(serialized_trailers))))) => {
                            let mut fields = crate::bindings::wasi::http::types::Fields::new();
                            for (key, value) in serialized_trailers {
                                fields.append(key, value)?;
                            }

                            let fields = Fields::new(WrappedFields { fields });
                            Some(Ok(Ok(Some(fields))))
                        }
                        Ok(Some(Ok(Err(error_code)))) => Some(Ok(Err(error_code.into()))),
                        Ok(Some(Err(_))) => Some(Err(())),
                        Ok(None) => None,
                        Err(error) => {
                            panic!("Error replaying FutureTrailers::get: {error}");
                        }
                    }
                }
            })
        })
    }
}

impl Drop for WrappedFutureTrailers {
    fn drop(&mut self) {
        observe_function_call("http::types::future_trailers", "drop");
    }
}

struct WrappedOutgoingResponse {
    response: crate::bindings::wasi::http::types::OutgoingResponse,
}

impl crate::bindings::exports::wasi::http::types::GuestOutgoingResponse
    for WrappedOutgoingResponse
{
    fn new(headers: Headers) -> Self {
        observe_function_call("http::types::outgoing_response", "new");
        let headers = headers.into_inner::<WrappedFields>().fields;
        let response = crate::bindings::wasi::http::types::OutgoingResponse::new(headers);
        WrappedOutgoingResponse { response }
    }

    fn status_code(&self) -> StatusCode {
        observe_function_call("http::types::outgoing_response", "status_code");
        let status_code = self.response.status_code();
        status_code
    }

    fn set_status_code(&self, status_code: StatusCode) -> Result<(), ()> {
        observe_function_call("http::types::outgoing_response", "set_status_code");
        Ok(self.response.set_status_code(status_code)?)
    }

    fn headers(&self) -> Headers {
        observe_function_call("http::types::outgoing_response", "headers");
        let headers = self.response.headers();
        Headers::new(WrappedFields { fields: headers })
    }

    fn body(&self) -> Result<OutgoingBody, ()> {
        observe_function_call("http::types::outgoing_response", "body");
        let body = self.response.body()?;
        Ok(OutgoingBody::new(WrappedOutgoingBody { body }))
    }
}

impl Drop for WrappedOutgoingResponse {
    fn drop(&mut self) {
        observe_function_call("http::types::outgoing_response", "drop");
    }
}

struct WrappedOutgoingBody {
    body: crate::bindings::wasi::http::types::OutgoingBody,
}

impl crate::bindings::exports::wasi::http::types::GuestOutgoingBody for WrappedOutgoingBody {
    fn write(&self) -> Result<OutputStream, ()> {
        observe_function_call("http::types::outgoing_body", "write");
        let stream = self.body.write()?;
        Ok(OutputStream::new(WrappedOutputStream {
            output_stream: stream,
        }))
    }

    fn finish(this: OutgoingBody, trailers: Option<Trailers>) -> Result<(), ErrorCode> {
        observe_function_call("http::types::outgoing_body", "finish");
        let this = this.into_inner::<WrappedOutgoingBody>().body;
        let trailers = trailers.map(|t| t.into_inner::<WrappedFields>().fields);
        Ok(crate::bindings::wasi::http::types::OutgoingBody::finish(
            this, trailers,
        )?)
    }
}

impl Drop for WrappedOutgoingBody {
    fn drop(&mut self) {
        observe_function_call("http::types::outgoing_body", "drop");
    }
}

struct WrappedFutureIncomingResponse {
    response: crate::bindings::wasi::http::types::FutureIncomingResponse,
}

impl crate::bindings::exports::wasi::http::types::GuestFutureIncomingResponse
    for WrappedFutureIncomingResponse
{
    fn subscribe(&self) -> Pollable {
        observe_function_call("http::types::future_incoming_response", "subscribe");
        // In replay mode the future is in Deferred state for which the built-in Subscribe implementation immediately returns.
        // This is exactly what we want for replay mode. In live mode the future is in Pending state until the response is
        // available, and the returned Pollable will wait for the request task to finish.
        let pollable = self.response.subscribe();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }

    fn get(&self) -> Option<Result<Result<IncomingResponse, ErrorCode>, ()>> {
        observe_function_call("http::types::future_incoming_response", "get");
        // Each get call is stored in the oplog. If the result was Error or None (future is pending), we just
        // continue the replay. If the result was Ok, we return register the stored response to the table as a new
        // HostIncomingResponse and return its reference.
        // In live mode the underlying implementation is either polling the response future, or, if it was Deferred
        // (when the request was initiated in replay mode), it starts executing the deferred request and returns None.
        //
        // Note that the response body is streaming, so at this point we don't have it in memory. Each chunk read from
        // the body is stored in the oplog, so we can replay it later. In replay mode we initialize the body with a
        // fake stream which can only be read in the oplog, and fails if we try to read it in live mode.

        let handle = self.response.handle();
        let durable_execution_state = current_durable_execution_state();
        if durable_execution_state.is_live
            || matches!(
                durable_execution_state.persistence_level,
                PersistenceLevel::PersistNothing
            )
        {
            OPEN_HTTP_REQUESTS.with_borrow(|open_http_requests| {
                OPEN_FUNCTION_TABLE.with_borrow(|open_function_table| {
                    let request_state = open_http_requests.get(&handle).unwrap_or_else(|| {
                        panic!("No matching HTTP request is associated with resource handle")
                    });

                    let begin_idx = *open_function_table
                        .get(&request_state.root_handle)
                        .unwrap_or_else(|| {
                            panic!(
                                "No matching BeginRemoteWrite index was found for the open HTTP request"
                            )
                        });

                    let request = request_state.request.clone();
                    let response = self.response.get();

                    let (serializable_response, wrapped_response) = match response {
                        None => (SerializableResponse::Pending, None),
                        Some(Ok(Ok(incoming_response))) => {
                            let mut result = SerializableResponseHeaders { status: incoming_response.status(), headers: HashMap::new() };
                            let headers = incoming_response.headers();
                            for (key, value) in headers.entries() {
                                result.headers.insert(key, value);
                            }

                            (SerializableResponse::HeadersReceived(result), Some(Ok(Ok(IncomingResponse::new(WrappedIncomingResponse::proxied(incoming_response))))))
                        }
                        Some(Err(_)) => (SerializableResponse::InternalError(None), Some(Err(()))),
                        Some(Ok(Err(error_code))) => {
                            (SerializableResponse::HttpError(error_code.clone().into()), Some(Ok(Err(error_code.into()))))
                        }
                    };

                    if !matches!(durable_execution_state.persistence_level, PersistenceLevel::PersistNothing) {
                        let serialized_request = serialize(&request).unwrap_or_else(|err| {
                            panic!("failed to serialize input ({input:?}) for persisting durable function invocation: {err}")
                        }).to_vec();
                        let serialized_response = serialize(&serializable_response).unwrap_or_else(|err| {
                            panic!("failed to serialize result ({result:?}) for persisting durable function invocation: {err}")
                        }).to_vec();

                        persist_durable_function_invocation(
                            "http::types::future_incoming_response::get",
                            &serialized_request,
                            &serialized_response,
                            DurableFunctionType::WriteRemoteBatched(Some(begin_idx)),
                        );
                    }

                    if !matches!(serializable_response, SerializableResponse::Pending) {
                        if let Some(Ok(Ok(resource))) = &response {
                            let incoming_response_handle = resource.handle();
                            continue_http_request(
                                handle,
                                incoming_response_handle,
                                HttpRequestCloseOwner::IncomingResponseDrop,
                            );
                        }
                    }

                    wrapped_response
                })
            })
        } else {
            let oplog_entry = read_persisted_durable_function_invocation();

            let serialized_response: SerializableResponse = try_deserialize(&oplog_entry.response)
                .unwrap_or_else(|err| panic!("Unexpected ImportedFunctionInvoked payload: {err}"))
                .expect("Payload is empty");

            match serialized_response {
                SerializableResponse::Pending => None,
                SerializableResponse::HeadersReceived(serializable_response_headers) => {
                    let incoming_response = IncomingResponse::new(
                        WrappedIncomingResponse::replayed(serializable_response_headers),
                    );

                    continue_http_request(
                        handle,
                        incoming_response.handle(),
                        HttpRequestCloseOwner::IncomingResponseDrop,
                    );

                    Some(Ok(Ok(incoming_response)))
                }
                SerializableResponse::InternalError(None) => Some(Err(())),
                SerializableResponse::InternalError(Some(serializable_error)) => {
                    panic!("Unexpected error in replayed response: {serializable_error}")
                }
                SerializableResponse::HttpError(error_code) => Some(Ok(Err(error_code.into()))),
            }
        }
    }
}

impl Drop for WrappedFutureIncomingResponse {
    fn drop(&mut self) {
        observe_function_call("http::types::future_incoming_response", "drop");

        OPEN_HTTP_REQUESTS.with_borrow_mut(|open_http_requests| {
            let handle = self.response.handle();
            if let Some(state) = open_http_requests.get(&handle) {
                if state.close_owner == HttpRequestCloseOwner::FutureIncomingResponseDrop {
                    end_http_request(handle);
                }
            }
        });
    }
}

impl crate::bindings::exports::wasi::http::types::Guest for crate::Component {
    type Fields = WrappedFields;
    type IncomingRequest = WrappedIncomingRequest;
    type OutgoingRequest = WrappedOutgoingRequest;
    type RequestOptions = WrappedRequestOptions;
    type ResponseOutparam = WrappedResponseOutparam;
    type IncomingResponse = WrappedIncomingResponse;
    type IncomingBody = WrappedIncomingBody;
    type FutureTrailers = WrappedFutureTrailers;
    type OutgoingResponse = WrappedOutgoingResponse;
    type OutgoingBody = WrappedOutgoingBody;
    type FutureIncomingResponse = WrappedFutureIncomingResponse;

    fn http_error_code(err: IoErrorBorrow<'_>) -> Option<ErrorCode> {
        observe_function_call("http::types", "http_error_code");
        let err = &err.get::<WrappedError>().error;
        let error_code = http_error_code(err);
        let error_code = unsafe { transmute(error_code) };
        error_code
    }
}
