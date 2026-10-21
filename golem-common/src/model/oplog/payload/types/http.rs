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

use std::fmt::{Display, Formatter};

use wasmtime_wasi_http::p2::bindings::http::types::{
    DnsErrorPayload, FieldSizePayload, Method, TlsAlertReceivedPayload,
};

use super::{
    SerializableDnsErrorPayload, SerializableFieldSizePayload, SerializableHttpErrorCode,
    SerializableHttpMethod, SerializableTlsAlertReceivedPayload,
};

impl From<&TlsAlertReceivedPayload> for SerializableTlsAlertReceivedPayload {
    fn from(value: &TlsAlertReceivedPayload) -> Self {
        Self {
            alert_id: value.alert_id,
            alert_message: value.alert_message.clone(),
        }
    }
}

impl From<SerializableTlsAlertReceivedPayload> for TlsAlertReceivedPayload {
    fn from(value: SerializableTlsAlertReceivedPayload) -> Self {
        Self {
            alert_id: value.alert_id,
            alert_message: value.alert_message,
        }
    }
}

impl From<&DnsErrorPayload> for SerializableDnsErrorPayload {
    fn from(value: &DnsErrorPayload) -> Self {
        Self {
            rcode: value.rcode.clone(),
            info_code: value.info_code,
        }
    }
}

impl From<SerializableDnsErrorPayload> for DnsErrorPayload {
    fn from(value: SerializableDnsErrorPayload) -> Self {
        Self {
            rcode: value.rcode,
            info_code: value.info_code,
        }
    }
}

impl From<&FieldSizePayload> for SerializableFieldSizePayload {
    fn from(value: &FieldSizePayload) -> Self {
        Self {
            field_name: value.field_name.clone(),
            field_size: value.field_size,
        }
    }
}

impl From<SerializableFieldSizePayload> for FieldSizePayload {
    fn from(value: SerializableFieldSizePayload) -> Self {
        Self {
            field_name: value.field_name,
            field_size: value.field_size,
        }
    }
}

impl From<wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> for SerializableHttpErrorCode {
    fn from(value: wasmtime_wasi_http::p2::bindings::http::types::ErrorCode) -> Self {
        (&value).into()
    }
}

