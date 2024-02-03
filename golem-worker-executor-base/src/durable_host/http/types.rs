use anyhow::anyhow;
use async_trait::async_trait;

use golem_common::model::Timestamp;
use http::{HeaderName, HeaderValue};

use std::collections::HashMap;
use std::str::FromStr;
use tracing::info;

use wasmtime::component::Resource;
use wasmtime_wasi::preview2::subscribe;

use crate::durable_host::{Durability, DurableWorkerCtx, Ready};
use crate::metrics::wasm::record_host_function_call;

use crate::durable_host::http::serialized::{
    SerializableErrorCode, SerializableResponse, SerializableResponseHeaders,
};
use crate::durable_host::serialized::SerializableError;
use crate::workerctx::WorkerCtx;
use golem_common::model::{OplogEntry, WrappedFunctionType};
use wasmtime_wasi_http::bindings::wasi::http::types::{
    Duration, ErrorCode, FieldKey, FieldValue, Fields, FutureIncomingResponse, FutureTrailers,
    HeaderError, Headers, Host, HostFields, HostFutureIncomingResponse, HostFutureTrailers,
    HostIncomingBody, HostIncomingRequest, HostIncomingResponse, HostOutgoingBody,
    HostOutgoingRequest, HostOutgoingResponse, HostRequestOptions, HostResponseOutparam,
    IncomingBody, IncomingRequest, IncomingResponse, InputStream, IoError, Method, OutgoingBody,
    OutgoingRequest, OutgoingResponse, OutputStream, Pollable, RequestOptions, ResponseOutparam,
    Scheme, StatusCode, Trailers,
};
use wasmtime_wasi_http::types::FieldMap;
use wasmtime_wasi_http::types_impl::get_fields;

impl<Ctx: WorkerCtx> HostFields for DurableWorkerCtx<Ctx> {
    fn new(&mut self) -> anyhow::Result<Resource<Fields>> {
        record_host_function_call("http::types::fields", "new");
        HostFields::new(&mut self.as_wasi_http_view())
    }

    fn from_list(
        &mut self,
        entries: Vec<(FieldKey, FieldValue)>,
    ) -> anyhow::Result<Result<Resource<Fields>, HeaderError>> {
        record_host_function_call("http::types::fields", "from_list");
        HostFields::from_list(&mut self.as_wasi_http_view(), entries)
    }

    fn get(&mut self, self_: Resource<Fields>, name: FieldKey) -> anyhow::Result<Vec<FieldValue>> {
        record_host_function_call("http::types::fields", "get");
        HostFields::get(&mut self.as_wasi_http_view(), self_, name)
    }

    fn has(&mut self, self_: Resource<Fields>, name: FieldKey) -> anyhow::Result<bool> {
        record_host_function_call("http::types::fields", "has");
        HostFields::has(&mut self.as_wasi_http_view(), self_, name)
    }

    fn set(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
        value: Vec<FieldValue>,
    ) -> anyhow::Result<Result<(), HeaderError>> {
        record_host_function_call("http::types::fields", "set");
        HostFields::set(&mut self.as_wasi_http_view(), self_, name, value)
    }

    fn delete(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
    ) -> anyhow::Result<Result<(), HeaderError>> {
        record_host_function_call("http::types::fields", "delete");
        HostFields::delete(&mut self.as_wasi_http_view(), self_, name)
    }

    fn append(
        &mut self,
        self_: Resource<Fields>,
        name: FieldKey,
        value: FieldValue,
    ) -> anyhow::Result<Result<(), HeaderError>> {
        record_host_function_call("http::types::fields", "append");
        HostFields::append(&mut self.as_wasi_http_view(), self_, name, value)
    }

    fn entries(&mut self, self_: Resource<Fields>) -> anyhow::Result<Vec<(FieldKey, FieldValue)>> {
        record_host_function_call("http::types::fields", "entries");
        HostFields::entries(&mut self.as_wasi_http_view(), self_)
    }

