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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable, NotCancellable};
use crate::durable_host::durability::{DurableCallTrapContext, mark_durable_call_trap_context};
use crate::durable_host::p3::{DurableP3, DurableP3View, durable_worker_ctx, wasi_http_view};
use crate::workerctx::WorkerCtx;
use anyhow::Context as _;
use bytes::Bytes;
use golem_common::model::oplog::host_functions::{
    P3HttpClientConsumeBody, P3HttpClientConsumeBodyChunk, P3HttpClientSend,
};
use golem_common::model::oplog::payload::types::{
    SerializableDnsErrorPayload, SerializableFieldSizePayload, SerializableHttpErrorCode,
    SerializableHttpMethod, SerializableP3HttpBodyChunk, SerializableP3HttpClientSend,
    SerializableP3HttpClientSendResult, SerializableP3HttpConsumeBodyResult,
    SerializableP3HttpRequestOptions, SerializableP3HttpScheme, SerializableResponseHeaders,
    SerializableTlsAlertReceivedPayload,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestP3HttpClientSend,
    HostResponseP3HttpClientConsumeBodyChunk, HostResponseP3HttpClientConsumeBodyResult,
    HostResponseP3HttpClientSendResult,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body_util::BodyExt as _;
use http_body_util::Empty;
use http_body_util::combinators::UnsyncBoxBody;
use std::collections::HashMap;
use std::io::Cursor;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureProducer, FutureReader, Resource,
    StreamProducer, StreamReader, StreamResult,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::TrappableError;
use wasmtime_wasi_http::FieldMap;
use wasmtime_wasi_http::p3::bindings::clocks::monotonic_clock::Duration;
use wasmtime_wasi_http::p3::bindings::http::types::{
    ErrorCode, FieldName, FieldValue, Fields, Headers, Method, Request, RequestOptions, Response,
    Scheme, StatusCode, Trailers,
};
use wasmtime_wasi_http::p3::bindings::http::{client, types};
use wasmtime_wasi_http::p3::{HostBodyStreamProducer, WasiHttp, WasiHttpView};

type HttpError = TrappableError<ErrorCode>;
type HeaderError = TrappableError<types::HeaderError>;
type RequestOptionsError = TrappableError<types::RequestOptionsError>;

type HttpResult<T> = Result<T, HttpError>;
type HeaderResult<T> = Result<T, HeaderError>;
type RequestOptionsResult<T> = Result<T, RequestOptionsError>;

impl<Ctx: WorkerCtx> client::Host for DurableP3View<'_, Ctx> {}

impl<Ctx: WorkerCtx> client::HostWithStore for DurableP3<Ctx> {
    async fn send<U: Send>(
        store: &Accessor<U, Self>,
        req: Resource<Request>,
    ) -> HttpResult<Resource<Response>> {
        let request = serialize_request::<Ctx, U>(store, borrow_resource(&req))?;
        let mut handle = CallHandle::<P3HttpClientSend, Cancellable>::start_access(
            store,
            durable_worker_ctx::<Ctx, U>,
            HostRequestP3HttpClientSend { request },
            DurableFunctionType::WriteRemoteBatched(None),
        )
        .await
        .map_err(HttpError::trap)?;

        if !handle.is_live() {
            match handle
                .replay_access(store, durable_worker_ctx::<Ctx, U>)
                .await
                .map_err(HttpError::trap)?
            {
                CallReplayOutcome::Replayed(response) => {
                    return replay_send_response::<Ctx, U>(store, response.result);
                }
                CallReplayOutcome::Incomplete(live_handle) => handle = live_handle,
            }
        }

        let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
        match <WasiHttp as client::HostWithStore>::send(&http_store, req).await {
            Ok(response) => {
                let result =
                    SerializableP3HttpClientSendResult::Success(serialize_response_headers::<
                        Ctx,
                        U,
                    >(
                        store,
                        borrow_resource(&response),
                    )?);
                handle
                    .complete_access(
                        store,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3HttpClientSendResult { result },
                    )
                    .await
                    .map_err(HttpError::trap)?;
                Ok(response)
            }
            Err(error) => {
                if let Some(error_code) = error.downcast_ref() {
                    let result = SerializableP3HttpClientSendResult::HttpError(
                        serialize_error_code(error_code),
                    );
                    handle
                        .complete_access(
                            store,
                            durable_worker_ctx::<Ctx, U>,
                            HostResponseP3HttpClientSendResult { result },
                        )
                        .await
                        .map_err(HttpError::trap)?;
                    Err(error)
                } else {
                    Err(HttpError::trap(wasmtime::Error::from_anyhow(
                        handle.trap(error),
                    )))
                }
            }
        }
    }
}

fn borrow_resource<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}

fn serialize_request<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
) -> HttpResult<SerializableP3HttpClientSend> {
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    Ok(
        http_store.with(|mut access| -> HttpResult<SerializableP3HttpClientSend> {
            let mut view = access.get();
            let method = serialize_method(
                types::HostRequest::get_method(&mut view, borrow_resource(&req))
                    .map_err(HttpError::trap)?,
            );
            let scheme = types::HostRequest::get_scheme(&mut view, borrow_resource(&req))
                .map_err(HttpError::trap)?
                .map(serialize_scheme);
            let authority = types::HostRequest::get_authority(&mut view, borrow_resource(&req))
                .map_err(HttpError::trap)?;
            let path_with_query =
                types::HostRequest::get_path_with_query(&mut view, borrow_resource(&req))
                    .map_err(HttpError::trap)?;
            let headers_resource =
                types::HostRequest::get_headers(&mut view, borrow_resource(&req))
                    .map_err(HttpError::trap)?;
            let headers = copy_fields(&mut view, headers_resource)?;
            let options = match types::HostRequest::get_options(&mut view, borrow_resource(&req))
                .map_err(HttpError::trap)?
            {
                Some(options) => {
                    let serialized = SerializableP3HttpRequestOptions {
                        connect_timeout_nanos: types::HostRequestOptions::get_connect_timeout(
                            &mut view,
                            borrow_resource(&options),
                        )
                        .map_err(HttpError::trap)?,
                        first_byte_timeout_nanos:
                            types::HostRequestOptions::get_first_byte_timeout(
                                &mut view,
                                borrow_resource(&options),
                            )
                            .map_err(HttpError::trap)?,
                        between_bytes_timeout_nanos:
                            types::HostRequestOptions::get_between_bytes_timeout(
                                &mut view,
                                borrow_resource(&options),
                            )
                            .map_err(HttpError::trap)?,
                    };
                    types::HostRequestOptions::drop(&mut view, options).map_err(HttpError::trap)?;
                    Some(serialized)
                }
                None => None,
            };

            Ok(SerializableP3HttpClientSend {
                method,
                scheme,
                authority,
                path_with_query,
                headers,
                options,
            })
        })?,
    )
}

