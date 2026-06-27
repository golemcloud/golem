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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, Cancellable};
use crate::durable_host::p3::{DurableP3, DurableP3View, durable_worker_ctx, wasi_http_view};
use crate::workerctx::WorkerCtx;
use anyhow::Context as _;
use bytes::Bytes;
use golem_common::model::oplog::host_functions::{P3HttpClientConsumeBody, P3HttpClientSend};
use golem_common::model::oplog::payload::types::{
    SerializableDnsErrorPayload, SerializableFieldSizePayload, SerializableHttpErrorCode,
    SerializableHttpMethod, SerializableP3HttpClientSend, SerializableP3HttpClientSendResult,
    SerializableP3HttpConsumeBodyResult, SerializableP3HttpRequestOptions, SerializableP3HttpScheme,
    SerializableResponseHeaders, SerializableTlsAlertReceivedPayload,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostRequestP3HttpClientSend,
    HostResponseP3HttpClientConsumeBodyResult, HostResponseP3HttpClientSendResult,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use http_body::Body as _;
use http_body_util::Empty;
use http_body_util::combinators::UnsyncBoxBody;
use std::collections::HashMap;
use std::io::Cursor;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::oneshot;
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

const HTTP_BODY_STREAM_BUFFER_CAPACITY: usize = 8192;

/// Runtime capture of a consumed response body, sent from the body stream
/// producer to the durable [`HttpConsumeBodyTask`] once the stream terminates
/// or is dropped.
struct CapturedHttpBody {
    /// Every byte pulled from the upstream body, in order. Recorded into the
    /// `consume-body` End payload (and replayed regardless of `result`, so a
    /// partial transfer that later errored still replays its observed bytes).
    ///
    /// On cancellation this is the set of bytes *pulled from upstream*, which
    /// can be a superset of the bytes the guest actually read (a frame larger
    /// than the guest's read buffer is recorded whole, with the surplus handed
    /// to the stream's buffer for later delivery). This is sound because the
    /// recorded bytes are replayed lazily: the deterministic guest reads the
    /// same prefix on replay and never observes the unread surplus.
    contents: Vec<u8>,
    /// How the body terminated: `Ok(None)` clean EOF without trailers,
    /// `Ok(Some(..))` clean EOF with trailers, `Err(..)` a body transfer error.
    result: Result<Option<HeaderMap>, ErrorCode>,
    /// Whether the body reached a natural terminal (`true`) or the guest
    /// dropped the stream before it completed (`false`). A non-completed body
    /// records a `Cancelled` terminal carrying the partial bytes observed so
    /// far.
    completed: bool,
}

/// The mode the deferred body producer should switch into, decided by the
/// durable task once it knows whether this `consume-body` is live or replayed.
enum HttpBodyMode {
    /// Drive (and record) the live upstream body held by the producer.
    Live,
    /// Replay the recorded body bytes from the oplog.
    Replayed(Vec<u8>),
    /// The durable call could not be started/replayed; fail the stream.
    Error(String),
}

/// Result fed to the guest-facing trailers `FutureReader` once the body closes.
type HttpTrailersOutcome = Result<Option<HeaderMap>, ErrorCode>;

/// Live body stream producer: drives the host body taken from the upstream
/// response, copies bytes to the guest, and records every byte pulled plus the
/// terminal trailers/error so the durable task can persist them.
struct HostHttpBodyProducer {
    body: UnsyncBoxBody<Bytes, ErrorCode>,
    contents: Vec<u8>,
    result_tx: Option<oneshot::Sender<CapturedHttpBody>>,
}

impl HostHttpBodyProducer {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        result_tx: oneshot::Sender<CapturedHttpBody>,
    ) -> Self {
        Self {
            body,
            contents: Vec::new(),
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self, result: Result<Option<HeaderMap>, ErrorCode>, completed: bool) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(CapturedHttpBody {
                contents: std::mem::take(&mut self.contents),
                result,
                completed,
            });
        }
    }
}

