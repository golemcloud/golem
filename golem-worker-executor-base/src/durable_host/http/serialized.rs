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

use bincode::{Decode, Encode};
use http::{HeaderName, HeaderValue, Version};

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use crate::durable_host::serialized::SerializableError;
use golem_wasm_rpc_derive::IntoValue;
use wasmtime_wasi_http::bindings::http::types::{
    DnsErrorPayload, ErrorCode, FieldSizePayload, Method, TlsAlertReceivedPayload,
};
use wasmtime_wasi_http::body::HostIncomingBody;
use wasmtime_wasi_http::types::{FieldMap, HostIncomingResponse};

#[derive(Debug, Clone, Encode, Decode)]
pub enum SerializedHttpVersion {
    Http09,
    /// `HTTP/1.0`
    Http10,
    /// `HTTP/1.1`
    Http11,
    /// `HTTP/2.0`
    Http2,
    /// `HTTP/3.0`
    Http3,
}

impl TryFrom<Version> for SerializedHttpVersion {
    type Error = String;

    fn try_from(value: Version) -> Result<Self, Self::Error> {
        if value == Version::HTTP_09 {
            Ok(SerializedHttpVersion::Http09)
        } else if value == Version::HTTP_10 {
            Ok(SerializedHttpVersion::Http10)
        } else if value == Version::HTTP_11 {
            Ok(SerializedHttpVersion::Http11)
        } else if value == Version::HTTP_2 {
            Ok(SerializedHttpVersion::Http2)
        } else if value == Version::HTTP_3 {
            Ok(SerializedHttpVersion::Http3)
        } else {
            Err(format!("Unknown HTTP version: {:?}", value))
        }
    }
}