fn serialize_response_headers<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: Resource<Response>,
) -> HttpResult<SerializableResponseHeaders> {
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    Ok(
        http_store.with(|mut access| -> HttpResult<SerializableResponseHeaders> {
            let mut view = access.get();
            let status =
                types::HostResponse::get_status_code(&mut view, borrow_resource(&response))
                    .map_err(HttpError::trap)?;
            let headers_resource =
                types::HostResponse::get_headers(&mut view, response).map_err(HttpError::trap)?;
            let headers = copy_fields(&mut view, headers_resource)?;
            Ok(SerializableResponseHeaders { status, headers })
        })?,
    )
}

fn copy_fields(
    view: &mut wasmtime_wasi_http::p3::WasiHttpCtxView<'_>,
    fields: Resource<Fields>,
) -> HttpResult<HashMap<String, Vec<Vec<u8>>>> {
    let entries =
        types::HostFields::copy_all(view, borrow_resource(&fields)).map_err(HttpError::trap)?;
    types::HostFields::drop(view, fields).map_err(HttpError::trap)?;
    let mut headers = HashMap::new();
    for (name, value) in entries {
        headers.entry(name).or_insert_with(Vec::new).push(value);
    }
    Ok(headers)
}

fn replay_send_response<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    result: SerializableP3HttpClientSendResult,
) -> HttpResult<Resource<Response>> {
    match result {
        SerializableP3HttpClientSendResult::Success(headers) => {
            response_from_recorded_headers::<Ctx, U>(store, headers)
        }
        SerializableP3HttpClientSendResult::HttpError(error) => {
            Err(deserialize_error_code(error).into())
        }
    }
}

fn response_from_recorded_headers<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    recorded: SerializableResponseHeaders,
) -> HttpResult<Resource<Response>> {
    let status = http::StatusCode::from_u16(recorded.status).map_err(HttpError::trap)?;
    let mut headers = HeaderMap::new();
    for (name, values) in recorded.headers {
        let name = HeaderName::try_from(name).map_err(HttpError::trap)?;
        for value in values {
            headers.append(
                name.clone(),
                HeaderValue::try_from(value).map_err(HttpError::trap)?,
            );
        }
    }

    let mut response = http::Response::new(Empty::<Bytes>::new());
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    let (response, _io) = wasmtime_wasi_http::p3::Response::from_http(response);
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    Ok(http_store.with(|mut access| {
        access
            .get()
            .table
            .push(response)
            .context("failed to push replayed p3 HTTP response to table")
            .map_err(wasmtime::Error::from_anyhow)
            .map_err(HttpError::trap)
    })?)
}

fn serialize_method(method: Method) -> SerializableHttpMethod {
    match method {
        Method::Get => SerializableHttpMethod::Get,
        Method::Post => SerializableHttpMethod::Post,
        Method::Put => SerializableHttpMethod::Put,
        Method::Delete => SerializableHttpMethod::Delete,
        Method::Head => SerializableHttpMethod::Head,
        Method::Connect => SerializableHttpMethod::Connect,
        Method::Options => SerializableHttpMethod::Options,
        Method::Trace => SerializableHttpMethod::Trace,
        Method::Patch => SerializableHttpMethod::Patch,
        Method::Other(method) => SerializableHttpMethod::Other(method),
    }
}

fn serialize_scheme(scheme: Scheme) -> SerializableP3HttpScheme {
    match scheme {
        Scheme::Http => SerializableP3HttpScheme::Http,
        Scheme::Https => SerializableP3HttpScheme::Https,
        Scheme::Other(scheme) => SerializableP3HttpScheme::Other(scheme),
    }
}

fn serialize_error_code(error: &ErrorCode) -> SerializableHttpErrorCode {
    match error {
        ErrorCode::DnsTimeout => SerializableHttpErrorCode::DnsTimeout,
        ErrorCode::DnsError(payload) => {
            SerializableHttpErrorCode::DnsError(serialize_dns_error_payload(payload))
        }
        ErrorCode::DestinationNotFound => SerializableHttpErrorCode::DestinationNotFound,
        ErrorCode::DestinationUnavailable => SerializableHttpErrorCode::DestinationUnavailable,
        ErrorCode::DestinationIpProhibited => SerializableHttpErrorCode::DestinationIpProhibited,
        ErrorCode::DestinationIpUnroutable => SerializableHttpErrorCode::DestinationIpUnroutable,
        ErrorCode::ConnectionRefused => SerializableHttpErrorCode::ConnectionRefused,
        ErrorCode::ConnectionTerminated => SerializableHttpErrorCode::ConnectionTerminated,
        ErrorCode::ConnectionTimeout => SerializableHttpErrorCode::ConnectionTimeout,
        ErrorCode::ConnectionReadTimeout => SerializableHttpErrorCode::ConnectionReadTimeout,
        ErrorCode::ConnectionWriteTimeout => SerializableHttpErrorCode::ConnectionWriteTimeout,
        ErrorCode::ConnectionLimitReached => SerializableHttpErrorCode::ConnectionLimitReached,
        ErrorCode::TlsProtocolError => SerializableHttpErrorCode::TlsProtocolError,
        ErrorCode::TlsCertificateError => SerializableHttpErrorCode::TlsCertificateError,
        ErrorCode::TlsAlertReceived(payload) => SerializableHttpErrorCode::TlsAlertReceived(
            serialize_tls_alert_received_payload(payload),
        ),
        ErrorCode::HttpRequestDenied => SerializableHttpErrorCode::HttpRequestDenied,
        ErrorCode::HttpRequestLengthRequired => {
            SerializableHttpErrorCode::HttpRequestLengthRequired
        }
        ErrorCode::HttpRequestBodySize(payload) => {
            SerializableHttpErrorCode::HttpRequestBodySize(*payload)
        }
        ErrorCode::HttpRequestMethodInvalid => SerializableHttpErrorCode::HttpRequestMethodInvalid,
        ErrorCode::HttpRequestUriInvalid => SerializableHttpErrorCode::HttpRequestUriInvalid,
        ErrorCode::HttpRequestUriTooLong => SerializableHttpErrorCode::HttpRequestUriTooLong,
        ErrorCode::HttpRequestHeaderSectionSize(payload) => {
            SerializableHttpErrorCode::HttpRequestHeaderSectionSize(*payload)
        }
        ErrorCode::HttpRequestHeaderSize(payload) => {
            SerializableHttpErrorCode::HttpRequestHeaderSize(
                payload.as_ref().map(serialize_field_size_payload),
            )
        }
        ErrorCode::HttpRequestTrailerSectionSize(payload) => {
            SerializableHttpErrorCode::HttpRequestTrailerSectionSize(*payload)
        }
        ErrorCode::HttpRequestTrailerSize(payload) => {
            SerializableHttpErrorCode::HttpRequestTrailerSize(serialize_field_size_payload(payload))
        }
        ErrorCode::HttpResponseIncomplete => SerializableHttpErrorCode::HttpResponseIncomplete,
        ErrorCode::HttpResponseHeaderSectionSize(payload) => {
            SerializableHttpErrorCode::HttpResponseHeaderSectionSize(*payload)
        }
        ErrorCode::HttpResponseHeaderSize(payload) => {
            SerializableHttpErrorCode::HttpResponseHeaderSize(serialize_field_size_payload(payload))
        }
        ErrorCode::HttpResponseBodySize(payload) => {
            SerializableHttpErrorCode::HttpResponseBodySize(*payload)
        }
        ErrorCode::HttpResponseTrailerSectionSize(payload) => {
            SerializableHttpErrorCode::HttpResponseTrailerSectionSize(*payload)
        }
        ErrorCode::HttpResponseTrailerSize(payload) => {
            SerializableHttpErrorCode::HttpResponseTrailerSize(serialize_field_size_payload(
                payload,
            ))
        }
        ErrorCode::HttpResponseTransferCoding(payload) => {
            SerializableHttpErrorCode::HttpResponseTransferCoding(payload.clone())
        }
        ErrorCode::HttpResponseContentCoding(payload) => {
            SerializableHttpErrorCode::HttpResponseContentCoding(payload.clone())
        }
        ErrorCode::HttpResponseTimeout => SerializableHttpErrorCode::HttpResponseTimeout,
        ErrorCode::HttpUpgradeFailed => SerializableHttpErrorCode::HttpUpgradeFailed,
        ErrorCode::HttpProtocolError => SerializableHttpErrorCode::HttpProtocolError,
        ErrorCode::LoopDetected => SerializableHttpErrorCode::LoopDetected,
        ErrorCode::ConfigurationError => SerializableHttpErrorCode::ConfigurationError,
        ErrorCode::InternalError(payload) => {
            SerializableHttpErrorCode::InternalError(payload.clone())
        }
    }
}