impl From<&wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> for SerializableHttpErrorCode {
    fn from(value: &wasmtime_wasi_http::p2::bindings::http::types::ErrorCode) -> Self {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

        match value {
            ErrorCode::DnsTimeout => SerializableHttpErrorCode::DnsTimeout,
            ErrorCode::DnsError(payload) => SerializableHttpErrorCode::DnsError(payload.into()),
            ErrorCode::DestinationNotFound => SerializableHttpErrorCode::DestinationNotFound,
            ErrorCode::DestinationUnavailable => SerializableHttpErrorCode::DestinationUnavailable,
            ErrorCode::DestinationIpProhibited => {
                SerializableHttpErrorCode::DestinationIpProhibited
            }
            ErrorCode::DestinationIpUnroutable => {
                SerializableHttpErrorCode::DestinationIpUnroutable
            }
            ErrorCode::ConnectionRefused => SerializableHttpErrorCode::ConnectionRefused,
            ErrorCode::ConnectionTerminated => SerializableHttpErrorCode::ConnectionTerminated,
            ErrorCode::ConnectionTimeout => SerializableHttpErrorCode::ConnectionTimeout,
            ErrorCode::ConnectionReadTimeout => SerializableHttpErrorCode::ConnectionReadTimeout,
            ErrorCode::ConnectionWriteTimeout => SerializableHttpErrorCode::ConnectionWriteTimeout,
            ErrorCode::ConnectionLimitReached => SerializableHttpErrorCode::ConnectionLimitReached,
            ErrorCode::TlsProtocolError => SerializableHttpErrorCode::TlsProtocolError,
            ErrorCode::TlsCertificateError => SerializableHttpErrorCode::TlsCertificateError,
            ErrorCode::TlsAlertReceived(payload) => {
                SerializableHttpErrorCode::TlsAlertReceived(payload.into())
            }
            ErrorCode::HttpRequestDenied => SerializableHttpErrorCode::HttpRequestDenied,
            ErrorCode::HttpRequestLengthRequired => {
                SerializableHttpErrorCode::HttpRequestLengthRequired
            }
            ErrorCode::HttpRequestBodySize(payload) => {
                SerializableHttpErrorCode::HttpRequestBodySize(*payload)
            }
            ErrorCode::HttpRequestMethodInvalid => {
                SerializableHttpErrorCode::HttpRequestMethodInvalid
            }
            ErrorCode::HttpRequestUriInvalid => SerializableHttpErrorCode::HttpRequestUriInvalid,
            ErrorCode::HttpRequestUriTooLong => SerializableHttpErrorCode::HttpRequestUriTooLong,
            ErrorCode::HttpRequestHeaderSectionSize(payload) => {
                SerializableHttpErrorCode::HttpRequestHeaderSectionSize(*payload)
            }
            ErrorCode::HttpRequestHeaderSize(payload) => {
                SerializableHttpErrorCode::HttpRequestHeaderSize(payload.as_ref().map(|p| p.into()))
            }
            ErrorCode::HttpRequestTrailerSectionSize(payload) => {
                SerializableHttpErrorCode::HttpRequestTrailerSectionSize(*payload)
            }
            ErrorCode::HttpRequestTrailerSize(payload) => {
                SerializableHttpErrorCode::HttpRequestTrailerSize(payload.into())
            }
            ErrorCode::HttpResponseIncomplete => SerializableHttpErrorCode::HttpResponseIncomplete,
            ErrorCode::HttpResponseHeaderSectionSize(payload) => {
                SerializableHttpErrorCode::HttpResponseHeaderSectionSize(*payload)
            }
            ErrorCode::HttpResponseHeaderSize(payload) => {
                SerializableHttpErrorCode::HttpResponseHeaderSize(payload.into())
            }
            ErrorCode::HttpResponseBodySize(payload) => {
                SerializableHttpErrorCode::HttpResponseBodySize(*payload)
            }
            ErrorCode::HttpResponseTrailerSectionSize(payload) => {
                SerializableHttpErrorCode::HttpResponseTrailerSectionSize(*payload)
            }
            ErrorCode::HttpResponseTrailerSize(payload) => {
                SerializableHttpErrorCode::HttpResponseTrailerSize(payload.into())
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
}

impl From<SerializableHttpErrorCode> for wasmtime_wasi_http::p2::bindings::http::types::ErrorCode {
    fn from(value: SerializableHttpErrorCode) -> Self {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

        match value {
            SerializableHttpErrorCode::DnsTimeout => ErrorCode::DnsTimeout,
            SerializableHttpErrorCode::DnsError(payload) => ErrorCode::DnsError(payload.into()),
            SerializableHttpErrorCode::DestinationNotFound => ErrorCode::DestinationNotFound,
            SerializableHttpErrorCode::DestinationUnavailable => ErrorCode::DestinationUnavailable,
            SerializableHttpErrorCode::DestinationIpProhibited => {
                ErrorCode::DestinationIpProhibited
            }
            SerializableHttpErrorCode::DestinationIpUnroutable => {
                ErrorCode::DestinationIpUnroutable
            }
            SerializableHttpErrorCode::ConnectionRefused => ErrorCode::ConnectionRefused,
            SerializableHttpErrorCode::ConnectionTerminated => ErrorCode::ConnectionTerminated,
            SerializableHttpErrorCode::ConnectionTimeout => ErrorCode::ConnectionTimeout,
            SerializableHttpErrorCode::ConnectionReadTimeout => ErrorCode::ConnectionReadTimeout,
            SerializableHttpErrorCode::ConnectionWriteTimeout => ErrorCode::ConnectionWriteTimeout,
            SerializableHttpErrorCode::ConnectionLimitReached => ErrorCode::ConnectionLimitReached,
            SerializableHttpErrorCode::TlsProtocolError => ErrorCode::TlsProtocolError,
            SerializableHttpErrorCode::TlsCertificateError => ErrorCode::TlsCertificateError,
            SerializableHttpErrorCode::TlsAlertReceived(payload) => {
                ErrorCode::TlsAlertReceived(payload.into())
            }
            SerializableHttpErrorCode::HttpRequestDenied => ErrorCode::HttpRequestDenied,
            SerializableHttpErrorCode::HttpRequestLengthRequired => {
                ErrorCode::HttpRequestLengthRequired
            }
            SerializableHttpErrorCode::HttpRequestBodySize(payload) => {
                ErrorCode::HttpRequestBodySize(payload)
            }
            SerializableHttpErrorCode::HttpRequestMethodInvalid => {
                ErrorCode::HttpRequestMethodInvalid
            }
            SerializableHttpErrorCode::HttpRequestUriInvalid => ErrorCode::HttpRequestUriInvalid,
            SerializableHttpErrorCode::HttpRequestUriTooLong => ErrorCode::HttpRequestUriTooLong,
            SerializableHttpErrorCode::HttpRequestHeaderSectionSize(payload) => {
                ErrorCode::HttpRequestHeaderSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpRequestHeaderSize(payload) => {
                ErrorCode::HttpRequestHeaderSize(payload.map(|p| p.into()))
            }
            SerializableHttpErrorCode::HttpRequestTrailerSectionSize(payload) => {
                ErrorCode::HttpRequestTrailerSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpRequestTrailerSize(payload) => {
                ErrorCode::HttpRequestTrailerSize(payload.into())
            }
            SerializableHttpErrorCode::HttpResponseIncomplete => ErrorCode::HttpResponseIncomplete,
            SerializableHttpErrorCode::HttpResponseHeaderSectionSize(payload) => {
                ErrorCode::HttpResponseHeaderSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpResponseHeaderSize(payload) => {
                ErrorCode::HttpResponseHeaderSize(payload.into())
            }
            SerializableHttpErrorCode::HttpResponseBodySize(payload) => {
                ErrorCode::HttpResponseBodySize(payload)
            }
            SerializableHttpErrorCode::HttpResponseTrailerSectionSize(payload) => {
                ErrorCode::HttpResponseTrailerSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpResponseTrailerSize(payload) => {
                ErrorCode::HttpResponseTrailerSize(payload.into())
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
}

impl From<Method> for SerializableHttpMethod {
    fn from(value: Method) -> Self {
        match value {
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
}

impl TryFrom<&SerializableHttpMethod> for ::http::Method {
    type Error = anyhow::Error;

    fn try_from(value: &SerializableHttpMethod) -> Result<Self, Self::Error> {
        match value {
            SerializableHttpMethod::Get => Ok(::http::Method::GET),
            SerializableHttpMethod::Post => Ok(::http::Method::POST),
            SerializableHttpMethod::Put => Ok(::http::Method::PUT),
            SerializableHttpMethod::Delete => Ok(::http::Method::DELETE),
            SerializableHttpMethod::Head => Ok(::http::Method::HEAD),
            SerializableHttpMethod::Connect => Ok(::http::Method::CONNECT),
            SerializableHttpMethod::Options => Ok(::http::Method::OPTIONS),
            SerializableHttpMethod::Trace => Ok(::http::Method::TRACE),
            SerializableHttpMethod::Patch => Ok(::http::Method::PATCH),
            SerializableHttpMethod::Other(m) => ::http::Method::from_bytes(m.as_bytes())
                .map_err(|e| anyhow::anyhow!("invalid HTTP method '{m}': {e}")),
        }
    }
}

impl Display for SerializableHttpMethod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializableHttpMethod::Get => write!(f, "GET"),
            SerializableHttpMethod::Post => write!(f, "POST"),
            SerializableHttpMethod::Put => write!(f, "PUT"),
            SerializableHttpMethod::Delete => write!(f, "DELETE"),
            SerializableHttpMethod::Head => write!(f, "HEAD"),
            SerializableHttpMethod::Connect => write!(f, "CONNECT"),
            SerializableHttpMethod::Options => write!(f, "OPTIONS"),
            SerializableHttpMethod::Trace => write!(f, "TRACE"),
            SerializableHttpMethod::Patch => write!(f, "PATCH"),
            SerializableHttpMethod::Other(method) => write!(f, "{method}"),
        }
    }
}
