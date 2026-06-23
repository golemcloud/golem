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

use crate::durable_host::HttpOutgoingBodyState;
use crate::durable_host::concurrent::{CallHandle, NotCancellable, Resolution};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind, InFunctionRetryHost};
use crate::durable_host::http::inline_retry::{
    StatusRetryOutcome, take_http_background_retry_fallback, try_status_code_retry,
};
use crate::durable_host::http::{continue_http_request, end_http_request};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, HttpRequestCloseOwner};
use crate::services::HasWorker;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::workerctx::WorkerCtx;
use golem_common::model::NamedRetryPolicy;
use golem_common::model::oplog::host_functions::{
    HttpTypesFutureIncomingResponseGet, HttpTypesFutureTrailersGet,
};
use golem_common::model::oplog::types::{SerializableHttpResponse, SerializableResponseHeaders};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequest, HostResponse,
    HostResponseHttpFutureTrailersGet, HostResponseHttpResponse, PersistenceLevel,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use http::{HeaderName, HeaderValue};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use tracing::warn;
use wasmtime::component::Resource;
use wasmtime_wasi_http::FieldMap;
use wasmtime_wasi_http::p2::bindings::http::types::{
    Duration, ErrorCode, FieldKey, FieldValue, Fields, FutureIncomingResponse, FutureTrailers,
    HeaderError as BindingsHeaderError, Headers, Host, HostFields, HostFutureIncomingResponse,
    HostFutureTrailers, HostIncomingBody, HostIncomingRequest, HostIncomingResponse,
    HostOutgoingBody, HostOutgoingRequest, HostOutgoingResponse, HostRequestOptions,
    HostResponseOutparam, IncomingBody, IncomingRequest, IncomingResponse, InputStream, IoError,
    Method, OutgoingBody, OutgoingRequest, OutgoingResponse, OutputStream, Pollable,
    RequestOptions, ResponseOutparam, Scheme, StatusCode, Trailers,
};
use wasmtime_wasi_http::p2::{HeaderError, HeaderResult, HttpError, HttpResult};