impl<D> StreamProducer<D> for HostHttpBodyProducer {
    type Item = u8;
    type Buffer = Cursor<Bytes>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let res: Result<Option<HeaderMap>, ErrorCode> = 'result: {
            let cap = match dst.remaining(&mut store).map(NonZeroUsize::new) {
                Some(Some(cap)) => Some(cap),
                Some(None) => {
                    if self.body.is_end_stream() {
                        break 'result Ok(None);
                    } else {
                        None
                    }
                }
                None => None,
            };
            loop {
                match Pin::new(&mut self.body).poll_frame(cx) {
                    Poll::Ready(Some(Ok(frame))) => {
                        match frame.into_data().map_err(http_body::Frame::into_trailers) {
                            Ok(mut frame) => {
                                if frame.is_empty() {
                                    if self.body.is_end_stream() {
                                        break 'result Ok(None);
                                    }
                                    continue;
                                }
                                self.contents.extend_from_slice(&frame);
                                if let Some(cap) = cap {
                                    let n = frame.len();
                                    let cap = cap.into();
                                    if n > cap {
                                        dst.set_buffer(Cursor::new(frame.split_off(cap)));
                                        let mut dst = dst.as_direct(store, cap);
                                        dst.remaining().copy_from_slice(&frame);
                                        dst.mark_written(cap);
                                    } else {
                                        let mut dst = dst.as_direct(store, n);
                                        dst.remaining()[..n].copy_from_slice(&frame);
                                        dst.mark_written(n);
                                    }
                                } else {
                                    dst.set_buffer(Cursor::new(frame));
                                }
                                return Poll::Ready(Ok(StreamResult::Completed));
                            }
                            Err(Ok(trailers)) => break 'result Ok(Some(trailers)),
                            Err(Err(..)) => break 'result Err(ErrorCode::HttpProtocolError),
                        }
                    }
                    Poll::Ready(Some(Err(err))) => break 'result Err(err),
                    Poll::Ready(None) => break 'result Ok(None),
                    Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                    Poll::Pending => return Poll::Pending,
                }
            }
        };
        self.close(res, true);
        Poll::Ready(Ok(StreamResult::Dropped))
    }
}

impl Drop for HostHttpBodyProducer {
    fn drop(&mut self) {
        // Reached only when the guest dropped the stream before a natural
        // terminal (otherwise `result_tx` was already taken by `close`).
        self.close(Ok(None), false);
    }
}

/// Replay body stream producer: yields the recorded body bytes from the oplog
/// and signals the durable task once they have all been delivered.
struct RecordedHttpBodyProducer {
    contents: Cursor<Bytes>,
    result_tx: Option<oneshot::Sender<CapturedHttpBody>>,
}

impl RecordedHttpBodyProducer {
    fn new(contents: Vec<u8>, result_tx: oneshot::Sender<CapturedHttpBody>) -> Self {
        Self {
            contents: Cursor::new(Bytes::from(contents)),
            result_tx: Some(result_tx),
        }
    }

    fn close(&mut self, completed: bool) {
        if let Some(result_tx) = self.result_tx.take() {
            // On replay the terminal trailers/error come from the recorded End
            // payload, so only the delivery signal matters here.
            let _ = result_tx.send(CapturedHttpBody {
                contents: Vec::new(),
                result: Ok(None),
                completed,
            });
        }
    }
}