fn deserialize_error_code(error: SerializableHttpErrorCode) -> ErrorCode {
    match error {
        SerializableHttpErrorCode::DnsTimeout => ErrorCode::DnsTimeout,
        SerializableHttpErrorCode::DnsError(payload) => {
            ErrorCode::DnsError(deserialize_dns_error_payload(payload))
        }
        SerializableHttpErrorCode::DestinationNotFound => ErrorCode::DestinationNotFound,
        SerializableHttpErrorCode::DestinationUnavailable => ErrorCode::DestinationUnavailable,
        SerializableHttpErrorCode::DestinationIpProhibited => ErrorCode::DestinationIpProhibited,
        SerializableHttpErrorCode::DestinationIpUnroutable => ErrorCode::DestinationIpUnroutable,
        SerializableHttpErrorCode::ConnectionRefused => ErrorCode::ConnectionRefused,
        SerializableHttpErrorCode::ConnectionTerminated => ErrorCode::ConnectionTerminated,
        SerializableHttpErrorCode::ConnectionTimeout => ErrorCode::ConnectionTimeout,
        SerializableHttpErrorCode::ConnectionReadTimeout => ErrorCode::ConnectionReadTimeout,
        SerializableHttpErrorCode::ConnectionWriteTimeout => ErrorCode::ConnectionWriteTimeout,
        SerializableHttpErrorCode::ConnectionLimitReached => ErrorCode::ConnectionLimitReached,
        SerializableHttpErrorCode::TlsProtocolError => ErrorCode::TlsProtocolError,
        SerializableHttpErrorCode::TlsCertificateError => ErrorCode::TlsCertificateError,
        SerializableHttpErrorCode::TlsAlertReceived(payload) => {
            ErrorCode::TlsAlertReceived(deserialize_tls_alert_received_payload(payload))
        }
        SerializableHttpErrorCode::HttpRequestDenied => ErrorCode::HttpRequestDenied,
        SerializableHttpErrorCode::HttpRequestLengthRequired => {
            ErrorCode::HttpRequestLengthRequired
        }
        SerializableHttpErrorCode::HttpRequestBodySize(payload) => {
            ErrorCode::HttpRequestBodySize(payload)
        }
        SerializableHttpErrorCode::HttpRequestMethodInvalid => ErrorCode::HttpRequestMethodInvalid,
        SerializableHttpErrorCode::HttpRequestUriInvalid => ErrorCode::HttpRequestUriInvalid,
        SerializableHttpErrorCode::HttpRequestUriTooLong => ErrorCode::HttpRequestUriTooLong,
        SerializableHttpErrorCode::HttpRequestHeaderSectionSize(payload) => {
            ErrorCode::HttpRequestHeaderSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpRequestHeaderSize(payload) => {
            ErrorCode::HttpRequestHeaderSize(payload.map(deserialize_field_size_payload))
        }
        SerializableHttpErrorCode::HttpRequestTrailerSectionSize(payload) => {
            ErrorCode::HttpRequestTrailerSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpRequestTrailerSize(payload) => {
            ErrorCode::HttpRequestTrailerSize(deserialize_field_size_payload(payload))
        }
        SerializableHttpErrorCode::HttpResponseIncomplete => ErrorCode::HttpResponseIncomplete,
        SerializableHttpErrorCode::HttpResponseHeaderSectionSize(payload) => {
            ErrorCode::HttpResponseHeaderSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpResponseHeaderSize(payload) => {
            ErrorCode::HttpResponseHeaderSize(deserialize_field_size_payload(payload))
        }
        SerializableHttpErrorCode::HttpResponseBodySize(payload) => {
            ErrorCode::HttpResponseBodySize(payload)
        }
        SerializableHttpErrorCode::HttpResponseTrailerSectionSize(payload) => {
            ErrorCode::HttpResponseTrailerSectionSize(payload)
        }
        SerializableHttpErrorCode::HttpResponseTrailerSize(payload) => {
            ErrorCode::HttpResponseTrailerSize(deserialize_field_size_payload(payload))
        }
        SerializableHttpErrorCode::HttpResponseTransferCoding(payload) => {
            ErrorCode::HttpResponseTransferCoding(payload)
        }
        SerializableHttpErrorCode::HttpResponseContentCoding(payload) => {
            ErrorCode::HttpResponseContentCoding(payload)
        }
        SerializableHttpErrorCode::HttpResponseTimeout => ErrorCode::HttpResponseTimeout,
        SerializableHttpErrorCode::HttpUpgradeFailed => ErrorCode::HttpUpgradeFailed,
        SerializableHttpErrorCode::HttpProtocolError => ErrorCode::HttpProtocolError,
        SerializableHttpErrorCode::LoopDetected => ErrorCode::LoopDetected,
        SerializableHttpErrorCode::ConfigurationError => ErrorCode::ConfigurationError,
        SerializableHttpErrorCode::InternalError(payload) => ErrorCode::InternalError(payload),
    }
}

fn serialize_dns_error_payload(payload: &types::DnsErrorPayload) -> SerializableDnsErrorPayload {
    SerializableDnsErrorPayload {
        rcode: payload.rcode.clone(),
        info_code: payload.info_code,
    }
}

fn deserialize_dns_error_payload(payload: SerializableDnsErrorPayload) -> types::DnsErrorPayload {
    types::DnsErrorPayload {
        rcode: payload.rcode,
        info_code: payload.info_code,
    }
}

fn serialize_tls_alert_received_payload(
    payload: &types::TlsAlertReceivedPayload,
) -> SerializableTlsAlertReceivedPayload {
    SerializableTlsAlertReceivedPayload {
        alert_id: payload.alert_id,
        alert_message: payload.alert_message.clone(),
    }
}

fn deserialize_tls_alert_received_payload(
    payload: SerializableTlsAlertReceivedPayload,
) -> types::TlsAlertReceivedPayload {
    types::TlsAlertReceivedPayload {
        alert_id: payload.alert_id,
        alert_message: payload.alert_message,
    }
}

fn serialize_field_size_payload(payload: &types::FieldSizePayload) -> SerializableFieldSizePayload {
    SerializableFieldSizePayload {
        field_name: payload.field_name.clone(),
        field_size: payload.field_size,
    }
}

fn deserialize_field_size_payload(
    payload: SerializableFieldSizePayload,
) -> types::FieldSizePayload {
    types::FieldSizePayload {
        field_name: payload.field_name,
        field_size: payload.field_size,
    }
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: HttpError) -> wasmtime::Result<ErrorCode> {
        types::Host::convert_error_code(&mut WasiHttpView::http(self.0), error)
    }

    fn convert_header_error(&mut self, error: HeaderError) -> wasmtime::Result<types::HeaderError> {
        types::Host::convert_header_error(&mut WasiHttpView::http(self.0), error)
    }

    fn convert_request_options_error(
        &mut self,
        error: RequestOptionsError,
    ) -> wasmtime::Result<types::RequestOptionsError> {
        types::Host::convert_request_options_error(&mut WasiHttpView::http(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostFields for DurableP3View<'_, Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<Fields>> {
        types::HostFields::new(&mut WasiHttpView::http(self.0))
    }

    fn from_list(
        &mut self,
        entries: Vec<(FieldName, FieldValue)>,
    ) -> HeaderResult<Resource<Fields>> {
        types::HostFields::from_list(&mut WasiHttpView::http(self.0), entries)
    }

    fn get(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
    ) -> wasmtime::Result<Vec<FieldValue>> {
        types::HostFields::get(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn has(&mut self, fields: Resource<Fields>, name: FieldName) -> wasmtime::Result<bool> {
        types::HostFields::has(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn set(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
        value: Vec<FieldValue>,
    ) -> HeaderResult<()> {
        types::HostFields::set(&mut WasiHttpView::http(self.0), fields, name, value)
    }

    fn delete(&mut self, fields: Resource<Fields>, name: FieldName) -> HeaderResult<()> {
        types::HostFields::delete(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn get_and_delete(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
    ) -> HeaderResult<Vec<FieldValue>> {
        types::HostFields::get_and_delete(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn append(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
        value: FieldValue,
    ) -> HeaderResult<()> {
        types::HostFields::append(&mut WasiHttpView::http(self.0), fields, name, value)
    }

    fn copy_all(
        &mut self,
        fields: Resource<Fields>,
    ) -> wasmtime::Result<Vec<(FieldName, FieldValue)>> {
        types::HostFields::copy_all(&mut WasiHttpView::http(self.0), fields)
    }

    fn clone(&mut self, fields: Resource<Fields>) -> wasmtime::Result<Resource<Fields>> {
        types::HostFields::clone(&mut WasiHttpView::http(self.0), fields)
    }

    fn drop(&mut self, fields: Resource<Fields>) -> wasmtime::Result<()> {
        types::HostFields::drop(&mut WasiHttpView::http(self.0), fields)
    }
}

impl<Ctx: WorkerCtx> types::HostRequest for DurableP3View<'_, Ctx> {
    fn get_method(&mut self, req: Resource<Request>) -> wasmtime::Result<Method> {
        types::HostRequest::get_method(&mut WasiHttpView::http(self.0), req)
    }

    fn set_method(
        &mut self,
        req: Resource<Request>,
        method: Method,
    ) -> wasmtime::Result<Result<(), ()>> {
        types::HostRequest::set_method(&mut WasiHttpView::http(self.0), req, method)
    }

    fn get_path_with_query(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<String>> {
        types::HostRequest::get_path_with_query(&mut WasiHttpView::http(self.0), req)
    }

    fn set_path_with_query(
        &mut self,
        req: Resource<Request>,
        path_with_query: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        types::HostRequest::set_path_with_query(
            &mut WasiHttpView::http(self.0),
            req,
            path_with_query,
        )
    }

    fn get_scheme(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<Scheme>> {
        types::HostRequest::get_scheme(&mut WasiHttpView::http(self.0), req)
    }

    fn set_scheme(
        &mut self,
        req: Resource<Request>,
        scheme: Option<Scheme>,
    ) -> wasmtime::Result<Result<(), ()>> {
        types::HostRequest::set_scheme(&mut WasiHttpView::http(self.0), req, scheme)
    }

    fn get_authority(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<String>> {
        types::HostRequest::get_authority(&mut WasiHttpView::http(self.0), req)
    }

    fn set_authority(
        &mut self,
        req: Resource<Request>,
        authority: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        types::HostRequest::set_authority(&mut WasiHttpView::http(self.0), req, authority)
    }

    fn get_options(
        &mut self,
        req: Resource<Request>,
    ) -> wasmtime::Result<Option<Resource<RequestOptions>>> {
        types::HostRequest::get_options(&mut WasiHttpView::http(self.0), req)
    }

    fn get_headers(&mut self, req: Resource<Request>) -> wasmtime::Result<Resource<Headers>> {
        types::HostRequest::get_headers(&mut WasiHttpView::http(self.0), req)
    }
}

impl<Ctx: WorkerCtx> types::HostRequestWithStore for DurableP3<Ctx> {
    fn new<U>(
        mut store: Access<U, Self>,
        headers: Resource<Headers>,
        contents: Option<StreamReader<u8>>,
        trailers: FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
        options: Option<Resource<RequestOptions>>,
    ) -> wasmtime::Result<(Resource<Request>, FutureReader<Result<(), ErrorCode>>)> {
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostRequestWithStore>::new(store, headers, contents, trailers, options)
    }

    fn consume_body<U>(
        mut store: Access<U, Self>,
        req: Resource<Request>,
        fut: FutureReader<Result<(), ErrorCode>>,
    ) -> wasmtime::Result<(
        StreamReader<u8>,
        FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    )> {
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostRequestWithStore>::consume_body(store, req, fut)
    }

    fn drop<U>(mut store: Access<U, Self>, req: Resource<Request>) -> wasmtime::Result<()> {
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostRequestWithStore>::drop(store, req)
    }
}

impl<Ctx: WorkerCtx> types::HostRequestOptions for DurableP3View<'_, Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<RequestOptions>> {
        types::HostRequestOptions::new(&mut WasiHttpView::http(self.0))
    }

    fn get_connect_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        types::HostRequestOptions::get_connect_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_connect_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        types::HostRequestOptions::set_connect_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn get_first_byte_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        types::HostRequestOptions::get_first_byte_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_first_byte_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        types::HostRequestOptions::set_first_byte_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn get_between_bytes_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        types::HostRequestOptions::get_between_bytes_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_between_bytes_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        types::HostRequestOptions::set_between_bytes_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn clone(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Resource<RequestOptions>> {
        types::HostRequestOptions::clone(&mut WasiHttpView::http(self.0), opts)
    }

    fn drop(&mut self, opts: Resource<RequestOptions>) -> wasmtime::Result<()> {
        types::HostRequestOptions::drop(&mut WasiHttpView::http(self.0), opts)
    }
}

impl<Ctx: WorkerCtx> types::HostResponse for DurableP3View<'_, Ctx> {
    fn get_status_code(&mut self, res: Resource<Response>) -> wasmtime::Result<StatusCode> {
        types::HostResponse::get_status_code(&mut WasiHttpView::http(self.0), res)
    }

    fn set_status_code(
        &mut self,
        res: Resource<Response>,
        status_code: StatusCode,
    ) -> wasmtime::Result<Result<(), ()>> {
        types::HostResponse::set_status_code(&mut WasiHttpView::http(self.0), res, status_code)
    }

    fn get_headers(&mut self, res: Resource<Response>) -> wasmtime::Result<Resource<Headers>> {
        types::HostResponse::get_headers(&mut WasiHttpView::http(self.0), res)
    }
}

/// Result fed to the guest-facing trailers `FutureReader` once the body closes.
type HttpTrailersOutcome = Result<Option<HeaderMap>, ErrorCode>;

/// A demand from the body stream producer to the durable [`HttpConsumeBodyTask`]
/// for the next body chunk, carrying the channel the task replies on.
type HttpBodyDemand = oneshot::Sender<HttpBodyChunkReply>;

/// The task's reply to a single producer demand.
enum HttpBodyChunkReply {
    /// One non-empty body frame, already persisted to the oplog as a `Data`
    /// child chunk before being handed back for delivery to the guest.
    Data(Bytes),
    /// The body stream reached its terminal (clean EOF, trailers, or a body
    /// error); there are no more bytes to deliver. The producer signals `ack`
    /// immediately before it reports EOF to the guest, so the durable task only
    /// resolves trailers (and finalizes the parent marker) once the terminal has
    /// actually been observed by the guest-facing stream.
    End { ack: oneshot::Sender<()> },
    /// A durable failure occurred while persisting/replaying the body; the guest
    /// stream traps with this message, tagged with the failing call scope's trap
    /// context so post-trap retry grouping stays owned by that call.
    Failed {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Resolution delivered to the guest-facing trailers future once the body closes
/// (or the durable task fails before recording the terminal).
enum HttpTrailersResolution {
    /// The body terminal: clean trailers (or a body `ErrorCode`).
    Outcome(HttpTrailersOutcome),
    /// A durability failure: the trailers future traps with this message, tagged
    /// with the failing call scope's trap context.
    Trap {
        message: String,
        trap_context: DurableCallTrapContext,
    },
}

/// Body stream returned to the guest from `consume-body`.
///
/// `consume-body` is a *synchronous* host function but durable persistence is
/// async, so the producer never touches the oplog (or the upstream body)
/// itself. Instead it bridges to the spawned [`HttpConsumeBodyTask`] with a
/// demand/reply protocol: when the guest needs more bytes the producer sends a
/// demand and parks; the task reads (live) or replays (on replay) exactly one
/// body frame, persists/claims it as a child durable call, and replies with the
/// bytes. The whole frame is then handed to the runtime's buffer
/// (`Destination::set_buffer`), which delivers it across however many guest
/// reads and only calls `poll_produce` again once it is fully drained — so
/// exactly one child chunk is produced per real demand, identically live and on
/// replay.
struct DurableHttpBodyProducer {
    demand_tx: mpsc::UnboundedSender<HttpBodyDemand>,
    pending: Option<oneshot::Receiver<HttpBodyChunkReply>>,
    finished: bool,
}

impl DurableHttpBodyProducer {
    fn new(demand_tx: mpsc::UnboundedSender<HttpBodyDemand>) -> Self {
        Self {
            demand_tx,
            pending: None,
            finished: false,
        }
    }
}

impl<D> StreamProducer<D> for DurableHttpBodyProducer {
    type Item = u8;
    type Buffer = Cursor<Bytes>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            if self.finished {
                return Poll::Ready(Ok(StreamResult::Dropped));
            }

            if let Some(rx) = self.pending.as_mut() {
                match Pin::new(rx).poll(cx) {
                    Poll::Pending => {
                        // A demand is in flight: the task has been asked for
                        // (and will durably persist) exactly one chunk. We must
                        // deliver that chunk to a guest read rather than abandon
                        // it, otherwise the recorded child chunk would have no
                        // matching delivery and replay would diverge. So even
                        // when the guest is trying to cancel (`finish`), wait for
                        // the in-flight chunk instead of returning `Cancelled`.
                        // The demand is only ever issued from a positive-capacity
                        // poll (a deterministic guest read), so the demand/child
                        // sequence is identical on replay; the wait, however, is
                        // only bounded if the upstream eventually produces the
                        // frame (or closes) — a stalled upstream blocks
                        // cancellation, matching P2 blocking-read semantics.
                        return Poll::Pending;
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Data(bytes))) => {
                        self.pending = None;
                        if bytes.is_empty() {
                            continue;
                        }
                        // Hand the whole frame to the runtime; it delivers it
                        // across as many guest reads as needed and only calls
                        // us again once it is drained.
                        dst.set_buffer(Cursor::new(bytes));
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::End { ack })) => {
                        self.pending = None;
                        self.finished = true;
                        // Acknowledge the terminal *before* reporting EOF so the
                        // task only resolves trailers after this stream observes
                        // the terminal. A dropped `ack` receiver just means the
                        // task is already gone, which is harmless here.
                        let _ = ack.send(());
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }
                    Poll::Ready(Ok(HttpBodyChunkReply::Failed {
                        message,
                        trap_context,
                    })) => {
                        self.pending = None;
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::from_anyhow(
                            mark_durable_call_trap_context(
                                anyhow::Error::msg(message),
                                trap_context,
                            ),
                        )));
                    }
                    Poll::Ready(Err(_)) => {
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "consume-body durable task dropped before replying",
                        )));
                    }
                }
            }

            // No demand in flight.
            if dst.remaining(&mut store) == Some(0) {
                // Zero-length read: the guest is probing readiness, not reading.
                // Do not turn this into a durable body read.
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            if finish {
                // The guest is cancelling a read and we have nothing buffered
                // and no demand in flight: report a cancelled (empty) read
                // without starting a new durable body read.
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }

            let (tx, rx) = oneshot::channel();
            if self.demand_tx.send(tx).is_err() {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(
                    "consume-body durable task is gone",
                )));
            }
            self.pending = Some(rx);
            // Loop to register the receiver's waker (the reply is not ready yet).
        }
    }
}

/// Guest-facing trailers `FutureReader` producer. Awaits the terminal trailers
/// from the durable task and, only when read, materializes a `trailers`
/// resource in the store table.
struct HttpTrailersFutureProducer<Ctx, U> {
    rx: oneshot::Receiver<HttpTrailersResolution>,
    _phantom: PhantomData<fn() -> (Ctx, U)>,
}

impl<Ctx, U> HttpTrailersFutureProducer<Ctx, U> {
    fn new(rx: oneshot::Receiver<HttpTrailersResolution>) -> Self {
        Self {
            rx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> FutureProducer<U> for HttpTrailersFutureProducer<Ctx, U>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    type Item = Result<Option<Resource<Trailers>>, ErrorCode>;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<U>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        let this = self.get_mut();
        match Pin::new(&mut this.rx).poll(cx) {
            Poll::Pending if finish => Poll::Ready(Ok(None)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(HttpTrailersResolution::Outcome(outcome))) => {
                let item = match outcome {
                    Ok(None) => Ok(None),
                    Ok(Some(headers)) => {
                        let view = wasi_http_view::<Ctx, U>(store.data_mut());
                        match view.table.push(FieldMap::new_immutable(headers)) {
                            Ok(resource) => Ok(Some(resource)),
                            Err(err) => {
                                return Poll::Ready(Err(wasmtime::Error::from(err)
                                    .context("failed to push consume-body trailers to table")));
                            }
                        }
                    }
                    Err(error) => Err(error),
                };
                Poll::Ready(Ok(Some(item)))
            }
            // A durability failure occurred before the terminal was recorded: the
            // trailers future must trap (carrying the failing call scope's trap
            // context) rather than resolve to a normal error that would mask it.
            Poll::Ready(Ok(HttpTrailersResolution::Trap {
                message,
                trap_context,
            })) => Poll::Ready(Err(wasmtime::Error::from_anyhow(
                mark_durable_call_trap_context(anyhow::Error::msg(message), trap_context),
            ))),
            // The channel is closed without any resolution: the durable task was
            // aborted before sending. On the normal path the task always sends a
            // resolution before dropping the sender, so a closed channel here is
            // a durability failure and must trap rather than resolve to a normal
            // error that would mask it.
            Poll::Ready(Err(_)) => Poll::Ready(Err(wasmtime::Error::msg(
                "consume-body durable task dropped before resolving trailers",
            ))),
        }
    }
}

fn serialize_consume_body_result(
    result: &Result<Option<HeaderMap>, ErrorCode>,
) -> SerializableP3HttpConsumeBodyResult {
    match result {
        Ok(trailers) => {
            SerializableP3HttpConsumeBodyResult::Trailers(trailers.as_ref().map(serialize_headers))
        }
        Err(error) => SerializableP3HttpConsumeBodyResult::HttpError(serialize_error_code(error)),
    }
}

fn deserialize_consume_body_result(
    result: SerializableP3HttpConsumeBodyResult,
) -> Result<Option<HeaderMap>, ErrorCode> {
    match result {
        SerializableP3HttpConsumeBodyResult::Trailers(trailers) => {
            Ok(trailers.map(deserialize_headers))
        }
        SerializableP3HttpConsumeBodyResult::HttpError(error) => Err(deserialize_error_code(error)),
    }
}

/// Fail the durable `consume-body` task loudly on a durability-machinery error
/// (an oplog read/write failure), as opposed to a normal HTTP body error.
///
/// A durability failure must not be turned into a normal terminal: doing so
/// would commit a completed parent marker sitting after an incomplete child
/// chunk (a malformed oplog). Instead we return `Err` from the task, which the
/// runtime surfaces as a trap. The parent batched scope is left without a
/// terminal marker (the caller abandons/traps the parent handle so a `Cancelled`
/// is never written), so on replay the worker recovers from the incomplete
/// `Start` rather than observing committed-but-corrupt durable state.
///
/// The `error` must already carry the failing call's [`DurableCallTrapContext`]
/// (via `CallHandle::trap`, a `TerminalCallError`, or `mark_durable_call_trap_context`)
/// so post-trap retry grouping stays owned by that call's scope; this helper does
/// not stringify it for the returned trap.
///
/// The guest-facing trailers future is resolved with a [`HttpTrailersResolution::Trap`]
/// carrying `trap_context` (the failing call scope's context) so it also fails
/// loud — with correct retry grouping — instead of resolving to a normal error
/// that would mask the durability failure. When `trap_context` is `None` (no
/// owning call scope exists yet) the sender is dropped, which still traps the
/// trailers future loudly.
fn fail_consume_body_task(
    trailers_tx: oneshot::Sender<HttpTrailersResolution>,
    error: wasmtime::Error,
    trap_context: Option<DurableCallTrapContext>,
) -> wasmtime::Result<()> {
    match trap_context {
        Some(trap_context) => {
            // The detailed cause is preserved in the returned (marked) task error;
            // give the guest-facing trailers trap a clear, stable message rather
            // than re-displaying the trap-context marker carried by `error`.
            let _ = trailers_tx.send(HttpTrailersResolution::Trap {
                message: "consume-body durable persistence failed".to_string(),
                trap_context,
            });
        }
        None => drop(trailers_tx),
    }
    Err(error)
}

fn serialize_headers(headers: &HeaderMap) -> HashMap<String, Vec<Vec<u8>>> {
    let mut serialized: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    for (name, value) in headers.iter() {
        serialized
            .entry(name.as_str().to_string())
            .or_default()
            .push(value.as_bytes().to_vec());
    }
    serialized
}

fn deserialize_headers(headers: HashMap<String, Vec<Vec<u8>>>) -> HeaderMap {
    let mut header_map = HeaderMap::new();
    for (name, values) in headers {
        let Ok(name) = HeaderName::try_from(name) else {
            continue;
        };
        for value in values {
            if let Ok(value) = HeaderValue::try_from(value) {
                header_map.append(name.clone(), value);
            }
        }
    }
    header_map
}

/// One unit read from the upstream response body by the durable task.
enum HttpBodyFrame {
    /// A non-empty data frame.
    Data(Bytes),
    /// The body closed cleanly, optionally delivering trailers.
    End(Option<HeaderMap>),
    /// The body transfer errored.
    Error(ErrorCode),
}

/// One item produced by a single iteration of the durable consume-body loop —
/// after the chunk has been persisted (live) or replayed (replay) — to be
/// delivered to the guest-facing body stream.
enum ProducedChunk {
    /// A non-empty body chunk to hand to the guest.
    Data(Bytes),
    /// The recorded stream's terminal: there are no more chunks to deliver.
    Terminal,
}

/// Reads the next meaningful frame from the upstream body, skipping empty data
/// frames so an empty frame is never persisted/delivered as a body chunk.
async fn read_http_body_frame(body: &mut UnsyncBoxBody<Bytes, ErrorCode>) -> HttpBodyFrame {
    loop {
        match body.frame().await {
            Some(Ok(frame)) => match frame.into_data() {
                Ok(data) => {
                    if data.is_empty() {
                        continue;
                    }
                    return HttpBodyFrame::Data(data);
                }
                Err(frame) => match frame.into_trailers() {
                    Ok(trailers) => return HttpBodyFrame::End(Some(trailers)),
                    Err(_) => return HttpBodyFrame::Error(ErrorCode::HttpProtocolError),
                },
            },
            Some(Err(err)) => return HttpBodyFrame::Error(err),
            None => return HttpBodyFrame::End(None),
        }
    }
}

/// Durable driver for a `consume-body` response stream.
///
/// Owns the upstream body and persists it **chunk-by-chunk** under a single
/// `consume-body` batched durable scope (mirroring the P2 incoming-body stream):
///
/// * the parent `P3HttpClientConsumeBody` call opens the batched scope and is
///   finalized last with a marker carrying the trailers / body-error terminal;
/// * every delivered body frame is persisted as a `P3HttpClientConsumeBodyChunk`
///   child (`Data`) before its bytes are handed to the guest;
/// * a final `End` child terminates the recorded stream so replay knows when to
///   stop reading children.
///
/// Each child is produced in response to exactly one producer demand, so on
/// replay the same number of children are read back from the oplog and delivered
/// in the same order — no whole-body buffering, bounded memory.
struct HttpConsumeBodyTask<Ctx> {
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    demand_rx: mpsc::UnboundedReceiver<HttpBodyDemand>,
    trailers_tx: oneshot::Sender<HttpTrailersResolution>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpConsumeBodyTask<Ctx> {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        demand_rx: mpsc::UnboundedReceiver<HttpBodyDemand>,
        trailers_tx: oneshot::Sender<HttpTrailersResolution>,
    ) -> Self {
        Self {
            body,
            demand_rx,
            trailers_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for HttpConsumeBodyTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let HttpConsumeBodyTask {
            mut body,
            mut demand_rx,
            trailers_tx,
            ..
        } = self;

        // Open the parent batched scope. Children nest under its begin index.
        let mut parent = match CallHandle::<P3HttpClientConsumeBody, Cancellable>::start_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemoteBatched(None),
        )
        .await
        {
            Ok(parent) => parent,
            // No parent handle exists yet, so there is nothing to abandon; the
            // `WorkerExecutorError` carries no call context but there is no scope
            // to group against either.
            Err(error) => {
                return fail_consume_body_task(trailers_tx, wasmtime::Error::from(error), None);
            }
        };
        let parent_begin = parent.begin_index();

        // The trailers / body-error terminal, set on the live path; on replay it
        // is taken from the parent marker instead.
        let mut terminal: HttpTrailersOutcome = Ok(None);

        loop {
            let demand = demand_rx.recv().await;

            let child =
                match CallHandle::<P3HttpClientConsumeBodyChunk, NotCancellable>::start_access(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostRequestNoInput {},
                    DurableFunctionType::WriteRemoteBatched(Some(parent_begin)),
                )
                .await
                {
                    Ok(child) => child,
                    Err(error) => {
                        // Durable-machinery failure (not an HTTP body error): surface
                        // it to the in-flight guest read and fail the task. No child
                        // `Start` was persisted; `parent.trap` abandons the parent so
                        // it never records a `Cancelled` (a trap is not a
                        // cancellation) and tags the error with the parent scope's
                        // trap context for correct retry grouping.
                        let trap_context = parent.trap_context();
                        if let Some(reply_tx) = demand {
                            let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                                message: error.to_string(),
                                trap_context,
                            });
                        }
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(error)),
                            Some(trap_context),
                        );
                    }
                };

            // Produce the next item: replay the recorded child (replay) or read
            // the upstream body and persist it (live). Delivery to the guest-facing
            // stream happens afterwards, identically on both paths.
            let produced = if !child.is_live() {
                match child
                    .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
                    .await
                {
                    Ok(CallReplayOutcome::Replayed(response)) => match response.chunk {
                        SerializableP3HttpBodyChunk::Data(bytes) => {
                            ProducedChunk::Data(Bytes::from(bytes))
                        }
                        SerializableP3HttpBodyChunk::End => ProducedChunk::Terminal,
                    },
                    Ok(CallReplayOutcome::Incomplete(mut child)) => {
                        // A batched (`WriteRemoteBatched(Some(..))`) child is not
                        // re-executable: `replay_access` hard-errors on an
                        // incomplete `Start` rather than returning `Incomplete`,
                        // so this arm is not reachable in normal operation. Treat
                        // it defensively: abandon the live child handle (a trap is
                        // not a cancellation) so it is not dropped unfinished, then
                        // trap the parent.
                        child.abandon_for_trap();
                        let message =
                            "consume-body chunk replay returned an unexpected incomplete child"
                                .to_string();
                        let trap_context = parent.trap_context();
                        if let Some(reply_tx) = demand {
                            let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                                message: message.clone(),
                                trap_context,
                            });
                        }
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(anyhow::Error::msg(message))),
                            Some(trap_context),
                        );
                    }
                    Err(error) => {
                        let trap_context = parent.trap_context();
                        if let Some(reply_tx) = demand {
                            let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                                message: error.to_string(),
                                trap_context,
                            });
                        }
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(parent.trap(error)),
                            Some(trap_context),
                        );
                    }
                }
            } else {
                // When the producer is already gone (guest dropped the stream) we
                // terminate the recorded stream with an `End` child instead of
                // reading more of the upstream body — and we must not start a new
                // upstream read whose persisted chunk could never be delivered.
                let producer_gone = demand
                    .as_ref()
                    .map(|reply_tx| reply_tx.is_closed())
                    .unwrap_or(true);
                let frame = if producer_gone {
                    HttpBodyFrame::End(None)
                } else {
                    read_http_body_frame(&mut body).await
                };

                let chunk = match &frame {
                    HttpBodyFrame::Data(bytes) => SerializableP3HttpBodyChunk::Data(bytes.to_vec()),
                    HttpBodyFrame::End(_) | HttpBodyFrame::Error(_) => {
                        SerializableP3HttpBodyChunk::End
                    }
                };

                if let Err(error) = child
                    .complete_access(
                        accessor,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3HttpClientConsumeBodyChunk { chunk },
                    )
                    .await
                {
                    // The child `Start` is already persisted but its `End` failed:
                    // the recorded chunk history is now incomplete. Fail the task
                    // loud rather than papering over it with a normal terminal and a
                    // completed parent marker, which would commit a malformed oplog.
                    // `complete_access` already finished the child handle without
                    // recording a `Cancelled` and its `TerminalCallError` carries the
                    // child scope's trap context, so preserve that error; we only need
                    // to abandon the still-open parent so it is not dropped unfinished
                    // (which would wrongly record a parent `Cancelled`).
                    let trap_context = parent.trap_context();
                    if let Some(reply_tx) = demand {
                        let _ = reply_tx.send(HttpBodyChunkReply::Failed {
                            message: error.to_string(),
                            trap_context,
                        });
                    }
                    parent.abandon_for_trap();
                    return fail_consume_body_task(
                        trailers_tx,
                        wasmtime::Error::from(error),
                        Some(trap_context),
                    );
                }

                match frame {
                    HttpBodyFrame::Data(bytes) => ProducedChunk::Data(bytes),
                    HttpBodyFrame::End(trailers) => {
                        terminal = Ok(trailers);
                        ProducedChunk::Terminal
                    }
                    HttpBodyFrame::Error(error) => {
                        terminal = Err(error);
                        ProducedChunk::Terminal
                    }
                }
            };

            // Deliver the produced item to the guest-facing stream. This is the
            // single point where chunks reach the guest, identically live and on
            // replay, so the count/order of delivered chunks always matches the
            // count/order of persisted children.
            match produced {
                ProducedChunk::Data(bytes) => match demand {
                    Some(reply_tx) => {
                        if reply_tx.send(HttpBodyChunkReply::Data(bytes)).is_err() {
                            // The chunk was persisted but the producer vanished
                            // before it could be delivered. The recorded stream
                            // would diverge on replay (where the chunk *would* be
                            // delivered), so fail loud instead of finalizing the
                            // parent with a clean terminal over an undelivered chunk.
                            let trap_context = parent.trap_context();
                            parent.abandon_for_trap();
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                                    anyhow::Error::msg(
                                        "consume-body persisted a body chunk that could not be \
                                         delivered to the guest stream",
                                    ),
                                    trap_context,
                                )),
                                Some(trap_context),
                            );
                        }
                    }
                    None => {
                        // A `Data` item is only ever produced in response to a
                        // demand, so a missing demand here is a protocol invariant
                        // violation rather than a clean stream end.
                        let trap_context = parent.trap_context();
                        parent.abandon_for_trap();
                        return fail_consume_body_task(
                            trailers_tx,
                            wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                                anyhow::Error::msg(
                                    "consume-body produced a body chunk without a pending demand",
                                ),
                                trap_context,
                            )),
                            Some(trap_context),
                        );
                    }
                },
                ProducedChunk::Terminal => {
                    if let Some(reply_tx) = demand {
                        let (ack_tx, ack_rx) = oneshot::channel();
                        if reply_tx
                            .send(HttpBodyChunkReply::End { ack: ack_tx })
                            .is_ok()
                        {
                            // Wait for the producer to observe the terminal (report
                            // EOF to the guest) before resolving trailers / finalizing
                            // the parent, so trailers never surface before the body
                            // stream's terminal is observed.
                            let _ = ack_rx.await;
                        }
                    }
                    break;
                }
            }
        }

        // Drop the upstream body so a partially-consumed (or replayed-empty)
        // body closes its network read promptly.
        drop(body);

        // Finalize the parent with the terminal marker. The parent always
        // completes with a marker on the normal path; the `Cancellable` policy
        // exists only for the crash/drop contract (task dropped without
        // finishing), handled by the call handle's drop machinery.
        //
        // Capture the parent scope's trap context first (it is a pure function of
        // the scope and survives the handle being consumed below) so every
        // finalize failure can tag the guest-facing trailers trap for correct
        // retry grouping.
        let parent_trap_context = parent.trap_context();
        let outcome = if parent.is_live() {
            match parent
                .complete_access(
                    accessor,
                    durable_worker_ctx::<Ctx, U>,
                    HostResponseP3HttpClientConsumeBodyResult {
                        result: serialize_consume_body_result(&terminal),
                    },
                )
                .await
            {
                Ok(response) => deserialize_consume_body_result(response.result),
                // `complete_access` consumed and finished the parent without
                // recording a `Cancelled`; its `TerminalCallError` carries the
                // parent scope's trap context, so preserve it.
                Err(error) => {
                    return fail_consume_body_task(
                        trailers_tx,
                        wasmtime::Error::from(error),
                        Some(parent_trap_context),
                    );
                }
            }
        } else {
            match parent
                .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
                .await
            {
                Ok(CallReplayOutcome::Replayed(response)) => {
                    deserialize_consume_body_result(response.result)
                }
                Ok(CallReplayOutcome::Incomplete(parent)) => {
                    match parent
                        .complete_access(
                            accessor,
                            durable_worker_ctx::<Ctx, U>,
                            HostResponseP3HttpClientConsumeBodyResult {
                                result: serialize_consume_body_result(&terminal),
                            },
                        )
                        .await
                    {
                        Ok(response) => deserialize_consume_body_result(response.result),
                        Err(error) => {
                            return fail_consume_body_task(
                                trailers_tx,
                                wasmtime::Error::from(error),
                                Some(parent_trap_context),
                            );
                        }
                    }
                }
                Err(error) => {
                    return fail_consume_body_task(
                        trailers_tx,
                        wasmtime::Error::from_anyhow(mark_durable_call_trap_context(
                            anyhow::Error::from(error),
                            parent_trap_context,
                        )),
                        Some(parent_trap_context),
                    );
                }
            }
        };

        let _ = trailers_tx.send(HttpTrailersResolution::Outcome(outcome));
        Ok(())
    }
}