    fn clone(&mut self, self_: Resource<Fields>) -> anyhow::Result<Resource<Fields>> {
        record_host_function_call("http::types::fields", "clone");
        HostFields::clone(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<Fields>) -> anyhow::Result<()> {
        record_host_function_call("http::types::fields", "drop");
        HostFields::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostIncomingRequest for DurableWorkerCtx<Ctx> {
    fn method(&mut self, self_: Resource<IncomingRequest>) -> anyhow::Result<Method> {
        record_host_function_call("http::types::incoming_request", "method");
        HostIncomingRequest::method(&mut self.as_wasi_http_view(), self_)
    }

    fn path_with_query(
        &mut self,
        self_: Resource<IncomingRequest>,
    ) -> anyhow::Result<Option<String>> {
        record_host_function_call("http::types::incoming_request", "path_with_query");
        HostIncomingRequest::path_with_query(&mut self.as_wasi_http_view(), self_)
    }

    fn scheme(&mut self, self_: Resource<IncomingRequest>) -> anyhow::Result<Option<Scheme>> {
        record_host_function_call("http::types::incoming_request", "scheme");
        HostIncomingRequest::scheme(&mut self.as_wasi_http_view(), self_)
    }

    fn authority(&mut self, self_: Resource<IncomingRequest>) -> anyhow::Result<Option<String>> {
        record_host_function_call("http::types::incoming_request", "authority");
        HostIncomingRequest::authority(&mut self.as_wasi_http_view(), self_)
    }

    fn headers(&mut self, self_: Resource<IncomingRequest>) -> anyhow::Result<Resource<Headers>> {
        record_host_function_call("http::types::incoming_request", "headers");
        HostIncomingRequest::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn consume(
        &mut self,
        self_: Resource<IncomingRequest>,
    ) -> anyhow::Result<Result<Resource<IncomingBody>, ()>> {
        record_host_function_call("http::types::incoming_request", "consume");
        HostIncomingRequest::consume(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<IncomingRequest>) -> anyhow::Result<()> {
        record_host_function_call("http::types::incoming_request", "drop");
        HostIncomingRequest::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> HostOutgoingRequest for DurableWorkerCtx<Ctx> {
    fn new(&mut self, headers: Resource<Headers>) -> anyhow::Result<Resource<OutgoingRequest>> {
        record_host_function_call("http::types::outgoing_request", "new");
        HostOutgoingRequest::new(&mut self.as_wasi_http_view(), headers)
    }

    fn body(
        &mut self,
        self_: Resource<OutgoingRequest>,
    ) -> anyhow::Result<Result<Resource<OutgoingBody>, ()>> {
        record_host_function_call("http::types::outgoing_request", "body");
        HostOutgoingRequest::body(&mut self.as_wasi_http_view(), self_)
    }

    fn method(&mut self, self_: Resource<OutgoingRequest>) -> anyhow::Result<Method> {
        record_host_function_call("http::types::outgoing_request", "method");
        HostOutgoingRequest::method(&mut self.as_wasi_http_view(), self_)
    }

    fn set_method(
        &mut self,
        self_: Resource<OutgoingRequest>,
        method: Method,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call("http::types::outgoing_request", "set_method");
        HostOutgoingRequest::set_method(&mut self.as_wasi_http_view(), self_, method)
    }

    fn path_with_query(
        &mut self,
        self_: Resource<OutgoingRequest>,
    ) -> anyhow::Result<Option<String>> {
        record_host_function_call("http::types::outgoing_request", "path_with_query");
        HostOutgoingRequest::path_with_query(&mut self.as_wasi_http_view(), self_)
    }

    fn set_path_with_query(
        &mut self,
        self_: Resource<OutgoingRequest>,
        path_with_query: Option<String>,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call("http::types::outgoing_request", "set_path_with_query");
        HostOutgoingRequest::set_path_with_query(
            &mut self.as_wasi_http_view(),
            self_,
            path_with_query,
        )
    }

    fn scheme(&mut self, self_: Resource<OutgoingRequest>) -> anyhow::Result<Option<Scheme>> {
        record_host_function_call("http::types::outgoing_request", "scheme");
        HostOutgoingRequest::scheme(&mut self.as_wasi_http_view(), self_)
    }

    fn set_scheme(
        &mut self,
        self_: Resource<OutgoingRequest>,
        scheme: Option<Scheme>,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call("http::types::outgoing_request", "set_scheme");
        HostOutgoingRequest::set_scheme(&mut self.as_wasi_http_view(), self_, scheme)
    }

    fn authority(&mut self, self_: Resource<OutgoingRequest>) -> anyhow::Result<Option<String>> {
        record_host_function_call("http::types::outgoing_request", "authority");
        HostOutgoingRequest::authority(&mut self.as_wasi_http_view(), self_)
    }

    fn set_authority(
        &mut self,
        self_: Resource<OutgoingRequest>,
        authority: Option<String>,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call("http::types::outgoing_request", "set_authority");
        HostOutgoingRequest::set_authority(&mut self.as_wasi_http_view(), self_, authority)
    }

    fn headers(&mut self, self_: Resource<OutgoingRequest>) -> anyhow::Result<Resource<Headers>> {
        record_host_function_call("http::types::outgoing_request", "headers");
        HostOutgoingRequest::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<OutgoingRequest>) -> anyhow::Result<()> {
        record_host_function_call("http::types::outgoing_request", "drop");
        HostOutgoingRequest::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostRequestOptions for DurableWorkerCtx<Ctx> {
    fn new(&mut self) -> anyhow::Result<Resource<RequestOptions>> {
        record_host_function_call("http::types::request_options", "new");
        HostRequestOptions::new(&mut self.as_wasi_http_view())
    }

    fn connect_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
    ) -> anyhow::Result<Option<Duration>> {
        record_host_function_call("http::types::request_options", "connect_timeout_ms");
        HostRequestOptions::connect_timeout(&mut self.as_wasi_http_view(), self_)
    }

    fn set_connect_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
        ms: Option<Duration>,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call("http::types::request_options", "set_connect_timeout_ms");
        info!("set_connect_timeout {ms:?}");
        HostRequestOptions::set_connect_timeout(&mut self.as_wasi_http_view(), self_, ms)
    }

    fn first_byte_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
    ) -> anyhow::Result<Option<Duration>> {
        record_host_function_call("http::types::request_options", "first_byte_timeout_ms");
        HostRequestOptions::first_byte_timeout(&mut self.as_wasi_http_view(), self_)
    }

    fn set_first_byte_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
        ms: Option<Duration>,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call("http::types::request_options", "set_first_byte_timeout_ms");
        HostRequestOptions::set_first_byte_timeout(&mut self.as_wasi_http_view(), self_, ms)
    }

    fn between_bytes_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
    ) -> anyhow::Result<Option<Duration>> {
        record_host_function_call("http::types::request_options", "between_bytes_timeout_ms");
        HostRequestOptions::between_bytes_timeout(&mut self.as_wasi_http_view(), self_)
    }

    fn set_between_bytes_timeout(
        &mut self,
        self_: Resource<RequestOptions>,
        ms: Option<Duration>,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call(
            "http::types::request_options",
            "set_between_bytes_timeout_ms",
        );
        HostRequestOptions::set_between_bytes_timeout(&mut self.as_wasi_http_view(), self_, ms)
    }

    fn drop(&mut self, rep: Resource<RequestOptions>) -> anyhow::Result<()> {
        record_host_function_call("http::types::request_options", "drop");
        HostRequestOptions::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostResponseOutparam for DurableWorkerCtx<Ctx> {
    fn set(
        &mut self,
        param: Resource<ResponseOutparam>,
        response: Result<Resource<OutgoingResponse>, ErrorCode>,
    ) -> anyhow::Result<()> {
        record_host_function_call("http::types::response_outparam", "set");
        HostResponseOutparam::set(&mut self.as_wasi_http_view(), param, response)
    }

    fn drop(&mut self, rep: Resource<ResponseOutparam>) -> anyhow::Result<()> {
        record_host_function_call("http::types::response_outparam", "drop");
        HostResponseOutparam::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostIncomingResponse for DurableWorkerCtx<Ctx> {
    fn status(&mut self, self_: Resource<IncomingResponse>) -> anyhow::Result<StatusCode> {
        record_host_function_call("http::types::incoming_response", "status");
        HostIncomingResponse::status(&mut self.as_wasi_http_view(), self_)
    }

    fn headers(&mut self, self_: Resource<IncomingResponse>) -> anyhow::Result<Resource<Headers>> {
        record_host_function_call("http::types::incoming_response", "headers");
        HostIncomingResponse::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn consume(
        &mut self,
        self_: Resource<IncomingResponse>,
    ) -> anyhow::Result<Result<Resource<IncomingBody>, ()>> {
        record_host_function_call("http::types::incoming_response", "consume");
        HostIncomingResponse::consume(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<IncomingResponse>) -> anyhow::Result<()> {
        record_host_function_call("http::types::incoming_response", "drop");
        HostIncomingResponse::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostIncomingBody for DurableWorkerCtx<Ctx> {
    fn stream(
        &mut self,
        self_: Resource<IncomingBody>,
    ) -> anyhow::Result<Result<Resource<InputStream>, ()>> {
        record_host_function_call("http::types::incoming_body", "stream");
        HostIncomingBody::stream(&mut self.as_wasi_http_view(), self_)
    }

    fn finish(&mut self, this: Resource<IncomingBody>) -> anyhow::Result<Resource<FutureTrailers>> {
        record_host_function_call("http::types::incoming_body", "finish");
        HostIncomingBody::finish(&mut self.as_wasi_http_view(), this)
    }

    fn drop(&mut self, rep: Resource<IncomingBody>) -> anyhow::Result<()> {
        record_host_function_call("http::types::incoming_body", "drop");
        HostIncomingBody::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureTrailers for DurableWorkerCtx<Ctx> {
    fn subscribe(&mut self, self_: Resource<FutureTrailers>) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("http::types::future_trailers", "subscribe");
        if self.is_replay() {
            let ready = self.table.push(Ready {})?;
            subscribe(&mut self.table, ready, None)
        } else {
            HostFutureTrailers::subscribe(&mut self.as_wasi_http_view(), self_)
        }
    }

    async fn get(
        &mut self,
        self_: Resource<FutureTrailers>,
    ) -> anyhow::Result<Option<Result<Result<Option<Resource<Trailers>>, ErrorCode>, ()>>> {
        record_host_function_call("http::types::future_trailers", "get");
        Durability::<
            Ctx,
            Option<Result<Result<Option<HashMap<String, Vec<u8>>>, SerializableErrorCode>, ()>>,
            SerializableError,
        >::custom_wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem http::types::future_trailers::get",
            |ctx| {
                Box::pin(async move {
                    HostFutureTrailers::get(&mut ctx.as_wasi_http_view(), self_).await
                })
            },
            |ctx, result| match result {
                Some(Ok(Ok(None))) => Ok(Some(Ok(Ok(None)))),
                Some(Ok(Ok(Some(trailers)))) => {
                    let mut serialized_trailers = HashMap::new();
                    let host_fields: &Resource<wasmtime_wasi_http::types::HostFields> =
                        unsafe { std::mem::transmute(trailers) };

                    for (key, value) in get_fields(&mut ctx.table, host_fields)? {
                        serialized_trailers
                            .insert(key.as_str().to_string(), value.as_bytes().to_vec());
                    }
                    Ok(Some(Ok(Ok(Some(serialized_trailers)))))
                }
                Some(Ok(Err(error_code))) => Ok(Some(Ok(Err(error_code.into())))),
                Some(Err(_)) => Ok(Some(Err(()))),
                None => Ok(None),
            },
            |ctx, serialized| {
                Box::pin(async {
                    match serialized {
                        Some(Ok(Ok(None))) => Ok(Some(Ok(Ok(None)))),
                        Some(Ok(Ok(Some(serialized_trailers)))) => {
                            let mut fields = FieldMap::new();
                            for (key, value) in serialized_trailers {
                                fields.insert(
                                    HeaderName::from_str(&key)?,
                                    HeaderValue::try_from(value)?,
                                );
                            }
                            let hdrs = ctx
                                .table
                                .push(wasmtime_wasi_http::types::HostFields::Owned { fields })?;
                            Ok(Some(Ok(Ok(Some(hdrs)))))
                        }
                        Some(Ok(Err(error_code))) => Ok(Some(Ok(Err(error_code.into())))),
                        Some(Err(_)) => Ok(Some(Err(()))),
                        None => Ok(None),
                    }
                })
            },
        )
        .await
    }

    fn drop(&mut self, rep: Resource<FutureTrailers>) -> anyhow::Result<()> {
        record_host_function_call("http::types::future_trailers", "drop");
        HostFutureTrailers::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostOutgoingResponse for DurableWorkerCtx<Ctx> {
    fn new(&mut self, headers: Resource<Headers>) -> anyhow::Result<Resource<OutgoingResponse>> {
        record_host_function_call("http::types::outgoing_response", "new");
        HostOutgoingResponse::new(&mut self.as_wasi_http_view(), headers)
    }

    fn status_code(&mut self, self_: Resource<OutgoingResponse>) -> anyhow::Result<StatusCode> {
        record_host_function_call("http::types::outgoing_response", "status_code");
        HostOutgoingResponse::status_code(&mut self.as_wasi_http_view(), self_)
    }

    fn set_status_code(
        &mut self,
        self_: Resource<OutgoingResponse>,
        status_code: StatusCode,
    ) -> anyhow::Result<Result<(), ()>> {
        record_host_function_call("http::types::outgoing_response", "set_status_code");
        HostOutgoingResponse::set_status_code(&mut self.as_wasi_http_view(), self_, status_code)
    }

    fn headers(&mut self, self_: Resource<OutgoingResponse>) -> anyhow::Result<Resource<Headers>> {
        record_host_function_call("http::types::outgoing_response", "headers");
        HostOutgoingResponse::headers(&mut self.as_wasi_http_view(), self_)
    }

    fn body(
        &mut self,
        self_: Resource<OutgoingResponse>,
    ) -> anyhow::Result<Result<Resource<OutgoingBody>, ()>> {
        record_host_function_call("http::types::outgoing_response", "body");
        HostOutgoingResponse::body(&mut self.as_wasi_http_view(), self_)
    }

    fn drop(&mut self, rep: Resource<OutgoingResponse>) -> anyhow::Result<()> {
        record_host_function_call("http::types::outgoing_response", "drop");
        HostOutgoingResponse::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostOutgoingBody for DurableWorkerCtx<Ctx> {
    fn write(
        &mut self,
        self_: Resource<OutgoingBody>,
    ) -> anyhow::Result<Result<Resource<OutputStream>, ()>> {
        record_host_function_call("http::types::outgoing_body", "write");
        HostOutgoingBody::write(&mut self.as_wasi_http_view(), self_)
    }

    fn finish(
        &mut self,
        this: Resource<OutgoingBody>,
        trailers: Option<Resource<Trailers>>,
    ) -> anyhow::Result<Result<(), ErrorCode>> {
        record_host_function_call("http::types::outgoing_body", "finish");
        HostOutgoingBody::finish(&mut self.as_wasi_http_view(), this, trailers)
    }

    fn drop(&mut self, rep: Resource<OutgoingBody>) -> anyhow::Result<()> {
        record_host_function_call("http::types::outgoing_body", "drop");
        HostOutgoingBody::drop(&mut self.as_wasi_http_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostFutureIncomingResponse for DurableWorkerCtx<Ctx> {
    fn subscribe(
        &mut self,
        self_: Resource<FutureIncomingResponse>,
    ) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call("http::types::future_incoming_response", "subscribe");
        // In replay mode the future is in Deferred state for which the built-in Subscribe implementation immediately returns.
        // This is exactly what we want for replay mode. In live mode the future is in Pending state until the response is
        // available, and the returned Pollable will wait for the request task to finish.
        HostFutureIncomingResponse::subscribe(&mut self.as_wasi_http_view(), self_)
    }

    async fn get(
        &mut self,
        self_: Resource<FutureIncomingResponse>,
    ) -> anyhow::Result<Option<Result<Result<Resource<IncomingResponse>, ErrorCode>, ()>>> {
        record_host_function_call("http::types::future_incoming_response", "get");
        // Each get call is stored in the oplog. If the result was Error or None (future is pending), we just
        // continue the replay. If the result was Ok, we return register the stored response to the table as a new
        // HostIncomingResponse and return its reference.
        // In live mode the underlying implementation is either polling the response future, or, if it was Deferred
        // (when the request was initiated in replay mode), it starts executing the deferred request and returns None.
        //
        // Note that the response body is streaming, so at this point we don't have it in memory. Each chunk read from
        // the body is stored in the oplog, so we can replay it later. In replay mode we initialize the body with a
        // fake stream which can only be read in the oplog, and fails if we try to read it in live mode.
        self.consume_hint_entries().await;
        if self.is_live() {
            let response =
                HostFutureIncomingResponse::get(&mut self.as_wasi_http_view(), self_).await;

            let serializable_response = match &response {
                Ok(None) => SerializableResponse::Pending,
                Ok(Some(Ok(Ok(resource)))) => {
                    let incoming_response = self.table.get(resource)?;
                    SerializableResponse::HeadersReceived(SerializableResponseHeaders::try_from(
                        incoming_response,
                    )?)
                }
                Ok(Some(Err(_))) => SerializableResponse::InternalError(None),
                Ok(Some(Ok(Err(error_code)))) => {
                    SerializableResponse::HttpError(error_code.clone().into())
                }
                Err(err) => SerializableResponse::InternalError(Some(err.into())),
            };

            let oplog_entry = OplogEntry::imported_function_invoked(
                Timestamp::now_utc(),
                "http::types::future_incoming_response::get".to_string(),
                &serializable_response,
                WrappedFunctionType::WriteRemote,
            )
            .unwrap_or_else(|err| panic!("failed to serialize http response: {err}"));
            self.set_oplog_entry(oplog_entry).await;
            self.commit_oplog().await;

            response
        } else {
            let serialized_response = self.get_oplog_entry_imported_function_invoked::<SerializableResponse>().await.map_err(|golem_err| anyhow!("failed to get http::types::future_incoming_response::get oplog entry: {golem_err}"))?;

            match serialized_response {
                SerializableResponse::Pending => Ok(None),
                SerializableResponse::HeadersReceived(serializable_response_headers) => {
                    let incoming_response: wasmtime_wasi_http::types::HostIncomingResponse =
                        serializable_response_headers.try_into()?;

                    let rep = self.table.push(incoming_response)?;
                    Ok(Some(Ok(Ok(rep))))
                }
                SerializableResponse::InternalError(None) => Ok(Some(Err(()))),
                SerializableResponse::InternalError(Some(serializable_error)) => {
                    Err(serializable_error.into())
                }
                SerializableResponse::HttpError(error_code) => Ok(Some(Ok(Err(error_code.into())))),
            }
        }
    }

    fn drop(&mut self, rep: Resource<FutureIncomingResponse>) -> anyhow::Result<()> {
        record_host_function_call("http::types::future_incoming_response", "drop");
        HostFutureIncomingResponse::drop(&mut self.as_wasi_http_view(), rep)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn http_error_code(&mut self, err: Resource<IoError>) -> anyhow::Result<Option<ErrorCode>> {
        record_host_function_call("http::types", "http_error_code");
        Host::http_error_code(&mut self.as_wasi_http_view(), err)
    }
}
