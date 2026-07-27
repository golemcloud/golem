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

use super::*;
use crate::durable_host::p3::{DurableP3, wasi_http_view};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::payload::types::{
    SerializableDnsErrorPayload, SerializableFieldSizePayload, SerializableHttpErrorCode,
    SerializableHttpMethod, SerializableP3HttpClientSend, SerializableP3HttpRequestOptions,
    SerializableP3HttpScheme, SerializableResponseHeaders, SerializableTlsAlertReceivedPayload,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use std::collections::HashMap;
use wasmtime::component::{Accessor, Resource};
use wasmtime_wasi_http::p3::WasiHttp;
use wasmtime_wasi_http::p3::bindings::http::types;
use wasmtime_wasi_http::p3::bindings::http::types::{
    ErrorCode, Fields, Method, Request, Response, Scheme,
};

pub(super) fn serialize_request<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    req: Resource<Request>,
) -> HttpResult<SerializableP3HttpClientSend> {
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
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
        let headers_resource = types::HostRequest::get_headers(&mut view, borrow_resource(&req))
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
                    first_byte_timeout_nanos: types::HostRequestOptions::get_first_byte_timeout(
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
    })
}

pub(super) fn serialize_response_headers<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: Resource<Response>,
) -> HttpResult<SerializableResponseHeaders> {
    let http_store = store.with_getter::<WasiHttp>(wasi_http_view::<Ctx, U>);
    http_store.with(|mut access| -> HttpResult<SerializableResponseHeaders> {
        let mut view = access.get();
        let status = types::HostResponse::get_status_code(&mut view, borrow_resource(&response))
            .map_err(HttpError::trap)?;
        let headers_resource =
            types::HostResponse::get_headers(&mut view, response).map_err(HttpError::trap)?;
        let headers = copy_fields(&mut view, headers_resource)?;
        Ok(SerializableResponseHeaders { status, headers })
    })
}

pub(super) fn copy_fields(
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

pub(super) fn serialize_method(method: Method) -> SerializableHttpMethod {
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

pub(super) fn serialize_scheme(scheme: Scheme) -> SerializableP3HttpScheme {
    match scheme {
        Scheme::Http => SerializableP3HttpScheme::Http,
        Scheme::Https => SerializableP3HttpScheme::Https,
        Scheme::Other(scheme) => SerializableP3HttpScheme::Other(scheme),
    }
}

pub(super) fn serialize_error_code(error: &ErrorCode) -> SerializableHttpErrorCode {
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

pub(super) fn deserialize_error_code(error: SerializableHttpErrorCode) -> ErrorCode {
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

pub(super) fn serialize_dns_error_payload(
    payload: &types::DnsErrorPayload,
) -> SerializableDnsErrorPayload {
    SerializableDnsErrorPayload {
        rcode: payload.rcode.clone(),
        info_code: payload.info_code,
    }
}

pub(super) fn deserialize_dns_error_payload(
    payload: SerializableDnsErrorPayload,
) -> types::DnsErrorPayload {
    types::DnsErrorPayload {
        rcode: payload.rcode,
        info_code: payload.info_code,
    }
}

pub(super) fn serialize_tls_alert_received_payload(
    payload: &types::TlsAlertReceivedPayload,
) -> SerializableTlsAlertReceivedPayload {
    SerializableTlsAlertReceivedPayload {
        alert_id: payload.alert_id,
        alert_message: payload.alert_message.clone(),
    }
}

pub(super) fn deserialize_tls_alert_received_payload(
    payload: SerializableTlsAlertReceivedPayload,
) -> types::TlsAlertReceivedPayload {
    types::TlsAlertReceivedPayload {
        alert_id: payload.alert_id,
        alert_message: payload.alert_message,
    }
}

pub(super) fn serialize_field_size_payload(
    payload: &types::FieldSizePayload,
) -> SerializableFieldSizePayload {
    SerializableFieldSizePayload {
        field_name: payload.field_name.clone(),
        field_size: payload.field_size,
    }
}

pub(super) fn deserialize_field_size_payload(
    payload: SerializableFieldSizePayload,
) -> types::FieldSizePayload {
    types::FieldSizePayload {
        field_name: payload.field_name,
        field_size: payload.field_size,
    }
}

pub(super) fn serialize_headers(headers: &HeaderMap) -> HashMap<String, Vec<Vec<u8>>> {
    let mut serialized: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    for (name, value) in headers.iter() {
        serialized
            .entry(name.as_str().to_string())
            .or_default()
            .push(value.as_bytes().to_vec());
    }
    serialized
}

pub(super) fn deserialize_headers(headers: HashMap<String, Vec<Vec<u8>>>) -> HeaderMap {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::p3::http::test_support::*;
    use golem_common::model::oplog::payload::types::*;
    use std::collections::HashMap;
    use test_r::test;

    #[test]
    fn error_code_conversion_roundtrips_through_p3() {
        for serializable in all_serializable_error_codes() {
            let roundtripped = serialize_error_code(&deserialize_error_code(serializable.clone()));
            assert_eq!(roundtripped, serializable);
        }
    }

    /// Response/trailer headers must replay with the same names, values, and
    /// per-name multiplicity. Header names are lower-cased by `http::HeaderName`,
    /// so the inputs here are already lower-case to make the roundtrip exact.
    #[test]
    fn headers_conversion_roundtrips() {
        let mut headers: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
        headers.insert("content-type".to_string(), vec![b"text/plain".to_vec()]);
        headers.insert(
            "set-cookie".to_string(),
            vec![b"a=1".to_vec(), b"b=2".to_vec()],
        );
        let roundtripped = serialize_headers(&deserialize_headers(headers.clone()));
        assert_eq!(roundtripped, headers);
    }

    /// The `consume-body` terminal (clean trailers, absent trailers, or a body
    /// `ErrorCode`) must replay unchanged.
    #[test]
    fn consume_body_result_conversion_roundtrips() {
        let cases = vec![
            SerializableP3HttpConsumeBodyResult::Trailers(None),
            SerializableP3HttpConsumeBodyResult::Trailers(Some(HashMap::from([(
                "x-trailer".to_string(),
                vec![b"value".to_vec()],
            )]))),
            SerializableP3HttpConsumeBodyResult::HttpError(
                SerializableHttpErrorCode::ConnectionRefused,
            ),
        ];
        for result in cases {
            let roundtripped =
                serialize_consume_body_result(&deserialize_consume_body_result(result.clone()));
            assert_eq!(roundtripped, result);
        }
    }
}
