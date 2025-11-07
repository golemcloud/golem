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

use desert_rust::BinaryCodec;
use std::fmt::Display;
use std::str::FromStr;

#[cfg(test)]
mod tests {
    use test_r::test;

    use http::Version;
    use proptest::option::of;
    use proptest::prelude::*;
    use proptest::strategy::LazyJust;
    use wasmtime_wasi_http::bindings::http::types::{
        DnsErrorPayload, ErrorCode, FieldSizePayload, TlsAlertReceivedPayload,
    };
    use golem_common::model::oplog::types::{SerializableHttpErrorCode, SerializedHttpVersion};

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
            let serialized: SerializableHttpErrorCode = (&error_code).into();
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
