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

use crate::durable_host::http::{continue_http_request, end_http_request};
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, HttpRequestCloseOwner};
use crate::get_oplog_entry;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::HasWorker;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use desert_rust::BinaryCodec;
use golem_common::model::oplog::host_functions::{
    HttpTypesFutureIncomingResponseGet, HttpTypesFutureTrailersGet,
};
use golem_common::model::oplog::types::{SerializableHttpResponse, SerializableResponseHeaders};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse,
    HostResponseHttpFutureTrailersGet, HostResponseHttpResponse, OplogEntry, PersistenceLevel,
};
use golem_common::model::ScheduleId;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_wasm_derive::{FromValue, IntoValue};
use http::{HeaderName, HeaderValue};
use std::collections::HashMap;
use std::str::FromStr;
use wasmtime::component::Resource;
use wasmtime_wasi_http::bindings::http::types::{
    Duration, ErrorCode, FieldKey, FieldValue, Fields, FutureIncomingResponse, FutureTrailers,
    HeaderError, Headers, Host, HostFields, HostFutureIncomingResponse, HostFutureTrailers,
    HostIncomingBody, HostIncomingRequest, HostIncomingResponse, HostOutgoingBody,
    HostOutgoingRequest, HostOutgoingResponse, HostRequestOptions, HostResponseOutparam,
    IncomingBody, IncomingRequest, IncomingResponse, InputStream, IoError, Method, OutgoingBody,
    OutgoingRequest, OutgoingResponse, OutputStream, Pollable, RequestOptions, ResponseOutparam,
    Scheme, StatusCode, Trailers,
};
use wasmtime_wasi_http::get_fields;
use wasmtime_wasi_http::types::FieldMap;
use wasmtime_wasi_http::{HttpError, HttpResult};