impl<D> StreamProducer<D> for RecordedHttpBodyProducer {
    type Item = u8;
    type Buffer = Cursor<Bytes>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let bytes = self.contents.get_ref().clone();
        let position = self.contents.position() as usize;
        if position >= bytes.len() {
            self.close(true);
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        let mut dst = dst.as_direct(store, HTTP_BODY_STREAM_BUFFER_CAPACITY);
        let remaining = &bytes[position..];
        let n = remaining.len().min(dst.remaining().len());
        dst.remaining()[..n].copy_from_slice(&remaining[..n]);
        dst.mark_written(n);
        self.contents.set_position((position + n) as u64);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for RecordedHttpBodyProducer {
    fn drop(&mut self) {
        self.close(false);
    }
}

enum DeferredHttpBodyProducerState {
    Awaiting {
        mode_rx: oneshot::Receiver<HttpBodyMode>,
        body: Option<UnsyncBoxBody<Bytes, ErrorCode>>,
        result_tx: Option<oneshot::Sender<CapturedHttpBody>>,
    },
    Live(HostHttpBodyProducer),
    Replay(RecordedHttpBodyProducer),
    Done,
}

/// Body stream returned to the guest from `consume-body`. It waits for the
/// durable task to decide live-vs-replay, then delegates to the matching
/// producer. This indirection is required because `consume-body` is a
/// synchronous host function: the durable `Start` is appended (and liveness
/// determined) by the spawned [`HttpConsumeBodyTask`], not at call time.
struct DeferredHttpBodyProducer {
    state: DeferredHttpBodyProducerState,
}

impl DeferredHttpBodyProducer {
    fn new(
        body: UnsyncBoxBody<Bytes, ErrorCode>,
        mode_rx: oneshot::Receiver<HttpBodyMode>,
        result_tx: oneshot::Sender<CapturedHttpBody>,
    ) -> Self {
        Self {
            state: DeferredHttpBodyProducerState::Awaiting {
                mode_rx,
                body: Some(body),
                result_tx: Some(result_tx),
            },
        }
    }
}

impl<D> StreamProducer<D> for DeferredHttpBodyProducer {
    type Item = u8;
    type Buffer = Cursor<Bytes>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        loop {
            match &mut self.state {
                DeferredHttpBodyProducerState::Awaiting {
                    mode_rx,
                    body,
                    result_tx,
                } => match Pin::new(mode_rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(HttpBodyMode::Live)) => {
                        let body = body
                            .take()
                            .expect("live http body available for incomplete replay");
                        let result_tx = result_tx
                            .take()
                            .expect("http body result sender available for live consume-body");
                        self.state = DeferredHttpBodyProducerState::Live(
                            HostHttpBodyProducer::new(body, result_tx),
                        );
                    }
                    Poll::Ready(Ok(HttpBodyMode::Replayed(contents))) => {
                        let result_tx = result_tx
                            .take()
                            .expect("http body result sender available for replayed consume-body");
                        self.state = DeferredHttpBodyProducerState::Replay(
                            RecordedHttpBodyProducer::new(contents, result_tx),
                        );
                    }
                    Poll::Ready(Ok(HttpBodyMode::Error(error))) => {
                        self.state = DeferredHttpBodyProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg(error)));
                    }
                    Poll::Ready(Err(_)) => {
                        self.state = DeferredHttpBodyProducerState::Done;
                        return Poll::Ready(Err(wasmtime::Error::msg(
                            "consume-body durable task dropped",
                        )));
                    }
                },
                DeferredHttpBodyProducerState::Live(producer) => {
                    return Pin::new(producer).poll_produce(cx, store, dst, finish);
                }
                DeferredHttpBodyProducerState::Replay(producer) => {
                    return Pin::new(producer).poll_produce(cx, store, dst, finish);
                }
                DeferredHttpBodyProducerState::Done => {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
            }
        }
    }
}

impl Drop for DeferredHttpBodyProducer {
    fn drop(&mut self) {
        if let DeferredHttpBodyProducerState::Awaiting {
            body, result_tx, ..
        } = &mut self.state
        {
            // Dropping the held body closes the upstream network read.
            let _ = body.take();
            if let Some(result_tx) = result_tx.take() {
                let _ = result_tx.send(CapturedHttpBody {
                    contents: Vec::new(),
                    result: Ok(None),
                    completed: false,
                });
            }
        }
    }
}

/// Guest-facing trailers `FutureReader` producer. Awaits the terminal trailers
/// from the durable task and, only when read, materializes a `trailers`
/// resource in the store table.
struct HttpTrailersFutureProducer<Ctx, U> {
    rx: oneshot::Receiver<HttpTrailersOutcome>,
    _phantom: PhantomData<fn() -> (Ctx, U)>,
}

