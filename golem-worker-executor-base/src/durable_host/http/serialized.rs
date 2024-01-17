use crate::durable_host::SerializableError;

use bincode::{Decode, Encode};
use http::{HeaderName, HeaderValue, Version};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use wasmtime_wasi_http::bindings::http::types::{
    DnsErrorPayload, ErrorCode, FieldSizePayload, TlsAlertReceivedPayload,
};
use wasmtime_wasi_http::body::HostIncomingBody;
use wasmtime_wasi_http::types::{FieldMap, HostIncomingResponse};

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum SerializableResponse {
    Pending,
    HeadersReceived(SerializableResponseHeaders),
    HttpError(SerializableErrorCode),
    InternalError(Option<SerializableError>),
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct SerializableResponseHeaders {
    status: u16,
    headers: HashMap<String, Vec<u8>>,
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

        let fake_worker = tokio::spawn(async {}).into();

        Ok(Self {
            status: value.status,
            headers,
            body: Some(HostIncomingBody::failing(
                "Body stream was interrupted due to a restart".to_string(),
            )), // NOTE: high enough timeout so it does not matter, but not as high to overflow instants
            worker: Arc::new(fake_worker),
        })
    }
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
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