impl<Ctx: WorkerCtx> HostFields for DurableWorkerCtx<Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<Fields>> {
        self.observe_function_call("http::types::fields", "new");
        HostFields::new(&mut self.as_wasi_http_view())
    }

    fn from_list(
        &mut self,
        entries: Vec<(FieldKey, FieldValue)>,
    ) -> wasmtime::Result<Result<Resource<Fields>, HeaderError>> {
        self.observe_function_call("http::types::fields", "from_list");
        HostFields::from_list(&mut self.as_wasi_http_view(), entries)
    }

    fn get(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
    ) -> wasmtime::Result<Vec<FieldValue>> {
        self.observe_function_call("http::types::fields", "get");
        HostFields::get(&mut self.as_wasi_http_view(), self_, name)
    }

    fn has(&mut self, self_: Resource<Fields>, name: FieldKey) -> wasmtime::Result<bool> {
        self.observe_function_call("http::types::fields", "has");
        HostFields::has(&mut self.as_wasi_http_view(), self_, name)
    }

    fn set(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
        value: Vec<FieldValue>,
    ) -> wasmtime::Result<Result<(), HeaderError>> {
        self.observe_function_call("http::types::fields", "set");
        HostFields::set(&mut self.as_wasi_http_view(), self_, name, value)
    }

    fn delete(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
    ) -> wasmtime::Result<Result<(), HeaderError>> {
        self.observe_function_call("http::types::fields", "delete");
        HostFields::delete(&mut self.as_wasi_http_view(), self_, name)
    }

    fn append(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
        value: FieldValue,
    ) -> wasmtime::Result<Result<(), HeaderError>> {
        self.observe_function_call("http::types::fields", "append");
        HostFields::append(&mut self.as_wasi_http_view(), self_, name, value)
    }

    fn entries(
        &mut self,
        self_: Resource<Fields>,
    ) -> wasmtime::Result<Vec<(FieldKey, FieldValue)>> {
        self.observe_function_call("http::types::fields", "entries");
        HostFields::entries(&mut self.as_wasi_http_view(), self_)
    }

    fn clone(&mut self, self_: Resource<Fields>) -> wasmtime::Result<Resource<Fields>> {
        self.observe_function_call("http::types::fields", "clone");
        HostFields::clone(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<Fields>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::fields", "drop");
        HostFields::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostIncomingRequest for DurableWorkerCtx<Ctx> {
    fn method(&mut self, self_: Resource<IncomingRequest>) -> wasmtime::Result<Method> {
        self.observe_function_call("http::types::incoming_request", "method");
        HostIncomingRequest::method(&mut self.as_wasi_http_view(), self_)
    }

    fn path_with_query(
        &mut self,
        self_: Resource<IncomingRequest>,
    ) -> wasmtime::Result<Option<String>> {
        self.observe_function_call("http::types::incoming_request", "path_with_query");
        HostIncomingRequest::path_with_query(&mut self.as_wasi_http_view(), self_)
    }

    fn scheme(&mut self, self_: Resource<IncomingRequest>) -> wasmtime::Result<Option<Scheme>> {
        self.observe_function_call("http::types::incoming_request", "scheme");
        HostIncomingRequest::scheme(&mut self.as_wasi_http_view(), self_)
    }

    fn authority(&mut self, self_: Resource<IncomingRequest>) -> wasmtime::Result<Option<String>> {
        self.observe_function_call("http::types::incoming_request", "authority");
        HostIncomingRequest::authority(&mut self.as_wasi_http_view(), self_)
    }

    fn headers(&mut self, self_: Resource<IncomingRequest>) -> wasmtime::Result<Resource<Headers>> {
        self.observe_function_call("http::types::incoming_request", "headers");
        HostIncomingRequest::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn consume(
        &mut self,
        self_: Resource<IncomingRequest>,
    ) -> wasmtime::Result<Result<Resource<IncomingBody>, ()>> {
        self.observe_function_call("http::types::incoming_request", "consume");
        HostIncomingRequest::consume(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<IncomingRequest>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::incoming_request", "drop");
        HostIncomingRequest::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostOutgoingRequest for DurableWorkerCtx<Ctx> {
    fn new(&mut self, headers: Resource<Headers>) -> wasmtime::Result<Resource<OutgoingRequest>> {
        self.observe_function_call("http::types::outgoing_request", "new");
        HostOutgoingRequest::new(&mut self.as_wasi_http_view(), headers)
    }

    fn body(
        &mut self,
        self_: Resource<OutgoingRequest>,
    ) -> wasmtime::Result<Result<Resource<OutgoingBody>, ()>> {
        self.observe_function_call("http::types::outgoing_request", "body");
        HostOutgoingRequest::body(&mut self.as_wasi_http_view(), self_)
    }

    fn method(&mut self, self_: Resource<OutgoingRequest>) -> wasmtime::Result<Method> {
        self.observe_function_call("http::types::outgoing_request", "method");
        HostOutgoingRequest::method(&mut self.as_wasi_http_view(), self_)
    }

    fn set_method(
        &mut self,
        self_: Resource<OutgoingRequest>,
        method: Method,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call("http::types::outgoing_request", "set_method");
        HostOutgoingRequest::set_method(&mut self.as_wasi_http_view(), self_, method)
    }

    fn path_with_query(
        &mut self,
        self_: Resource<OutgoingRequest>,
    ) -> wasmtime::Result<Option<String>> {
        self.observe_function_call("http::types::outgoing_request", "path_with_query");
        HostOutgoingRequest::path_with_query(&mut self.as_wasi_http_view(), self_)
    }

    fn set_path_with_query(
        &mut self,
        self_: Resource<OutgoingRequest>,
        path_with_query: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call("http::types::outgoing_request", "set_path_with_query");
        HostOutgoingRequest::set_path_with_query(
            &mut self.as_wasi_http_view(),
            self_,
            path_with_query,
        )
    }

    fn scheme(&mut self, self_: Resource<OutgoingRequest>) -> wasmtime::Result<Option<Scheme>> {
        self.observe_function_call("http::types::outgoing_request", "scheme");
        HostOutgoingRequest::scheme(&mut self.as_wasi_http_view(), self_)
    }

    fn set_scheme(
        &mut self,
        self_: Resource<OutgoingRequest>,
        scheme: Option<Scheme>,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call("http::types::outgoing_request", "set_scheme");
        HostOutgoingRequest::set_scheme(&mut self.as_wasi_http_view(), self_, scheme)
    }

    fn authority(&mut self, self_: Resource<OutgoingRequest>) -> wasmtime::Result<Option<String>> {
        self.observe_function_call("http::types::outgoing_request", "authority");
        HostOutgoingRequest::authority(&mut self.as_wasi_http_view(), self_)
    }

    fn set_authority(
        &mut self,
        self_: Resource<OutgoingRequest>,
        authority: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call("http::types::outgoing_request", "set_authority");
        HostOutgoingRequest::set_authority(&mut self.as_wasi_http_view(), self_, authority)
    }

    fn headers(&mut self, self_: Resource<OutgoingRequest>) -> wasmtime::Result<Resource<Headers>> {
        self.observe_function_call("http::types::outgoing_request", "headers");
        HostOutgoingRequest::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<OutgoingRequest>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::outgoing_request", "drop");
        HostOutgoingRequest::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostRequestOptions for DurableWorkerCtx<Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<RequestOptions>> {
        self.observe_function_call("http::types::request_options", "new");
        HostRequestOptions::new(&mut self.as_wasi_http_view())
    }

    fn connect_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        self.observe_function_call("http::types::request_options", "connect_timeout_ms");
        HostRequestOptions::connect_timeout(&mut self.as_wasi_http_view(), self_)
    }

    fn set_connect_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
        ms: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call("http::types::request_options", "set_connect_timeout_ms");
        HostRequestOptions::set_connect_timeout(&mut self.as_wasi_http_view(), self_, ms)
    }

    fn first_byte_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        self.observe_function_call("http::types::request_options", "first_byte_timeout_ms");
        HostRequestOptions::first_byte_timeout(&mut self.as_wasi_http_view(), self_)
    }

    fn set_first_byte_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
        ms: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call("http::types::request_options", "set_first_byte_timeout_ms");
        HostRequestOptions::set_first_byte_timeout(&mut self.as_wasi_http_view(), self_, ms)
    }

    fn between_bytes_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        self.observe_function_call("http::types::request_options", "between_bytes_timeout_ms");
        HostRequestOptions::between_bytes_timeout(&mut self.as_wasi_http_view(), self_)
    }

    fn set_between_bytes_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
        ms: Option<Duration>,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call(
            "http::types::request_options",
            "set_between_bytes_timeout_ms",
        );
        HostRequestOptions::set_between_bytes_timeout(&mut self.as_wasi_http_view(), self_, ms)
    }

    fn drop(&mut self, rep: Resource<RequestOptions>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::request_options", "drop");
        HostRequestOptions::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostResponseOutparam for DurableWorkerCtx<Ctx> {
    fn set(
        &mut self,
        param: Resource<ResponseOutparam>,
        response: Result<Resource<OutgoingResponse>, ErrorCode>,
    ) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::response_outparam", "set");
        HostResponseOutparam::set(&mut self.as_wasi_http_view(), param, response)
    }

    fn send_informational(
        &mut self,
        id: Resource<ResponseOutparam>,
        status: u16,
        headers: Resource<Fields>,
    ) -> HttpResult<()> {
        self.observe_function_call("http::types::response_outparam", "send_informational");
        HostResponseOutparam::send_informational(&mut self.as_wasi_http_view(), id, status, headers)
    }

    fn drop(&mut self, rep: Resource<ResponseOutparam>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::response_outparam", "drop");
        HostResponseOutparam::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostIncomingResponse for DurableWorkerCtx<Ctx> {
    fn status(&mut self, self_: Resource<IncomingResponse>) -> wasmtime::Result<StatusCode> {
        self.observe_function_call("http::types::incoming_response", "status");
        HostIncomingResponse::status(&mut self.as_wasi_http_view(), self_)
    }

    fn headers(
        &mut self,
        self_: Resource<IncomingResponse>,
    ) -> wasmtime::Result<Resource<Headers>> {
        self.observe_function_call("http::types::incoming_response", "headers");
        HostIncomingResponse::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn consume(
        &mut self,
        self_: Resource<IncomingResponse>,
    ) -> wasmtime::Result<Result<Resource<IncomingBody>, ()>> {
        self.observe_function_call("http::types::incoming_response", "consume");
        let handle = self_.rep();
        let result = HostIncomingResponse::consume(&mut self.as_wasi_http_view(), self_);

        if let Ok(Ok(resource)) = &result {
            let incoming_body_handle = resource.rep();
            continue_http_request(
                self,
                handle,
                incoming_body_handle,
                HttpRequestCloseOwner::IncomingBodyDropOrFinish,
            );
        }

        result
    }

    async fn drop(&mut self, rep: Resource<IncomingResponse>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::incoming_response", "drop");

        let handle = rep.rep();
        if let Some(state) = self.state.open_http_requests.get(&handle) {
            if state.close_owner == HttpRequestCloseOwner::IncomingResponseDrop {
                end_http_request(self, handle).await?;
            }
        }

        HostIncomingResponse::drop(&mut self.as_wasi_http_view(), rep).await
    }
}

impl<Ctx: WorkerCtx> HostIncomingBody for DurableWorkerCtx<Ctx> {
    fn stream(
        &mut self,
        self_: Resource<IncomingBody>,
    ) -> wasmtime::Result<Result<Resource<InputStream>, ()>> {
        self.observe_function_call("http::types::incoming_body", "stream");

        let handle = self_.rep();
        let result = HostIncomingBody::stream(&mut self.as_wasi_http_view(), self_);

        if let Ok(Ok(resource)) = &result {
            let stream_handle = resource.rep();
            continue_http_request(
                self,
                handle,
                stream_handle,
                HttpRequestCloseOwner::InputStreamClosed,
            );
        }

        result
    }

    async fn finish(
        &mut self,
        this: Resource<IncomingBody>,
    ) -> wasmtime::Result<Resource<FutureTrailers>> {
        self.observe_function_call("http::types::incoming_body", "finish");

        let handle = this.rep();
        if let Some(state) = self.state.open_http_requests.get(&handle) {
            if state.close_owner == HttpRequestCloseOwner::IncomingBodyDropOrFinish {
                end_http_request(self, handle).await?;
            }
        }

        HostIncomingBody::finish(&mut self.as_wasi_http_view(), this).await
    }

    async fn drop(&mut self, rep: Resource<IncomingBody>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::incoming_body", "drop");

        let handle = rep.rep();
        if let Some(state) = self.state.open_http_requests.get(&handle) {
            if state.close_owner == HttpRequestCloseOwner::IncomingBodyDropOrFinish {
                end_http_request(self, handle).await?;
            }
        }

        HostIncomingBody::drop(&mut self.as_wasi_http_view(), rep).await
    }
}

impl<Ctx: WorkerCtx> HostFutureTrailers for DurableWorkerCtx<Ctx> {
    fn subscribe(
        &mut self,
        self_: Resource<FutureTrailers>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("http::types::future_trailers", "subscribe");
        HostFutureTrailers::subscribe(&mut self.as_wasi_http_view(), self_)
    }

    async fn get(
        &mut self,
        self_: Resource<FutureTrailers>,
    ) -> wasmtime::Result<Option<Result<Result<Option<Resource<Trailers>>, ErrorCode>, ()>>> {
        // Trailers might be associated with an incoming http request or an http response.
        // Only in the second case do we need to add durability. We can distinguish these
        // two cases by checking for the presence of an associated open http request.
        if let Some(request_state) = self.state.open_http_requests.get(&self_.rep()) {
            let request = request_state.request.clone();

            let durability = Durability::<HttpTypesFutureTrailersGet>::new(
                self,
                DurableFunctionType::WriteRemoteBatched(Some(request_state.begin_index)),
            )
            .await
            .map_err(wasmtime::Error::from)?;

            if durability.is_live() {
                let result = HostFutureTrailers::get(&mut self.as_wasi_http_view(), self_).await;
                let (to_serialize, for_retry) = match &result {
                    Ok(Some(Ok(Ok(None)))) => (Ok(Some(Ok(Ok(None)))), Ok(())),
                    Ok(Some(Ok(Ok(Some(trailers))))) => {
                        let mut serialized_trailers = HashMap::new();

                        for (key, value) in get_fields(self.table(), trailers)?.as_ref().iter() {
                            serialized_trailers
                                .insert(key.as_str().to_string(), value.as_bytes().to_vec());
                        }
                        (Ok(Some(Ok(Ok(Some(serialized_trailers))))), Ok(()))
                    }
                    Ok(Some(Ok(Err(error_code)))) => (
                        Ok(Some(Ok(Err(error_code.into())))),
                        Err(error_code.to_string()),
                    ),
                    Ok(Some(Err(_))) => (Ok(Some(Err(()))), Err("Unknown error".to_string())),
                    Ok(None) => (Ok(None), Ok(())),
                    Err(err) => (Err(err.to_string()), Err(err.to_string())),
                };
                durability
                    .try_trigger_retry(self, &for_retry)
                    .await
                    .map_err(wasmtime::Error::from_anyhow)?;
                let _ = durability
                    .persist(
                        self,
                        request,
                        HostResponseHttpFutureTrailersGet {
                            result: to_serialize,
                        },
                    )
                    .await
                    .map_err(wasmtime::Error::from)?;
                result
            } else {
                let serialized: HostResponseHttpFutureTrailersGet =
                    durability
                        .replay(self)
                        .await
                        .map_err(wasmtime::Error::from)?;
                match serialized.result {
                    Ok(Some(Ok(Ok(None)))) => Ok(Some(Ok(Ok(None)))),
                    Ok(Some(Ok(Ok(Some(serialized_trailers))))) => {
                        let mut header_map = http::HeaderMap::new();
                        for (key, value) in serialized_trailers {
                            header_map
                                .insert(HeaderName::from_str(&key)?, HeaderValue::try_from(value)?);
                        }
                        let field_size_limit = {
                            let mut view = self.as_wasi_http_view();
                            use wasmtime_wasi_http::types::WasiHttpView;
                            view.ctx().field_size_limit
                        };
                        let fields = FieldMap::new(header_map, field_size_limit);
                        let hdrs = self
                            .table()
                            .push(wasmtime_wasi_http::types::HostFields::Owned { fields })?;
                        Ok(Some(Ok(Ok(Some(hdrs)))))
                    }
                    Ok(Some(Ok(Err(error_code)))) => Ok(Some(Ok(Err(error_code.into())))),
                    Ok(Some(Err(_))) => Ok(Some(Err(()))),
                    Ok(None) => Ok(None),
                    Err(error) => Err(wasmtime::Error::msg(error)),
                }
            }
        } else {
            self.observe_function_call("http::types::future_trailers", "get");
            HostFutureTrailers::get(&mut self.as_wasi_http_view(), self_).await
        }
    }

    fn drop(&mut self, rep: Resource<FutureTrailers>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::future_trailers", "drop");
        HostFutureTrailers::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostOutgoingResponse for DurableWorkerCtx<Ctx> {
    fn new(&mut self, headers: Resource<Headers>) -> wasmtime::Result<Resource<OutgoingResponse>> {
        self.observe_function_call("http::types::outgoing_response", "new");
        HostOutgoingResponse::new(&mut self.as_wasi_http_view(), headers)
    }

    fn status_code(&mut self, self_: Resource<OutgoingResponse>) -> wasmtime::Result<StatusCode> {
        self.observe_function_call("http::types::outgoing_response", "status_code");
        HostOutgoingResponse::status_code(&mut self.as_wasi_http_view(), self_)
    }

    fn set_status_code(
        &mut self,
        self_: Resource<OutgoingResponse>,
        status_code: StatusCode,
    ) -> wasmtime::Result<Result<(), ()>> {
        self.observe_function_call("http::types::outgoing_response", "set_status_code");
        HostOutgoingResponse::set_status_code(&mut self.as_wasi_http_view(), self_, status_code)
    }

    fn headers(
        &mut self,
        self_: Resource<OutgoingResponse>,
    ) -> wasmtime::Result<Resource<Headers>> {
        self.observe_function_call("http::types::outgoing_response", "headers");
        HostOutgoingResponse::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn body(
        &mut self,
        self_: Resource<OutgoingResponse>,
    ) -> wasmtime::Result<Result<Resource<OutgoingBody>, ()>> {
        self.observe_function_call("http::types::outgoing_response", "body");
        HostOutgoingResponse::body(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<OutgoingResponse>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::outgoing_response", "drop");
        HostOutgoingResponse::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostOutgoingBody for DurableWorkerCtx<Ctx> {
    fn write(
        &mut self,
        self_: Resource<OutgoingBody>,
    ) -> wasmtime::Result<Result<Resource<OutputStream>, ()>> {
        self.observe_function_call("http::types::outgoing_body", "write");
        HostOutgoingBody::write(&mut self.as_wasi_http_view(), self_)
    }

    fn finish(
        &mut self,
        this: Resource<OutgoingBody>,
        trailers: Option<Resource<Trailers>>,
    ) -> HttpResult<()> {
        self.observe_function_call("http::types::outgoing_body", "finish");
        HostOutgoingBody::finish(&mut self.as_wasi_http_view(), this, trailers)
    }

    fn drop(&mut self, rep: Resource<OutgoingBody>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::outgoing_body", "drop");
        HostOutgoingBody::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostFutureIncomingResponse for DurableWorkerCtx<Ctx> {
    fn subscribe(
        &mut self,
        self_: Resource<FutureIncomingResponse>,
    ) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("http::types::future_incoming_response", "subscribe");
        // In replay mode the future is in Deferred state for which the built-in Subscribe implementation immediately returns.
        // This is exactly what we want for replay mode. In live mode the future is in Pending state until the response is
        // available, and the returned Pollable will wait for the request task to finish.
        HostFutureIncomingResponse::subscribe(&mut self.as_wasi_http_view(), self_)
    }

    async fn get(
        &mut self,
        self_: Resource<FutureIncomingResponse>,
    ) -> wasmtime::Result<Option<Result<Result<Resource<IncomingResponse>, ErrorCode>, ()>>> {
        self.observe_function_call("http::types::future_incoming_response", "get");
        // Each get call is stored in the oplog. If the result was Error or None (future is pending), we just
        // continue the replay. If the result was Ok, we return register the stored response to the table as a new
        // HostIncomingResponse and return its reference.
        // In live mode the underlying implementation is either polling the response future, or, if it was Deferred
        // (when the request was initiated in replay mode), it starts executing the deferred request and returns None.
        //
        // Note that the response body is streaming, so at this point we don't have it in memory. Each chunk read from
        // the body is stored in the oplog, so we can replay it later. In replay mode we initialize the body with a
        // fake stream which can only be read in the oplog, and fails if we try to read it in live mode.
        let handle = self_.rep();
        let durable_execution_state = self.durable_execution_state();
        if durable_execution_state.is_live || self.state.snapshotting_mode.is_some() {
            let request_state = self.state.open_http_requests.get(&handle).ok_or_else(|| {
                wasmtime::Error::msg("No matching HTTP request is associated with resource handle")
            })?;

            let request = request_state.request.clone();
            let begin_index = request_state.begin_index;

            let response =
                HostFutureIncomingResponse::get(&mut self.as_wasi_http_view(), self_).await;

            let (serializable_response, for_retry) = match &response {
                Ok(None) => (SerializableHttpResponse::Pending, Ok(())),
                Ok(Some(Ok(Ok(resource)))) => {
                    let incoming_response = self.table().get(resource)?;
                    (
                        SerializableHttpResponse::HeadersReceived(
                            SerializableResponseHeaders::try_from(incoming_response)
                                .map_err(wasmtime::Error::from_anyhow)?,
                        ),
                        Ok(()),
                    )
                }
                Ok(Some(Err(_))) => (
                    SerializableHttpResponse::InternalError(None),
                    Err("Unknown error".to_string()),
                ),
                Ok(Some(Ok(Err(error_code)))) => (
                    SerializableHttpResponse::HttpError(error_code.clone().into()),
                    Err(error_code.to_string()),
                ),
                Err(err) => (
                    SerializableHttpResponse::InternalError(Some(err.to_string())),
                    Err(err.to_string()),
                ),
            };

            if let Err(err) = for_retry {
                self.state.current_retry_point = begin_index;
                self.try_trigger_retry(anyhow!(err))
                    .await
                    .map_err(wasmtime::Error::from_anyhow)?;
            }

            let is_pending = matches!(serializable_response, SerializableHttpResponse::Pending);
            if self.state.snapshotting_mode.is_none() {
                self.state
                    .oplog
                    .add_host_call(
                        HttpTypesFutureIncomingResponseGet::HOST_FUNCTION_NAME,
                        &HostRequest::HttpRequest(request),
                        &HostResponse::HttpResponse(HostResponseHttpResponse {
                            response: serializable_response,
                        }),
                        DurableFunctionType::WriteRemoteBatched(Some(begin_index)),
                    )
                    .await
                    .unwrap_or_else(|err| panic!("failed to serialize http response: {err}"));
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await;
            }

            if !is_pending {
                if let Ok(Some(Ok(Ok(resource)))) = &response {
                    let incoming_response_handle = resource.rep();
                    continue_http_request(
                        self,
                        handle,
                        incoming_response_handle,
                        HttpRequestCloseOwner::IncomingResponseDrop,
                    );
                }
            }

            response
        } else if durable_execution_state.persistence_level == PersistenceLevel::PersistNothing {
            Err(WorkerExecutorError::runtime(
                "Trying to replay an http request in a PersistNothing block",
            )
            .into())
        } else {
            let (_, oplog_entry) = get_oplog_entry!(self.state.replay_state, OplogEntry::HostCall).map_err(|golem_err| wasmtime::Error::msg(format!("failed to get http::types::future_incoming_response::get oplog entry: {golem_err}")))?;

            let serialized_response = match oplog_entry {
                OplogEntry::HostCall { response, .. } => {
                    let response = self
                        .state
                        .oplog
                        .download_payload(response)
                        .await
                        .unwrap_or_else(|err| {
                            panic!("failed to deserialize function response: {err}")
                        });
                    match response {
                        HostResponse::HttpResponse(response) => response.response,
                        other => panic!("unexpected oplog payload: {other:?}"),
                    }
                }
                other => panic!("unexpected oplog entry: {other:?}"),
            };

            match serialized_response {
                SerializableHttpResponse::Pending => Ok(None),
                SerializableHttpResponse::HeadersReceived(serializable_response_headers) => {
                    let incoming_response: wasmtime_wasi_http::types::HostIncomingResponse =
                        serializable_response_headers
                            .try_into()
                            .map_err(wasmtime::Error::from_anyhow)?;

                    let rep = self.table().push(incoming_response)?;
                    let incoming_response_handle = rep.rep();

                    continue_http_request(
                        self,
                        handle,
                        incoming_response_handle,
                        HttpRequestCloseOwner::IncomingResponseDrop,
                    );

                    Ok(Some(Ok(Ok(rep))))
                }
                SerializableHttpResponse::InternalError(None) => Ok(Some(Err(()))),
                SerializableHttpResponse::InternalError(Some(serializable_error)) => {
                    Err(wasmtime::Error::msg(serializable_error))
                }
                SerializableHttpResponse::HttpError(error_code) => {
                    Ok(Some(Ok(Err(error_code.into()))))
                }
            }
        }
    }

    async fn drop(&mut self, rep: Resource<FutureIncomingResponse>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::future_incoming_response", "drop");

        let handle = rep.rep();
        if let Some(state) = self.state.open_http_requests.get(&handle) {
            if state.close_owner == HttpRequestCloseOwner::FutureIncomingResponseDrop {
                end_http_request(self, handle).await?;
            }
        }

        HostFutureIncomingResponse::drop(&mut self.as_wasi_http_view(), rep).await
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn http_error_code(&mut self, err: Resource<IoError>) -> wasmtime::Result<Option<ErrorCode>> {
        self.observe_function_call("http::types", "http_error_code");
        Host::http_error_code(&mut self.as_wasi_http_view(), err)
    }

    fn convert_error_code(&mut self, err: HttpError) -> wasmtime::Result<ErrorCode> {
        self.observe_function_call("http::types", "convert_error_code");
        Host::convert_error_code(&mut self.as_wasi_http_view(), err)
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
#[wit_transparent]
pub struct SerializableScheduleId {
    pub data: Vec<u8>,
}

impl SerializableScheduleId {
    pub fn from_domain(schedule_id: &ScheduleId) -> Self {
        let data = golem_common::serialization::serialize(schedule_id)
            .unwrap()
            .to_vec();
        Self { data }
    }

    pub fn as_domain(&self) -> Result<ScheduleId, String> {
        golem_common::serialization::deserialize(&self.data)
    }
}