impl<Ctx: WorkerCtx> HostFields for DurableWorkerCtx<Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<Fields>> {
        self.observe_function_call("http::types::fields", "new");
        HostFields::new(&mut self.as_wasi_http_view())
    }

    fn from_list(
        &mut self,
        entries: Vec<(FieldKey, FieldValue)>,
    ) -> HeaderResult<Resource<Fields>> {
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
    ) -> HeaderResult<()> {
        self.observe_function_call("http::types::fields", "set");
        HostFields::set(&mut self.as_wasi_http_view(), self_, name, value)
    }

    fn delete(&mut self, self_: Resource<Fields>, name: FieldKey) -> HeaderResult<()> {
        self.observe_function_call("http::types::fields", "delete");
        HostFields::delete(&mut self.as_wasi_http_view(), self_, name)
    }

    fn append(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
        value: FieldValue,
    ) -> HeaderResult<()> {
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
        let request_rep = self_.rep();
        let result = HostOutgoingRequest::body(&mut self.as_wasi_http_view(), self_);
        if let Ok(Ok(ref body)) = result {
            let body_rep = body.rep();
            self.state
                .pending_http_outgoing_request_body
                .insert(request_rep, body_rep);
            self.state
                .pending_http_retry_eligibility
                .entry(request_rep)
                .or_default();
        }
        result
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
        let request_rep = rep.rep();
        self.state
            .pending_http_outgoing_request_body
            .remove(&request_rep);
        self.state
            .pending_http_retry_eligibility
            .remove(&request_rep);
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
        if let Some(state) = self.state.open_http_requests.get(&handle)
            && state.close_owner == HttpRequestCloseOwner::IncomingResponseDrop
        {
            end_http_request(self, handle).await?;
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
            // Record the body handle so that when the stream closes, we can
            // transfer tracking back to the body (enabling finish() to then
            // transfer to FutureTrailers for durable trailers handling).
            if let Some(state) = self.state.open_http_requests.get_mut(&stream_handle) {
                state.body_handle = Some(handle);
            }
        }

        result
    }

    async fn finish(
        &mut self,
        this: Resource<IncomingBody>,
    ) -> wasmtime::Result<Resource<FutureTrailers>> {
        self.observe_function_call("http::types::incoming_body", "finish");

        let handle = this.rep();
        let has_tracking = self.state.open_http_requests.contains_key(&handle);

        let result = HostIncomingBody::finish(&mut self.as_wasi_http_view(), this).await?;

        if has_tracking {
            let ft_handle = result.rep();
            continue_http_request(
                self,
                handle,
                ft_handle,
                HttpRequestCloseOwner::FutureTrailersDrop,
            );
        }

        Ok(result)
    }

    async fn drop(&mut self, rep: Resource<IncomingBody>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::incoming_body", "drop");

        let handle = rep.rep();
        if let Some(state) = self.state.open_http_requests.get(&handle)
            && state.close_owner == HttpRequestCloseOwner::IncomingBodyDropOrFinish
        {
            end_http_request(self, handle).await?;
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

            // `WriteRemoteBatched`: not re-executable on an incomplete `Start`, so replay never
            // yields `Incomplete` (it hard-errors instead) and the lone batched `Start` is recovered
            // by the surrounding durable scope.
            let mut call = CallHandle::<HttpTypesFutureTrailersGet, NotCancellable>::start(
                self,
                request,
                DurableFunctionType::WriteRemoteBatched(Some(request_state.begin_index)),
            )
            .await
            .map_err(wasmtime::Error::from)?;

            let handle = self_.rep();
            if call.is_live() {
                let result = HostFutureTrailers::get(&mut self.as_wasi_http_view(), self_).await;

                // The only fallible step while the `Start` is open is reading the trailers from the
                // resource table; on failure abandon the started call (leaving its `Start`
                // incomplete, never a `Cancelled`) and propagate.
                let trailers_serialized = match &result {
                    Ok(Some(Ok(Ok(Some(trailers))))) => {
                        let mut serialized_trailers: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
                        match self.table().get(trailers) {
                            Ok(trailers) => {
                                for (key, value) in trailers.iter() {
                                    serialized_trailers
                                        .entry(key.as_str().to_string())
                                        .or_default()
                                        .push(value.as_bytes().to_vec());
                                }
                                Some(serialized_trailers)
                            }
                            Err(err) => {
                                call.abandon_for_trap();
                                return Err(err.into());
                            }
                        }
                    }
                    _ => None,
                };

                let (to_serialize, for_retry) = match &result {
                    Ok(Some(Ok(Ok(None)))) => (Ok(Some(Ok(Ok(None)))), Ok(())),
                    Ok(Some(Ok(Ok(Some(_))))) => (Ok(Some(Ok(Ok(trailers_serialized)))), Ok(())),
                    Ok(Some(Ok(Err(error_code)))) => (
                        Ok(Some(Ok(Err(error_code.into())))),
                        Err(HttpFailure::ErrorCode(error_code.clone())),
                    ),
                    Ok(Some(Err(_))) => (
                        Ok(Some(Err(()))),
                        Err(HttpFailure::Other("Unknown error".to_string())),
                    ),
                    Ok(None) => (Ok(None), Ok(())),
                    Err(err) => (
                        Err(err.to_string()),
                        Err(HttpFailure::Other(err.to_string())),
                    ),
                };
                call.try_trigger_retry(self, &for_retry, |err| match err {
                    HttpFailure::ErrorCode(code) => classify_http_error_code(code),
                    HttpFailure::Other(_) => HostFailureKind::Transient,
                })
                .await
                .map_err(wasmtime::Error::from_anyhow)?;
                let _ = call
                    .complete(
                        self,
                        HostResponseHttpFutureTrailersGet {
                            result: to_serialize,
                        },
                    )
                    .await
                    .map_err(wasmtime::Error::from)?;

                // End the HTTP request when trailers have resolved (not pending)
                let is_resolved = !matches!(&result, Ok(None));
                if is_resolved {
                    end_http_request(self, handle).await?;
                }

                result
            } else {
                let serialized: HostResponseHttpFutureTrailersGet = call
                    .replay_expecting_completion(self)
                    .await
                    .map_err(wasmtime::Error::from)?;
                let result = match serialized.result {
                    Ok(Some(Ok(Ok(None)))) => Ok(Some(Ok(Ok(None)))),
                    Ok(Some(Ok(Ok(Some(serialized_trailers))))) => {
                        let mut header_map = http::HeaderMap::new();
                        for (key, values) in serialized_trailers {
                            let name = HeaderName::from_str(&key)?;
                            for value in values {
                                header_map.append(name.clone(), HeaderValue::try_from(value)?);
                            }
                        }
                        let fields = FieldMap::new_immutable(header_map);
                        let hdrs = self.table().push(fields)?;
                        Ok(Some(Ok(Ok(Some(hdrs)))))
                    }
                    Ok(Some(Ok(Err(error_code)))) => Ok(Some(Ok(Err(error_code.into())))),
                    Ok(Some(Err(_))) => Ok(Some(Err(()))),
                    Ok(None) => Ok(None),
                    Err(error) => Err(wasmtime::Error::msg(error)),
                };

                // End the HTTP request when trailers have resolved (not pending)
                let is_resolved = !matches!(&result, Ok(None));
                if is_resolved {
                    end_http_request(self, handle).await?;
                }

                result
            }
        } else {
            self.observe_function_call("http::types::future_trailers", "get");
            HostFutureTrailers::get(&mut self.as_wasi_http_view(), self_).await
        }
    }

    fn drop(&mut self, rep: Resource<FutureTrailers>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::future_trailers", "drop");

        let handle = rep.rep();
        // The durable boundary of an HTTP request is closed by `end_http_request`, called from the
        // normal `get()` completion paths and from the *async* drops of the response, body and
        // future-incoming-response resources. This `drop` is synchronous (trailers `drop` is not an
        // async host call), so it cannot await that oplog write — it can only release the in-memory
        // request tracking. If the trailers future is the request's close-owner and is dropped
        // without `get()`, the request's durable scope is therefore left open; the warning makes
        // that observable.
        if let Some(state) = self.state.open_http_requests.remove(&handle)
            && state.close_owner == HttpRequestCloseOwner::FutureTrailersDrop
        {
            warn!(
                "FutureTrailers dropped without get() — HTTP request tracking for handle {} \
                     removed but durable function boundary not closed",
                handle
            );
        }

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

/// Aborts an outgoing HTTP request body that is being finished while a
/// status-code retry policy has already matched against an early response.
///
/// `HostOutgoingBody::drop` in wasmtime calls `HostOutgoingBody::abort`
/// internally, which sends `FinishMessage::Abort` over the body's finish
/// channel. Hyper observes the resulting error frame and tears down the
/// underlying connection without writing the chunked-transfer terminator
/// (`0\r\n\r\n`).
///
/// This matters when the peer returned a final response (e.g. HTTP 500)
/// without consuming the request body. If we let hyper finish the body
/// normally, the trailing terminator would be parsed by the receiving
/// HTTP/1.1 server as the start of a new (malformed) request and prompt
/// it to reply HTTP 400 — bypassing the user's retry policy entirely.
///
/// The retry resend itself is independent of this connection: it
/// reconstructs the request from the oplog and dispatches it on a fresh
/// transport via `try_status_code_retry` (which forces
/// `connection_pool = None`).
fn abort_outgoing_body_for_pending_status_retry(
    view: &mut wasmtime_wasi_http::p2::WasiHttpCtxView<'_>,
    body: Resource<OutgoingBody>,
) -> HttpResult<()> {
    HostOutgoingBody::drop(view, body).map_err(|e| {
        HttpError::trap(wasmtime::Error::msg(format!(
            "failed to abort outgoing body for pending status retry: {e}"
        )))
    })
}

impl<Ctx: WorkerCtx> HostOutgoingBody for DurableWorkerCtx<Ctx> {
    fn write(
        &mut self,
        self_: Resource<OutgoingBody>,
    ) -> wasmtime::Result<Result<Resource<OutputStream>, ()>> {
        self.observe_function_call("http::types::outgoing_body", "write");
        let body_rep = self_.rep();
        let result = HostOutgoingBody::write(&mut self.as_wasi_http_view(), self_);
        if let Ok(Ok(ref stream)) = result {
            let stream_rep = stream.rep();
            // Associate the output stream with the HttpRequestState that owns this body
            if let Some(request_handle) = self.state.find_request_handle_by_outgoing_body(body_rep)
            {
                if let Some(state) = self.state.open_http_requests.get_mut(&request_handle) {
                    state.output_stream_rep = Some(stream_rep);
                }
            } else {
                // handle() hasn't been called yet — store the pending mapping so
                // handle() can populate output_stream_rep when it creates the state.
                self.state
                    .pending_http_outgoing_body_stream
                    .insert(body_rep, stream_rep);
            }
        }
        result
    }

    fn finish(
        &mut self,
        this: Resource<OutgoingBody>,
        trailers: Option<Resource<Trailers>>,
    ) -> HttpResult<()> {
        self.observe_function_call("http::types::outgoing_body", "finish");
        let body_rep = this.rep();
        let has_trailers = trailers.is_some();
        let request_handle = self.state.find_request_handle_by_outgoing_body(body_rep);
        let pending_status_retry_matched = request_handle
            .and_then(|handle| self.state.open_http_requests.get(&handle))
            .and_then(|state| state.pending_status_retry_decision.as_ref())
            .is_some_and(|rx| {
                matches!(
                    *rx.borrow(),
                    crate::durable_host::PendingStatusRetryDecision::Matched
                )
            });
        // When a status-code retry policy has already matched against the
        // early response, abort the body instead of finishing it. See
        // [`abort_outgoing_body_for_pending_status_retry`] for the full
        // rationale. Trailer-bearing finishes fall through to the normal
        // path because `is_http_inline_retry_eligible` already rejects
        // retries when trailers are present.
        //
        // NOTE: `pending_status_retry_matched` is sampled synchronously here.
        // If the wrapper task that resolves the policy hasn't published the
        // `Matched` decision yet, the body will be finished normally and the
        // doomed keep-alive socket may still be poisoned by the chunked
        // terminator. The downstream retry resend always uses a fresh
        // connection (`connection_pool = None`), so this only manifests as
        // an extra leftover request observed by some servers, not as a guest
        // failure. The write paths handle this race more robustly via
        // `tokio::sync::watch::Receiver::wait_for`; finish() cannot do the
        // same because we have no signal that an early response is even
        // expected.
        let result = if pending_status_retry_matched && !has_trailers {
            debug_assert!(trailers.is_none(), "checked via has_trailers guard above");
            abort_outgoing_body_for_pending_status_retry(&mut self.as_wasi_http_view(), this)
        } else {
            HostOutgoingBody::finish(&mut self.as_wasi_http_view(), this, trailers)
        };
        // The body channel was already closed by hyper when the early
        // response arrived, so a normal `finish` may now fail with
        // `HttpProtocolError`. Treat that error as benign when a status
        // retry is pending — the body bytes are in the oplog and the
        // retry path will resend on a fresh connection.
        let suppress_finish_error_for_pending_retry =
            result.is_err() && !has_trailers && pending_status_retry_matched;
        if result.is_ok() || suppress_finish_error_for_pending_retry {
            if let Some(handle) = request_handle
                && let Some(state) = self.state.open_http_requests.get_mut(&handle)
            {
                state.retry.body_finished = true;
                state.retry.has_outgoing_trailers = has_trailers;
                state.outgoing_body_rep = None;
                if let Some(body_state) = &state.outgoing_body_state {
                    let _ = body_state.send(HttpOutgoingBodyState::Finished);
                }
                state.outgoing_body_state = None;
                state.pending_status_retry_decision = None;
            } else if let Some(request_rep) = self
                .state
                .find_pending_request_rep_by_outgoing_body(body_rep)
            {
                let retry = self
                    .state
                    .pending_http_retry_eligibility
                    .entry(request_rep)
                    .or_default();
                retry.body_finished = true;
                retry.has_outgoing_trailers = has_trailers;
            }
        }
        if suppress_finish_error_for_pending_retry {
            Ok(())
        } else {
            result
        }
    }

    fn drop(&mut self, rep: Resource<OutgoingBody>) -> wasmtime::Result<()> {
        self.observe_function_call("http::types::outgoing_body", "drop");
        let body_rep = rep.rep();
        let result = HostOutgoingBody::drop(&mut self.as_wasi_http_view(), rep);
        if result.is_ok() {
            if let Some(handle) = self.state.find_request_handle_by_outgoing_body(body_rep) {
                if let Some(state) = self.state.open_http_requests.get_mut(&handle) {
                    if !state.retry.body_finished {
                        state.retry.body_closed_without_finish = true;
                    }
                    if !state.retry.body_finished
                        && let Some(body_state) = &state.outgoing_body_state
                    {
                        let _ = body_state.send(HttpOutgoingBodyState::Closed);
                    }
                    state.outgoing_body_state = None;
                    state.pending_status_retry_decision = None;
                    state.outgoing_body_rep = None;
                }
            } else if let Some(request_rep) = self
                .state
                .find_pending_request_rep_by_outgoing_body(body_rep)
            {
                self.state
                    .pending_http_retry_eligibility
                    .remove(&request_rep);
            }
            // Clean up pending mapping if handle() was never called
            self.state
                .pending_http_outgoing_body_stream
                .remove(&body_rep);
        }
        result
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
            let request_state = self
                .state
                .open_http_requests
                .get(&handle)
                .cloned()
                .ok_or_else(|| {
                    wasmtime::Error::msg(
                        "No matching HTTP request is associated with resource handle",
                    )
                })?;

            let request = request_state.request.clone();
            let begin_index = request_state.begin_index;

            let future_is_deferred = {
                let future = self
                    .table()
                    .get(&Resource::<FutureIncomingResponse>::new_borrow(handle))?;
                matches!(
                    future,
                    wasmtime_wasi_http::p2::types::HostFutureIncomingResponse::Deferred { .. }
                )
            };

            // When body writes were replayed from oplog (not written to the live
            // body pipe), the request that handle() sent has an empty body. We must
            // reconstruct the full request from oplog and swap the future before
            // trying to get the response.
            if request_state.retry.body_finished
                && (request_state.retry.replayed_body_writes || future_is_deferred)
            {
                self.rebuild_request_after_replay(handle, &request_state)
                    .await?;
            }

            let mut response =
                HostFutureIncomingResponse::get(&mut self.as_wasi_http_view(), self_).await;

            // Outer per-attempt loop. Each iteration:
            //   1. Converts background-retry "trap+replay" markers into the standard
            //      transient host-failure retry path.
            //   2. Classifies the response.
            //   3. Runs the existing transient transport-error inline retry.
            //   4. Triggers the transient host-failure retry if still erroring.
            //   5. Runs status-code retry against user-defined policies. On
            //      `Retried(Ok)` it swaps the future entry and continues the loop;
            //      on other outcomes it falls through to persist + expose.
            // The loop bound is the user policy's own attempt budget (encoded in the
            // retry state machinery), not a hard-coded cap.
            let (serializable_response, _for_retry) = loop {
                // Background retry may decide that the next delay must escape to the
                // outer retry/replay machinery. Convert that marker trap back into the
                // same transient host failure path used by non-background HTTP calls.
                if let Err(err) = &response
                    && let Some(error_code) = take_http_background_retry_fallback(err)
                {
                    self.state.set_ambient_retry_point(begin_index);
                    let failure = anyhow::Error::new(ClassifiedHostError {
                        kind: HostFailureKind::Transient,
                        message: error_code.to_string(),
                    });
                    let mut properties = golem_common::model::RetryContext::http(
                        &request_state.request.method.to_string(),
                        &request_state.request.uri,
                    );
                    self.state.enrich_retry_properties(&mut properties);
                    properties.set(
                        "error-type",
                        golem_common::model::PredicateValue::Text("transient".to_string()),
                    );
                    self.try_trigger_retry(failure, properties)
                        .await
                        .map_err(wasmtime::Error::from_anyhow)?;
                    let future_res = self
                        .table()
                        .get_mut(&Resource::<FutureIncomingResponse>::new_borrow(handle))?;
                    *future_res = wasmtime_wasi_http::p2::types::HostFutureIncomingResponse::ready(
                        Ok(Err(error_code.clone())),
                    );
                    response = Ok(Some(Ok(Err(error_code))));
                }

                let mut classified = classify_http_response(self.table(), &response)?;

                if let Err(err) = &classified.1 {
                    let kind = match err {
                        HttpFailure::ErrorCode(code) => classify_http_error_code(code),
                        HttpFailure::Other(_) => HostFailureKind::Transient,
                    };
                    // Only try an extra awaiting-response inline retry when background retry is not
                    // already managing this request. Background retry either succeeded, exhausted,
                    // or already requested trap+replay in the block above.
                    let has_background_retry = self
                        .state
                        .open_http_requests
                        .get(&handle)
                        .is_some_and(|s| s.retry.has_background_retry);

                    if kind == HostFailureKind::Transient
                        && !has_background_retry
                        && let Some(request_state) =
                            self.state.open_http_requests.get(&handle).cloned()
                        && let Some(retried_response) =
                            crate::durable_host::http::inline_retry::try_awaiting_response_inline_retry(
                                self,
                                &request_state,
                            )
                            .await
                            .map_err(wasmtime::Error::from_anyhow)?
                    {
                        let future_res = self
                            .table()
                            .get_mut(&Resource::<FutureIncomingResponse>::new_borrow(handle))?;
                        *future_res =
                            wasmtime_wasi_http::p2::types::HostFutureIncomingResponse::ready(Ok(
                                Ok(retried_response),
                            ));

                        let self2 = Resource::<FutureIncomingResponse>::new_borrow(handle);
                        response = HostFutureIncomingResponse::get(
                            &mut self.as_wasi_http_view(),
                            self2,
                        )
                        .await;
                        classified = classify_http_response(self.table(), &response)?;
                    }
                }

                let (serializable_response, for_retry) = classified;

                if let Err(err) = &for_retry {
                    let kind = match err {
                        HttpFailure::ErrorCode(code) => classify_http_error_code(code),
                        HttpFailure::Other(_) => HostFailureKind::Transient,
                    };
                    let has_background_retry = self
                        .state
                        .open_http_requests
                        .get(&handle)
                        .is_some_and(|s| s.retry.has_background_retry);
                    if kind == HostFailureKind::Transient && !has_background_retry {
                        self.state.set_ambient_retry_point(begin_index);
                        let failure = anyhow::Error::new(ClassifiedHostError {
                            kind,
                            message: err.to_string(),
                        });
                        let mut properties = golem_common::model::RetryProperties::new();
                        properties.set(
                            "error-type",
                            golem_common::model::PredicateValue::Text("transient".to_string()),
                        );
                        self.try_trigger_retry(failure, properties)
                            .await
                            .map_err(wasmtime::Error::from_anyhow)?;
                    }
                }

                // status-code retry against user-defined policies. Only when:
                //   - the response is a real headers-received result (not pending)
                //   - we have a Resource<IncomingResponse> handle in `response`
                //   - the request is still tracked in `open_http_requests`
                // Note: we deliberately do NOT gate on `has_background_retry` here.
                // In v1 background retry handles transport-error retry only, so
                // status retry must remain available even when the request was
                // wrapped with background retry.
                let status_retry_outcome = if let SerializableHttpResponse::HeadersReceived(headers) =
                    &serializable_response
                    && let Ok(Some(Ok(Ok(rejected)))) = &response
                    && let Some(request_state) = self.state.open_http_requests.get(&handle).cloned()
                {
                    let status = headers.status;
                    // Resource rep of the rejected response. Used by
                    // `try_status_code_retry` to poison the underlying pooled
                    // connection when a status-code retry policy matches.
                    let rejected_response_rep = rejected.rep();
                    let outcome = try_status_code_retry(
                        self,
                        &request_state,
                        status,
                        Some(rejected_response_rep),
                    )
                    .await
                    .map_err(wasmtime::Error::from_anyhow)?;
                    Some((status, request_state, outcome))
                } else {
                    None
                };

                match status_retry_outcome {
                    Some((_status, _request_state, StatusRetryOutcome::NoRetry)) => {
                        break (serializable_response, for_retry);
                    }
                    Some((_status, request_state, StatusRetryOutcome::Retried(retried))) => {
                        match *retried {
                            Ok(new_resp) => {
                                // Drop the rejected IncomingResponse resource so it does not
                                // leak. Use wasi-http's resource drop path (rather than raw
                                // table().delete) to ensure any host-side teardown runs.
                                if let Ok(Some(Ok(Ok(rejected)))) = &response {
                                    let rejected_rep = rejected.rep();
                                    let _ = HostIncomingResponse::drop(
                                        &mut self.as_wasi_http_view(),
                                        Resource::<IncomingResponse>::new_own(rejected_rep),
                                    )
                                    .await;
                                }
                                let future_res =
                                    self.table().get_mut(
                                        &Resource::<FutureIncomingResponse>::new_borrow(handle),
                                    )?;
                                *future_res =
                                    wasmtime_wasi_http::p2::types::HostFutureIncomingResponse::ready(
                                        Ok(Ok(new_resp)),
                                    );
                                let self2 = Resource::<FutureIncomingResponse>::new_borrow(handle);
                                response = HostFutureIncomingResponse::get(
                                    &mut self.as_wasi_http_view(),
                                    self2,
                                )
                                .await;
                                // Fall through to top of loop to re-classify and possibly retry again.
                                continue;
                            }
                            Err(error_code) => {
                                // The status-retry resend itself produced a transport error.
                                // This error did NOT come through the background-retry task,
                                // so it must be routed through the standard transient-failure
                                // retry/trap path regardless of whether the original request
                                // was background-managed (otherwise a real transport error
                                // could leak straight to the guest).
                                if let Ok(Some(Ok(Ok(rejected)))) = &response {
                                    let rejected_rep = rejected.rep();
                                    let _ = HostIncomingResponse::drop(
                                        &mut self.as_wasi_http_view(),
                                        Resource::<IncomingResponse>::new_own(rejected_rep),
                                    )
                                    .await;
                                }
                                if classify_http_error_code(&error_code)
                                    == HostFailureKind::Transient
                                {
                                    escalate_http_to_outer_retry(
                                        self,
                                        &request_state,
                                        None,
                                        "transient",
                                        error_code.to_string(),
                                    )
                                    .await?;
                                }
                                let future_res =
                                    self.table().get_mut(
                                        &Resource::<FutureIncomingResponse>::new_borrow(handle),
                                    )?;
                                *future_res =
                                    wasmtime_wasi_http::p2::types::HostFutureIncomingResponse::ready(
                                        Ok(Err(error_code.clone())),
                                    );
                                response = Ok(Some(Ok(Err(error_code))));
                                // If outer retry did not trap, its budget is exhausted. Expose
                                // the transport error produced by the status-retry resend, but
                                // do not continue into the generic transient branch and charge
                                // the same failure a second time.
                                break classify_http_response(self.table(), &response)?;
                            }
                        }
                    }
                    Some((_status, _request_state, StatusRetryOutcome::Exhausted)) => {
                        // Expose the most-recent rejected response as-is.
                        break (serializable_response, for_retry);
                    }
                    Some((status, request_state, StatusRetryOutcome::FallBackToTrap)) => {
                        // Escalate to the existing transient-host-failure trap path.
                        escalate_http_to_outer_retry(
                            self,
                            &request_state,
                            Some(status),
                            "http-status",
                            format!(
                                "HTTP response status {status} matched user-defined retry policy"
                            ),
                        )
                        .await?;
                        // If `try_trigger_retry` did not trap, the outer retry budget
                        // is also exhausted — expose the response.
                        break (serializable_response, for_retry);
                    }
                    None => {
                        break (serializable_response, for_retry);
                    }
                }
            };

            let is_pending = matches!(serializable_response, SerializableHttpResponse::Pending);
            if let Some(state) = self.state.open_http_requests.get_mut(&handle) {
                state.response_status = match &serializable_response {
                    SerializableHttpResponse::HeadersReceived(headers) => Some(headers.status),
                    _ => None,
                };
            }
            persist_http_response(self, request, &serializable_response, begin_index).await;

            if !is_pending && let Ok(Some(Ok(Ok(resource)))) = &response {
                let incoming_response_handle = resource.rep();
                continue_http_request(
                    self,
                    handle,
                    incoming_response_handle,
                    HttpRequestCloseOwner::IncomingResponseDrop,
                );
            }

            response
        } else if durable_execution_state.persistence_level == PersistenceLevel::PersistNothing {
            Err(WorkerExecutorError::runtime(
                "Trying to replay an http request in a PersistNothing block",
            )
            .into())
        } else {
            // Propagate WorkerExecutorError via `?` (From) so the downcast
            // survives the wasmtime::Error chain — TrapType::from_error
            // classifies UnexpectedOplogEntry as non-retriable.
            //
            // Each poll persists a completed HTTP durable call as a `Start` + `End` pair (see
            // `persist_http_response`). Replay it through the concurrent resolver: claim the call's
            // `Start` — validating the function identity the `End` does not carry — and await the
            // matching `End` instead of reading the pair positionally.
            let begin_index = self
                .state
                .open_http_requests
                .get(&handle)
                .map(|state| state.begin_index)
                .ok_or_else(|| {
                    wasmtime::Error::from(WorkerExecutorError::runtime(format!(
                        "no open HTTP request for handle {handle} while replaying future_incoming_response::get"
                    )))
                })?;
            let claim = self
                .state
                .replay_state
                .claim_concurrent_start(
                    &HttpTypesFutureIncomingResponseGet::HOST_FUNCTION_NAME,
                    &DurableFunctionType::WriteRemoteBatched(Some(begin_index)),
                )
                .await
                .map_err(wasmtime::Error::from)?;
            let resolution = self
                .state
                .replay_state
                .await_resolution(claim)
                .await
                .map_err(wasmtime::Error::from)?;

            let serialized_response = match resolution {
                Resolution::Completed { response, .. } => {
                    let response_payload = response.ok_or_else(|| {
                        wasmtime::Error::from(WorkerExecutorError::unexpected_oplog_entry(
                            "End { response: Some(..) }",
                            "End { response: None }".to_string(),
                        ))
                    })?;
                    let response = self
                        .state
                        .oplog
                        .download_payload(response_payload)
                        .await
                        .map_err(|err| {
                            WorkerExecutorError::runtime(format!(
                                "failed to download http::types::future_incoming_response::get oplog payload: {err}"
                            ))
                        })?;
                    match response {
                        HostResponse::HttpResponse(response) => response.response,
                        other => {
                            return Err(wasmtime::Error::from(
                                WorkerExecutorError::unexpected_oplog_entry(
                                    "HostResponse::HttpResponse",
                                    format!("{other:?}"),
                                ),
                            ));
                        }
                    }
                }
                Resolution::Cancelled { cancelled_idx, .. } => {
                    return Err(wasmtime::Error::from(
                        WorkerExecutorError::unexpected_oplog_entry(
                            "End",
                            format!("Cancelled at {cancelled_idx}"),
                        ),
                    ));
                }
            };

            match serialized_response {
                SerializableHttpResponse::Pending => Ok(None),
                SerializableHttpResponse::HeadersReceived(serializable_response_headers) => {
                    if let Some(state) = self.state.open_http_requests.get_mut(&handle) {
                        state.response_status = Some(serializable_response_headers.status);
                    }

                    let incoming_response: wasmtime_wasi_http::p2::types::HostIncomingResponse =
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
        if let Some(state) = self.state.open_http_requests.get(&handle)
            && state.close_owner == HttpRequestCloseOwner::FutureIncomingResponseDrop
        {
            end_http_request(self, handle).await?;
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

    fn convert_header_error(&mut self, err: HeaderError) -> wasmtime::Result<BindingsHeaderError> {
        self.observe_function_call("http::types", "convert_header_error");
        Host::convert_header_error(&mut self.as_wasi_http_view(), err)
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Rebuilds an HTTP request from oplog when body writes were replayed
    /// (not written to the live body pipe). This happens after a worker restart
    /// when the oplog contains successful body write entries that were replayed
    /// but not actually sent to the server.
    async fn rebuild_request_after_replay(
        &mut self,
        handle: u32,
        request_state: &crate::durable_host::HttpRequestState,
    ) -> wasmtime::Result<()> {
        use crate::durable_host::http::inline_retry::{
            body_chunks_to_hyper_body, reconstruct_http_request, reconstruct_outgoing_body_chunks,
            send_reconstructed_request, spawn_http_request_with_retry,
        };
        use crate::services::HasOplog;

        tracing::debug!(
            handle = handle,
            "Rebuilding HTTP request from oplog after replayed body writes"
        );

        let oplog = self.public_state.oplog();
        let body_chunks = reconstruct_outgoing_body_chunks(&oplog, request_state.begin_index)
            .await
            .map_err(|e| {
                wasmtime::Error::msg(format!("Failed to reconstruct body from oplog: {e}"))
            })?;

        let hyper_body = body_chunks_to_hyper_body(body_chunks);
        let http_request = reconstruct_http_request(&request_state.request, hyper_body, &[])
            .map_err(|e| {
                wasmtime::Error::msg(format!("Failed to reconstruct HTTP request: {e}"))
            })?;

        let config = request_state.outgoing_request_config();
        let new_future = send_reconstructed_request(http_request, config, None);

        // Wrap with background retry if the original had it
        let exec_state = self.durable_execution_state();
        let final_future = if request_state.retry.has_background_retry {
            if let wasmtime_wasi_http::p2::types::HostFutureIncomingResponse::Pending(
                pending_handle,
            ) = new_future
            {
                let environment_state_service = self.state.environment_state_service.clone();
                let environment_id = self.state.owned_agent_id.environment_id;
                let default_retry_policy =
                    NamedRetryPolicy::default_from_config(&self.state.config.retry);
                let agent_config_retry_policies = self.state.agent_config_retry_policies();
                let runtime_retry_policy_mutations =
                    self.state.runtime_retry_policy_mutations.clone();
                let mut retry_properties = golem_common::model::RetryContext::http(
                    &request_state.request.method.to_string(),
                    &request_state.request.uri,
                );
                self.state.enrich_retry_properties(&mut retry_properties);
                let retry_handle = spawn_http_request_with_retry(
                    pending_handle,
                    request_state.request.clone(),
                    request_state.outgoing_request_config(),
                    None,
                    self.public_state.worker(),
                    environment_state_service,
                    environment_id,
                    default_retry_policy,
                    agent_config_retry_policies,
                    runtime_retry_policy_mutations,
                    retry_properties,
                    exec_state.max_in_function_retry_delay,
                    request_state.begin_index,
                    self.execution_status.clone(),
                );
                wasmtime_wasi_http::p2::types::HostFutureIncomingResponse::pending(retry_handle)
            } else {
                new_future
            }
        } else {
            new_future
        };

        // Swap the FutureIncomingResponse in the resource table
        let future_res: &mut wasmtime_wasi_http::p2::types::HostFutureIncomingResponse = self
            .table()
            .get_mut(&Resource::<FutureIncomingResponse>::new_borrow(handle))?;
        *future_res = final_future;

        // Clear the replayed flag so we don't rebuild again
        if let Some(state) = self.state.open_http_requests.get_mut(&handle) {
            state.retry.replayed_body_writes = false;
        }

        Ok(())
    }
}

/// Escalates an HTTP failure that occurred during status-retry handling to the outer
/// transient host-failure retry/trap path. Used by both:
/// - `Retried(Err(error_code))`: a status-retry resend itself failed at transport level
///   (`status = None`, `error_type = "transient"`).
/// - `FallBackToTrap`: the user-defined status-code policy decided to escalate
///   (`status = Some(...)`, `error_type = "http-status"`).
async fn escalate_http_to_outer_retry<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    request_state: &crate::durable_host::HttpRequestState,
    status: Option<u16>,
    error_type: &'static str,
    message: String,
) -> wasmtime::Result<()> {
    ctx.state.set_ambient_retry_point(request_state.begin_index);
    let mut properties = golem_common::model::RetryContext::http_with_response(
        &request_state.request.method.to_string(),
        &request_state.request.uri,
        status,
        error_type,
    );
    ctx.state.enrich_retry_properties(&mut properties);
    let failure = anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Transient,
        message,
    });
    ctx.try_trigger_retry(failure, properties)
        .await
        .map_err(wasmtime::Error::from_anyhow)
}

#[allow(clippy::type_complexity)]
fn classify_http_response(
    table: &mut wasmtime::component::ResourceTable,
    response: &Result<
        Option<Result<Result<Resource<IncomingResponse>, ErrorCode>, ()>>,
        wasmtime::Error,
    >,
) -> Result<(SerializableHttpResponse, Result<(), HttpFailure>), wasmtime::Error> {
    match response {
        Ok(None) => Ok((SerializableHttpResponse::Pending, Ok(()))),
        Ok(Some(Ok(Ok(resource)))) => {
            let incoming_response = table.get(resource)?;
            Ok((
                SerializableHttpResponse::HeadersReceived(
                    SerializableResponseHeaders::try_from(incoming_response)
                        .map_err(wasmtime::Error::from_anyhow)?,
                ),
                Ok(()),
            ))
        }
        Ok(Some(Err(_))) => Ok((
            SerializableHttpResponse::InternalError(None),
            Err(HttpFailure::Other("Unknown error".to_string())),
        )),
        Ok(Some(Ok(Err(error_code)))) => Ok((
            SerializableHttpResponse::HttpError(error_code.clone().into()),
            Err(HttpFailure::ErrorCode(error_code.clone())),
        )),
        Err(err) => Ok((
            SerializableHttpResponse::InternalError(Some(err.to_string())),
            Err(HttpFailure::Other(err.to_string())),
        )),
    }
}

async fn persist_http_response<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    request: golem_common::model::oplog::HostRequestHttpRequest,
    serializable_response: &SerializableHttpResponse,
    begin_index: golem_common::model::oplog::OplogIndex,
) {
    if ctx.state.snapshotting_mode.is_none() {
        ctx.append_completed_child_call(
            HttpTypesFutureIncomingResponseGet::HOST_FUNCTION_NAME,
            &HostRequest::HttpRequest(request),
            &HostResponse::HttpResponse(HostResponseHttpResponse {
                response: serializable_response.clone(),
            }),
            DurableFunctionType::WriteRemoteBatched(Some(begin_index)),
            // The HTTP request always opens a `WriteRemoteBatched(None)` scope at `begin_index`
            // (see `outgoing_handler::handle`), so this poll nests directly inside it.
            Some(begin_index),
        )
        .await
        .unwrap_or_else(|err| panic!("failed to serialize http response: {err}"));
        ctx.public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::DurableOnly)
            .await;
    }
}

/// Typed HTTP failure for retry classification, preserving the original `ErrorCode`
/// so the classifier can distinguish transient from permanent errors.
enum HttpFailure {
    ErrorCode(ErrorCode),
    Other(String),
}

impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpFailure::ErrorCode(code) => write!(f, "{code}"),
            HttpFailure::Other(msg) => write!(f, "{msg}"),
        }
    }
}