impl<Ctx, U> HttpTrailersFutureProducer<Ctx, U> {
    fn new(rx: oneshot::Receiver<HttpTrailersOutcome>) -> Self {
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
            Poll::Ready(Ok(outcome)) => {
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
            Poll::Ready(Err(_)) => Poll::Ready(Ok(Some(Err(ErrorCode::InternalError(Some(
                "consume-body durable task dropped before resolving trailers".to_string(),
            )))))),
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
        SerializableP3HttpConsumeBodyResult::HttpError(error) => {
            Err(deserialize_error_code(error))
        }
    }
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

struct HttpConsumeBodyTask<Ctx> {
    mode_tx: oneshot::Sender<HttpBodyMode>,
    stream_rx: oneshot::Receiver<CapturedHttpBody>,
    trailers_tx: oneshot::Sender<HttpTrailersOutcome>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> HttpConsumeBodyTask<Ctx> {
    fn new(
        mode_tx: oneshot::Sender<HttpBodyMode>,
        stream_rx: oneshot::Receiver<CapturedHttpBody>,
        trailers_tx: oneshot::Sender<HttpTrailersOutcome>,
    ) -> Self {
        Self {
            mode_tx,
            stream_rx,
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
            mode_tx,
            stream_rx,
            trailers_tx,
            ..
        } = self;

        let call = match CallHandle::<P3HttpClientConsumeBody, Cancellable>::start_access(
            accessor,
            durable_worker_ctx::<Ctx, U>,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await
        {
            Ok(call) => call,
            Err(error) => {
                let error = error.to_string();
                let _ = mode_tx.send(HttpBodyMode::Error(error.clone()));
                let _ = trailers_tx.send(Err(ErrorCode::InternalError(Some(error))));
                return Ok(());
            }
        };

        if call.is_live() {
            let _ = mode_tx.send(HttpBodyMode::Live);
            let outcome =
                complete_http_consume_body::<Ctx, U>(accessor, call, stream_rx, &trailers_tx)
                    .await?;
            let _ = trailers_tx.send(outcome);
            return Ok(());
        }

        match call
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
        {
            Ok(CallReplayOutcome::Replayed(response)) => {
                let _ = mode_tx.send(HttpBodyMode::Replayed(response.contents));
                // Wait for the recorded body to finish being delivered to the
                // guest before resolving the trailers future (per the WIT
                // contract: trailers resolve only after the stream closes).
                let _ = stream_rx.await;
                let _ = trailers_tx.send(deserialize_consume_body_result(response.result));
            }
            Ok(CallReplayOutcome::Incomplete(call)) => {
                let _ = mode_tx.send(HttpBodyMode::Live);
                let outcome =
                    complete_http_consume_body::<Ctx, U>(accessor, call, stream_rx, &trailers_tx)
                        .await?;
                let _ = trailers_tx.send(outcome);
            }
            Err(error) => {
                let error = error.to_string();
                let _ = mode_tx.send(HttpBodyMode::Error(error.clone()));
                let _ = trailers_tx.send(Err(ErrorCode::InternalError(Some(error))));
            }
        }
        Ok(())
    }
}

async fn complete_http_consume_body<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    call: CallHandle<P3HttpClientConsumeBody, Cancellable>,
    stream_rx: oneshot::Receiver<CapturedHttpBody>,
    trailers_tx: &oneshot::Sender<HttpTrailersOutcome>,
) -> wasmtime::Result<HttpTrailersOutcome>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let captured = stream_rx.await.unwrap_or(CapturedHttpBody {
        contents: Vec::new(),
        result: Err(ErrorCode::InternalError(Some(
            "consume-body stream producer dropped without reporting a terminal".to_string(),
        ))),
        completed: false,
    });

    let response = HostResponseP3HttpClientConsumeBodyResult {
        contents: captured.contents,
        result: serialize_consume_body_result(&captured.result),
    };

    // Cancel (with the partial body/trailers as the recoverable terminal) when
    // the body did not complete naturally or the guest already dropped the
    // trailers future; complete otherwise.
    if !captured.completed || trailers_tx.is_closed() {
        let outcome = deserialize_consume_body_result(response.result.clone());
        call.cancel_access(accessor, durable_worker_ctx::<Ctx, U>, Some(response))
            .await
            .map_err(wasmtime::Error::from)?;
        return Ok(outcome);
    }

    let response = call
        .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
        .await
        .map_err(wasmtime::Error::from)?;
    Ok(deserialize_consume_body_result(response.result))
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
        let body = match upstream_stream.try_into::<HostBodyStreamProducer<U>>(store.as_context_mut())
        {
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

        let (mode_tx, mode_rx) = oneshot::channel();
        let (stream_tx, stream_rx) = oneshot::channel();
        let (trailers_tx, trailers_rx) = oneshot::channel();

        // Build both guest-facing handles before spawning the durable task. The
        // task appends the `consume-body` `Start`; the guest cannot poll either
        // handle until this host call returns, so spawning first would risk
        // committing a `Start` with no terminal (orphaned `Start`) if a later
        // handle construction fails.
        let mut stream = StreamReader::new(
            &mut store,
            DeferredHttpBodyProducer::new(body, mode_rx, stream_tx),
        )?;
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
            mode_tx, stream_rx, trailers_tx,
        ));
        Ok((stream, trailers))
    }

    fn drop<U>(mut store: Access<U, Self>, res: Resource<Response>) -> wasmtime::Result<()> {
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostResponseWithStore>::drop(store, res)
    }
}