impl<Ctx: WorkerCtx> types::HostResponseWithStore for DurableP3<Ctx> {
    fn new<U>(
        mut store: Access<U, Self>,
        headers: Resource<Headers>,
        contents: Option<StreamReader<u8>>,
        trailers: FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    ) -> wasmtime::Result<(Resource<Response>, FutureReader<Result<(), ErrorCode>>)> {
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostResponseWithStore>::new(store, headers, contents, trailers)
    }

    fn consume_body<U>(
        mut store: Access<U, Self>,
        res: Resource<Response>,
        fut: FutureReader<Result<(), ErrorCode>>,
    ) -> wasmtime::Result<(
        StreamReader<u8>,
        FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    )> {
        // Delegate to the built-in implementation to wire `fut` into the body's
        // transmission-result channel and to build the host body stream.
        let (upstream_stream, mut upstream_trailers) = {
            let http_store =
                Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
            <WasiHttp as types::HostResponseWithStore>::consume_body(http_store, res, fut)?
        };

        // Recover the host body producer so we can drive and record the body
        // transfer ourselves. Responses obtained from `client.send` (live or
        // replayed) always carry a host-constructed body, so this succeeds.
        let body =
            match upstream_stream.try_into::<HostBodyStreamProducer<U>>(store.as_context_mut()) {
                Ok(mut producer) => {
                    let body = producer.take_body();
                    // Dropping the now-empty producer resolves the upstream
                    // trailers future (`Ok(None)`), which we discard below.
                    drop(producer);
                    body
                }
                Err(stream) => {
                    // Guest-constructed response body (not from `send`): fall back
                    // to the non-durable passthrough.
                    return Ok((stream, upstream_trailers));
                }
            };

        // We surface trailers through our own future, so discard the built-in
        // trailers future.
        upstream_trailers.close(store.as_context_mut())?;

        let (demand_tx, demand_rx) = mpsc::unbounded_channel();
        let (trailers_tx, trailers_rx) = oneshot::channel();

        // Build both guest-facing handles before spawning the durable task. The
        // task appends the `consume-body` `Start`; the guest cannot poll either
        // handle until this host call returns, so spawning first would risk
        // committing a `Start` with no terminal (orphaned `Start`) if a later
        // handle construction fails.
        let mut stream = StreamReader::new(&mut store, DurableHttpBodyProducer::new(demand_tx))?;
        let trailers = match FutureReader::new(
            &mut store,
            HttpTrailersFutureProducer::<Ctx, U>::new(trailers_rx),
        ) {
            Ok(trailers) => trailers,
            Err(err) => {
                let _ = stream.close(store.as_context_mut());
                return Err(err);
            }
        };

        store.spawn(HttpConsumeBodyTask::<Ctx>::new(
            body,
            demand_rx,
            trailers_tx,
        ));
        Ok((stream, trailers))
    }

    fn drop<U>(mut store: Access<U, Self>, res: Resource<Response>) -> wasmtime::Result<()> {
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostResponseWithStore>::drop(store, res)
    }
}