/// Classifies a WASI HTTP `ErrorCode` as transient or permanent for retry purposes.
pub fn classify_http_error_code(code: &ErrorCode) -> HostFailureKind {
    match code {
        // DNS errors — transient (may resolve on retry)
        ErrorCode::DnsTimeout | ErrorCode::DnsError(_) => HostFailureKind::Transient,

        // Destination errors — transient (network routing may change)
        ErrorCode::DestinationNotFound
        | ErrorCode::DestinationUnavailable
        | ErrorCode::DestinationIpProhibited
        | ErrorCode::DestinationIpUnroutable => HostFailureKind::Transient,

        // TLS errors — permanent (certificate/protocol issues won't change on retry)
        ErrorCode::TlsProtocolError
        | ErrorCode::TlsAlertReceived(_)
        | ErrorCode::TlsCertificateError => HostFailureKind::Permanent,

        // Connection errors — transient (network issues are typically transient)
        ErrorCode::ConnectionRefused
        | ErrorCode::ConnectionTerminated
        | ErrorCode::ConnectionTimeout
        | ErrorCode::ConnectionReadTimeout
        | ErrorCode::ConnectionWriteTimeout
        | ErrorCode::ConnectionLimitReached => HostFailureKind::Transient,

        // HTTP protocol errors — permanent (deterministic for the same request)
        ErrorCode::HttpRequestDenied
        | ErrorCode::HttpRequestLengthRequired
        | ErrorCode::HttpRequestBodySize(_)
        | ErrorCode::HttpRequestMethodInvalid
        | ErrorCode::HttpRequestUriInvalid
        | ErrorCode::HttpRequestUriTooLong
        | ErrorCode::HttpRequestHeaderSectionSize(_)
        | ErrorCode::HttpRequestHeaderSize(_)
        | ErrorCode::HttpRequestTrailerSectionSize(_)
        | ErrorCode::HttpRequestTrailerSize(_)
        | ErrorCode::HttpResponseHeaderSectionSize(_)
        | ErrorCode::HttpResponseHeaderSize(_)
        | ErrorCode::HttpResponseBodySize(_)
        | ErrorCode::HttpResponseTrailerSectionSize(_)
        | ErrorCode::HttpResponseTrailerSize(_)
        | ErrorCode::HttpResponseTransferCoding(_)
        | ErrorCode::HttpResponseContentCoding(_)
        | ErrorCode::HttpUpgradeFailed => HostFailureKind::Permanent,

        // HttpProtocolError is used by hyper as a catch-all for connection-level
        // failures (e.g. connection reset mid-request). Treat as transient because
        // the same request may succeed on retry to the same server.
        ErrorCode::HttpProtocolError => HostFailureKind::Transient,

        // Timeout errors — transient (may succeed with more time)
        ErrorCode::LoopDetected
        | ErrorCode::ConfigurationError
        | ErrorCode::HttpResponseTimeout => HostFailureKind::Transient,

        // Incomplete/internal — transient (default)
        ErrorCode::HttpResponseIncomplete | ErrorCode::InternalError(_) => {
            HostFailureKind::Transient
        }
    }
}