impl From<SerializedHttpVersion> for Version {
    fn from(value: SerializedHttpVersion) -> Self {
        match value {
            SerializedHttpVersion::Http09 => Version::HTTP_09,
            SerializedHttpVersion::Http10 => Version::HTTP_10,
            SerializedHttpVersion::Http11 => Version::HTTP_11,
            SerializedHttpVersion::Http2 => Version::HTTP_2,
            SerializedHttpVersion::Http3 => Version::HTTP_3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub enum SerializableResponse {
    Pending,
    HeadersReceived(SerializableResponseHeaders),
    HttpError(SerializableErrorCode),
    InternalError(Option<SerializableError>),
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub struct SerializableResponseHeaders {
    pub status: u16,
    pub headers: HashMap<String, Vec<u8>>,
}

impl TryFrom<&HostIncomingResponse> for SerializableResponseHeaders {
    type Error = anyhow::Error;

    fn try_from(response: &HostIncomingResponse) -> Result<Self, Self::Error> {
        let mut headers = HashMap::new();
        for (key, value) in response.headers.iter() {
            headers.insert(key.as_str().to_string(), value.as_bytes().to_vec());
        }

        Ok(Self {
            status: response.status,
            headers,
        })
    }
}

impl TryFrom<SerializableResponseHeaders> for HostIncomingResponse {
    type Error = anyhow::Error;

    fn try_from(value: SerializableResponseHeaders) -> Result<Self, Self::Error> {
        let mut headers = FieldMap::new();
        for (key, value) in value.headers {
            headers.insert(HeaderName::from_str(&key)?, HeaderValue::try_from(value)?);
        }

        Ok(Self {
            status: value.status,
            headers,
            body: Some(HostIncomingBody::failing(
                "Body stream was interrupted due to a restart".to_string(),
            )), // NOTE: high enough timeout so it does not matter, but not as high to overflow instants
        })
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub struct SerializableTlsAlertReceivedPayload {
    pub alert_id: Option<u8>,
    pub alert_message: Option<String>,
}

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

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub struct SerializableDnsErrorPayload {
    pub rcode: Option<String>,
    pub info_code: Option<u16>,
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

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub struct SerializableFieldSizePayload {
    pub field_name: Option<String>,
    pub field_size: Option<u32>,
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

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub enum SerializableErrorCode {
    DnsTimeout,
    DnsError(SerializableDnsErrorPayload),
    DestinationNotFound,
    DestinationUnavailable,
    DestinationIpProhibited,
    DestinationIpUnroutable,
    ConnectionRefused,
    ConnectionTerminated,
    ConnectionTimeout,
    ConnectionReadTimeout,
    ConnectionWriteTimeout,
    ConnectionLimitReached,
    TlsProtocolError,
    TlsCertificateError,
    TlsAlertReceived(SerializableTlsAlertReceivedPayload),
    HttpRequestDenied,
    HttpRequestLengthRequired,
    HttpRequestBodySize(Option<u64>),
    HttpRequestMethodInvalid,
    HttpRequestUriInvalid,
    HttpRequestUriTooLong,
    HttpRequestHeaderSectionSize(Option<u32>),
    HttpRequestHeaderSize(Option<SerializableFieldSizePayload>),
    HttpRequestTrailerSectionSize(Option<u32>),
    HttpRequestTrailerSize(SerializableFieldSizePayload),
    HttpResponseIncomplete,
    HttpResponseHeaderSectionSize(Option<u32>),
    HttpResponseHeaderSize(SerializableFieldSizePayload),
    HttpResponseBodySize(Option<u64>),
    HttpResponseTrailerSectionSize(Option<u32>),
    HttpResponseTrailerSize(SerializableFieldSizePayload),
    HttpResponseTransferCoding(Option<String>),
    HttpResponseContentCoding(Option<String>),
    HttpResponseTimeout,
    HttpUpgradeFailed,
    HttpProtocolError,
    LoopDetected,
    ConfigurationError,
    InternalError(Option<String>),
}

impl From<ErrorCode> for SerializableErrorCode {
    fn from(value: ErrorCode) -> Self {
        (&value).into()
    }
}

impl From<&ErrorCode> for SerializableErrorCode {
    fn from(value: &ErrorCode) -> Self {
        match value {
            ErrorCode::DnsTimeout => SerializableErrorCode::DnsTimeout,
            ErrorCode::DnsError(payload) => SerializableErrorCode::DnsError(payload.into()),
            ErrorCode::DestinationNotFound => SerializableErrorCode::DestinationNotFound,
            ErrorCode::DestinationUnavailable => SerializableErrorCode::DestinationUnavailable,
            ErrorCode::DestinationIpProhibited => SerializableErrorCode::DestinationIpProhibited,
            ErrorCode::DestinationIpUnroutable => SerializableErrorCode::DestinationIpUnroutable,
            ErrorCode::ConnectionRefused => SerializableErrorCode::ConnectionRefused,
            ErrorCode::ConnectionTerminated => SerializableErrorCode::ConnectionTerminated,
            ErrorCode::ConnectionTimeout => SerializableErrorCode::ConnectionTimeout,
            ErrorCode::ConnectionReadTimeout => SerializableErrorCode::ConnectionReadTimeout,
            ErrorCode::ConnectionWriteTimeout => SerializableErrorCode::ConnectionWriteTimeout,
            ErrorCode::ConnectionLimitReached => SerializableErrorCode::ConnectionLimitReached,
            ErrorCode::TlsProtocolError => SerializableErrorCode::TlsProtocolError,
            ErrorCode::TlsCertificateError => SerializableErrorCode::TlsCertificateError,
            ErrorCode::TlsAlertReceived(payload) => {
                SerializableErrorCode::TlsAlertReceived(payload.into())
            }
            ErrorCode::HttpRequestDenied => SerializableErrorCode::HttpRequestDenied,
            ErrorCode::HttpRequestLengthRequired => {
                SerializableErrorCode::HttpRequestLengthRequired
            }
            ErrorCode::HttpRequestBodySize(payload) => {
                SerializableErrorCode::HttpRequestBodySize(*payload)
            }
            ErrorCode::HttpRequestMethodInvalid => SerializableErrorCode::HttpRequestMethodInvalid,
            ErrorCode::HttpRequestUriInvalid => SerializableErrorCode::HttpRequestUriInvalid,
            ErrorCode::HttpRequestUriTooLong => SerializableErrorCode::HttpRequestUriTooLong,
            ErrorCode::HttpRequestHeaderSectionSize(payload) => {
                SerializableErrorCode::HttpRequestHeaderSectionSize(*payload)
            }
            ErrorCode::HttpRequestHeaderSize(payload) => {
                SerializableErrorCode::HttpRequestHeaderSize(payload.as_ref().map(|p| p.into()))
            }
            ErrorCode::HttpRequestTrailerSectionSize(payload) => {
                SerializableErrorCode::HttpRequestTrailerSectionSize(*payload)
            }
            ErrorCode::HttpRequestTrailerSize(payload) => {
                SerializableErrorCode::HttpRequestTrailerSize(payload.into())
            }
            ErrorCode::HttpResponseIncomplete => SerializableErrorCode::HttpResponseIncomplete,
            ErrorCode::HttpResponseHeaderSectionSize(payload) => {
                SerializableErrorCode::HttpResponseHeaderSectionSize(*payload)
            }
            ErrorCode::HttpResponseHeaderSize(payload) => {
                SerializableErrorCode::HttpResponseHeaderSize(payload.into())
            }
            ErrorCode::HttpResponseBodySize(payload) => {
                SerializableErrorCode::HttpResponseBodySize(*payload)
            }
            ErrorCode::HttpResponseTrailerSectionSize(payload) => {
                SerializableErrorCode::HttpResponseTrailerSectionSize(*payload)
            }
            ErrorCode::HttpResponseTrailerSize(payload) => {
                SerializableErrorCode::HttpResponseTrailerSize(payload.into())
            }
            ErrorCode::HttpResponseTransferCoding(payload) => {
                SerializableErrorCode::HttpResponseTransferCoding(payload.clone())
            }
            ErrorCode::HttpResponseContentCoding(payload) => {
                SerializableErrorCode::HttpResponseContentCoding(payload.clone())
            }
            ErrorCode::HttpResponseTimeout => SerializableErrorCode::HttpResponseTimeout,
            ErrorCode::HttpUpgradeFailed => SerializableErrorCode::HttpUpgradeFailed,
            ErrorCode::HttpProtocolError => SerializableErrorCode::HttpProtocolError,
            ErrorCode::LoopDetected => SerializableErrorCode::LoopDetected,
            ErrorCode::ConfigurationError => SerializableErrorCode::ConfigurationError,
            ErrorCode::InternalError(payload) => {
                SerializableErrorCode::InternalError(payload.clone())
            }
        }
    }
}

impl From<SerializableErrorCode> for ErrorCode {
    fn from(value: SerializableErrorCode) -> Self {
        match value {
            SerializableErrorCode::DnsTimeout => ErrorCode::DnsTimeout,
            SerializableErrorCode::DnsError(payload) => ErrorCode::DnsError(payload.into()),
            SerializableErrorCode::DestinationNotFound => ErrorCode::DestinationNotFound,
            SerializableErrorCode::DestinationUnavailable => ErrorCode::DestinationUnavailable,
            SerializableErrorCode::DestinationIpProhibited => ErrorCode::DestinationIpProhibited,
            SerializableErrorCode::DestinationIpUnroutable => ErrorCode::DestinationIpUnroutable,
            SerializableErrorCode::ConnectionRefused => ErrorCode::ConnectionRefused,
            SerializableErrorCode::ConnectionTerminated => ErrorCode::ConnectionTerminated,
            SerializableErrorCode::ConnectionTimeout => ErrorCode::ConnectionTimeout,
            SerializableErrorCode::ConnectionReadTimeout => ErrorCode::ConnectionReadTimeout,
            SerializableErrorCode::ConnectionWriteTimeout => ErrorCode::ConnectionWriteTimeout,
            SerializableErrorCode::ConnectionLimitReached => ErrorCode::ConnectionLimitReached,
            SerializableErrorCode::TlsProtocolError => ErrorCode::TlsProtocolError,
            SerializableErrorCode::TlsCertificateError => ErrorCode::TlsCertificateError,
            SerializableErrorCode::TlsAlertReceived(payload) => {
                ErrorCode::TlsAlertReceived(payload.into())
            }
            SerializableErrorCode::HttpRequestDenied => ErrorCode::HttpRequestDenied,
            SerializableErrorCode::HttpRequestLengthRequired => {
                ErrorCode::HttpRequestLengthRequired
            }
            SerializableErrorCode::HttpRequestBodySize(payload) => {
                ErrorCode::HttpRequestBodySize(payload)
            }
            SerializableErrorCode::HttpRequestMethodInvalid => ErrorCode::HttpRequestMethodInvalid,
            SerializableErrorCode::HttpRequestUriInvalid => ErrorCode::HttpRequestUriInvalid,
            SerializableErrorCode::HttpRequestUriTooLong => ErrorCode::HttpRequestUriTooLong,
            SerializableErrorCode::HttpRequestHeaderSectionSize(payload) => {
                ErrorCode::HttpRequestHeaderSectionSize(payload)
            }
            SerializableErrorCode::HttpRequestHeaderSize(payload) => {
                ErrorCode::HttpRequestHeaderSize(payload.map(|p| p.into()))
            }
            SerializableErrorCode::HttpRequestTrailerSectionSize(payload) => {
                ErrorCode::HttpRequestTrailerSectionSize(payload)
            }
            SerializableErrorCode::HttpRequestTrailerSize(payload) => {
                ErrorCode::HttpRequestTrailerSize(payload.into())
            }
            SerializableErrorCode::HttpResponseIncomplete => ErrorCode::HttpResponseIncomplete,
            SerializableErrorCode::HttpResponseHeaderSectionSize(payload) => {
                ErrorCode::HttpResponseHeaderSectionSize(payload)
            }
            SerializableErrorCode::HttpResponseHeaderSize(payload) => {
                ErrorCode::HttpResponseHeaderSize(payload.into())
            }
            SerializableErrorCode::HttpResponseBodySize(payload) => {
                ErrorCode::HttpResponseBodySize(payload)
            }
            SerializableErrorCode::HttpResponseTrailerSectionSize(payload) => {
                ErrorCode::HttpResponseTrailerSectionSize(payload)
            }
            SerializableErrorCode::HttpResponseTrailerSize(payload) => {
                ErrorCode::HttpResponseTrailerSize(payload.into())
            }
            SerializableErrorCode::HttpResponseTransferCoding(payload) => {
                ErrorCode::HttpResponseTransferCoding(payload)
            }
            SerializableErrorCode::HttpResponseContentCoding(payload) => {
                ErrorCode::HttpResponseContentCoding(payload)
            }
            SerializableErrorCode::HttpResponseTimeout => ErrorCode::HttpResponseTimeout,
            SerializableErrorCode::HttpUpgradeFailed => ErrorCode::HttpUpgradeFailed,
            SerializableErrorCode::HttpProtocolError => ErrorCode::HttpProtocolError,
            SerializableErrorCode::LoopDetected => ErrorCode::LoopDetected,
            SerializableErrorCode::ConfigurationError => ErrorCode::ConfigurationError,
            SerializableErrorCode::InternalError(payload) => ErrorCode::InternalError(payload),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub enum SerializableHttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Connect,
    Options,
    Trace,
    Patch,
    Other(String),
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
            SerializableHttpMethod::Other(method) => write!(f, "{}", method),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, IntoValue)]
pub struct SerializableHttpRequest {
    pub uri: String,
    pub method: SerializableHttpMethod,
    pub headers: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::durable_host::http::serialized::{SerializableErrorCode, SerializedHttpVersion};
    use http::Version;
    use proptest::option::of;
    use proptest::prelude::*;
    use proptest::strategy::LazyJust;
    use wasmtime_wasi_http::bindings::http::types::{
        DnsErrorPayload, ErrorCode, FieldSizePayload, TlsAlertReceivedPayload,
    };

    fn version_strat() -> impl Strategy<Value = Version> {
        prop_oneof![
            Just(Version::HTTP_09),
            Just(Version::HTTP_10),
            Just(Version::HTTP_11),
            Just(Version::HTTP_2),
            Just(Version::HTTP_3),
        ]
    }

    fn field_size_payload_strat() -> impl Strategy<Value = FieldSizePayload> {
        (of(".*"), of(any::<u32>())).prop_map(|(field_name, field_size)| FieldSizePayload {
            field_name,
            field_size,
        })
    }

    fn error_code_strat() -> impl Strategy<Value = ErrorCode> {
        prop_oneof! {
            LazyJust::new(|| ErrorCode::DnsTimeout),
            (of(".*"), of(any::<u16>())).prop_map(|(rcode, info_code)| ErrorCode::DnsError(DnsErrorPayload { rcode, info_code } )),
            LazyJust::new(|| ErrorCode::DestinationNotFound),
            LazyJust::new(|| ErrorCode::DestinationUnavailable),
            LazyJust::new(|| ErrorCode::DestinationIpProhibited),
            LazyJust::new(|| ErrorCode::DestinationIpUnroutable),
            LazyJust::new(|| ErrorCode::ConnectionRefused),
            LazyJust::new(|| ErrorCode::ConnectionTerminated),
            LazyJust::new(|| ErrorCode::ConnectionTimeout),
            LazyJust::new(|| ErrorCode::ConnectionReadTimeout),
            LazyJust::new(|| ErrorCode::ConnectionWriteTimeout),
            LazyJust::new(|| ErrorCode::ConnectionLimitReached),
            LazyJust::new(|| ErrorCode::TlsProtocolError),
            LazyJust::new(|| ErrorCode::TlsCertificateError),
            (of(any::<u8>()), of(".*")).prop_map(|(alert_id, alert_message)| ErrorCode::TlsAlertReceived(TlsAlertReceivedPayload { alert_id, alert_message })),
            LazyJust::new(|| ErrorCode::HttpRequestDenied),
            LazyJust::new(|| ErrorCode::HttpRequestLengthRequired),
            of(any::<u64>()).prop_map(ErrorCode::HttpRequestBodySize),
            LazyJust::new(|| ErrorCode::HttpRequestMethodInvalid),
            LazyJust::new(|| ErrorCode::HttpRequestUriInvalid),
            LazyJust::new(|| ErrorCode::HttpRequestUriTooLong),
            of(any::<u32>()).prop_map(ErrorCode::HttpRequestHeaderSectionSize),
            of(field_size_payload_strat()).prop_map(ErrorCode::HttpRequestHeaderSize),
            of(any::<u32>()).prop_map(ErrorCode::HttpRequestTrailerSectionSize),
            field_size_payload_strat().prop_map(ErrorCode::HttpRequestTrailerSize),
            LazyJust::new(|| ErrorCode::HttpResponseIncomplete),
            of(any::<u32>()).prop_map(ErrorCode::HttpResponseHeaderSectionSize),
            field_size_payload_strat().prop_map(ErrorCode::HttpResponseHeaderSize),
            of(any::<u64>()).prop_map(ErrorCode::HttpResponseBodySize),
            of(any::<u32>()).prop_map(ErrorCode::HttpResponseTrailerSectionSize),
            field_size_payload_strat().prop_map(ErrorCode::HttpResponseTrailerSize),
            of(".*").prop_map(ErrorCode::HttpResponseTransferCoding),
            of(".*").prop_map(ErrorCode::HttpResponseContentCoding),
            LazyJust::new(|| ErrorCode::HttpResponseTimeout),
            LazyJust::new(|| ErrorCode::HttpUpgradeFailed),
            LazyJust::new(|| ErrorCode::HttpProtocolError),
            LazyJust::new(|| ErrorCode::LoopDetected),
            LazyJust::new(|| ErrorCode::ConfigurationError),
            of(".*").prop_map(ErrorCode::InternalError),
        }
    }

    proptest! {
        #[test]
        fn test_http_version_roundtrip(version in version_strat()) {
            let serialized: SerializedHttpVersion = version.try_into().unwrap();
            let deserialized: Version = serialized.into();
            prop_assert_eq!(version, deserialized);
        }

        #[test]
        fn test_error_code_roundtrip(error_code in error_code_strat()) {
            let serialized: SerializableErrorCode = (&error_code).into();
            let deserialized: ErrorCode = serialized.into();
            match (error_code, deserialized) {
                (ErrorCode::DnsTimeout, ErrorCode::DnsTimeout) => {}
                (ErrorCode::DnsError(a) , ErrorCode::DnsError(b) ) => {
                    prop_assert_eq!(a.rcode, b.rcode);
                    prop_assert_eq!(a.info_code, b.info_code);
                }
                (ErrorCode::DestinationNotFound, ErrorCode::DestinationNotFound) => {}
                (ErrorCode::DestinationUnavailable, ErrorCode::DestinationUnavailable) => {}
                (ErrorCode::DestinationIpProhibited, ErrorCode::DestinationIpProhibited) => {}
                (ErrorCode::DestinationIpUnroutable, ErrorCode::DestinationIpUnroutable) => {}
                (ErrorCode::ConnectionRefused, ErrorCode::ConnectionRefused) => {}
                (ErrorCode::ConnectionTerminated, ErrorCode::ConnectionTerminated) => {}
                (ErrorCode::ConnectionTimeout, ErrorCode::ConnectionTimeout) => {}
                (ErrorCode::ConnectionReadTimeout, ErrorCode::ConnectionReadTimeout) => {}
                (ErrorCode::ConnectionWriteTimeout, ErrorCode::ConnectionWriteTimeout) => {}
                (ErrorCode::ConnectionLimitReached, ErrorCode::ConnectionLimitReached) => {}
                (ErrorCode::TlsProtocolError, ErrorCode::TlsProtocolError) => {}
                (ErrorCode::TlsCertificateError, ErrorCode::TlsCertificateError) => {}
                (ErrorCode::TlsAlertReceived(a), ErrorCode::TlsAlertReceived(b)) => {
                    prop_assert_eq!(a.alert_id, b.alert_id);
                    prop_assert_eq!(a.alert_message, b.alert_message);
                }
                (ErrorCode::HttpRequestDenied, ErrorCode::HttpRequestDenied) => {}
                (ErrorCode::HttpRequestLengthRequired, ErrorCode::HttpRequestLengthRequired) => {}
                (ErrorCode::HttpRequestBodySize(a), ErrorCode::HttpRequestBodySize(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpRequestMethodInvalid, ErrorCode::HttpRequestMethodInvalid) => {}
                (ErrorCode::HttpRequestUriInvalid, ErrorCode::HttpRequestUriInvalid) => {}
                (ErrorCode::HttpRequestUriTooLong, ErrorCode::HttpRequestUriTooLong) => {}
                (ErrorCode::HttpRequestHeaderSectionSize(a), ErrorCode::HttpRequestHeaderSectionSize(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpRequestHeaderSize(a), ErrorCode::HttpRequestHeaderSize(b)) => {
                    match (a, b) {
                        (Some(a), Some(b)) => {
                            prop_assert_eq!(a.field_name, b.field_name);
                            prop_assert_eq!(a.field_size, b.field_size);
                        }
                        (None, None) => {}
                        _ => prop_assert!(false)
                    }
                }
                (ErrorCode::HttpRequestTrailerSectionSize(a), ErrorCode::HttpRequestTrailerSectionSize(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpRequestTrailerSize(a), ErrorCode::HttpRequestTrailerSize(b)) => {
                    prop_assert_eq!(a.field_name, b.field_name);
                    prop_assert_eq!(a.field_size, b.field_size);
                }
                (ErrorCode::HttpResponseIncomplete, ErrorCode::HttpResponseIncomplete) => {}
                (ErrorCode::HttpResponseHeaderSectionSize(a), ErrorCode::HttpResponseHeaderSectionSize(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpResponseHeaderSize(a), ErrorCode::HttpResponseHeaderSize(b)) => {
                    prop_assert_eq!(a.field_name, b.field_name);
                    prop_assert_eq!(a.field_size, b.field_size);
                }
                (ErrorCode::HttpResponseBodySize(a), ErrorCode::HttpResponseBodySize(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpResponseTrailerSectionSize(a), ErrorCode::HttpResponseTrailerSectionSize(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpResponseTrailerSize(a), ErrorCode::HttpResponseTrailerSize(b)) => {
                    prop_assert_eq!(a.field_name, b.field_name);
                    prop_assert_eq!(a.field_size, b.field_size);
                }
                (ErrorCode::HttpResponseTransferCoding(a), ErrorCode::HttpResponseTransferCoding(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpResponseContentCoding(a), ErrorCode::HttpResponseContentCoding(b)) => {
                    prop_assert_eq!(a, b);
                }
                (ErrorCode::HttpResponseTimeout, ErrorCode::HttpResponseTimeout) => {}
                (ErrorCode::HttpUpgradeFailed, ErrorCode::HttpUpgradeFailed) => {}
                (ErrorCode::HttpProtocolError, ErrorCode::HttpProtocolError) => {}
                (ErrorCode::LoopDetected, ErrorCode::LoopDetected) => {}
                (ErrorCode::ConfigurationError, ErrorCode::ConfigurationError) => {}
                (ErrorCode::InternalError(a), ErrorCode::InternalError(b)) => {
                    prop_assert_eq!(a, b);
                }
                _ => prop_assert!(false)
            }
        }
    }
}
