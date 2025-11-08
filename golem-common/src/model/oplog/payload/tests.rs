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

use chrono::Timelike;
use desert_rust::BinaryCodec;
use golem_wasm::{FromValue, IntoValue};
use std::fmt::Display;
use std::str::FromStr;

use test_r::test;

use crate::model::oplog::types::{SerializableDateTime, SerializableHttpErrorCode, SerializableIpAddress, SerializableIpAddresses, SerializableStreamError, SerializedHttpVersion};
use crate::model::oplog::{OplogIndex, WorkerError};
use crate::model::{ComponentId, PromiseId, ShardId, WorkerId};
use crate::model::oplog::raw_types::SpanData;
use crate::model::invocation_context::{SpanId, AttributeValue};
use crate::model::Timestamp;
use iso8601_timestamp as iso_ts;
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::strategy::LazyJust;
use std::ops::Add;
use std::time::{Duration, SystemTime};
use http::Version;
use proptest::option::of;
use uuid::Uuid;
use wasmtime_wasi::p2::bindings::sockets::network::IpAddress;
use wasmtime_wasi::p2::bindings::{filesystem, sockets};
use wasmtime_wasi::p2::{FsError, SocketError};
use wasmtime_wasi::StreamError;
use wasmtime_wasi_http::bindings::http::types::{DnsErrorPayload, ErrorCode, FieldSizePayload, TlsAlertReceivedPayload};
use std::num::NonZeroU64;

fn datetime_strat(
) -> impl Strategy<Value = wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime> {
    (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
        wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime {
            seconds,
            nanoseconds,
        }
    })
}

fn systemtime_strat() -> impl Strategy<Value = SystemTime> {
    (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
        SystemTime::UNIX_EPOCH.add(Duration::new(seconds, nanoseconds))
    })
}

fn anyhow_strat() -> impl Strategy<Value = anyhow::Error> {
    ".*".prop_map(anyhow::Error::msg)
}

fn fserror_strat() -> impl Strategy<Value = FsError> {
    prop_oneof! {
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Access)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::WouldBlock)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Already)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::BadDescriptor)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Busy)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Deadlock)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Quota)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Exist)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::FileTooLarge)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::IllegalByteSequence)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InProgress)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Interrupted)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Invalid)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Io)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::IsDirectory)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Loop)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::TooManyLinks)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::MessageSize)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NameTooLong)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoDevice)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoEntry)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoLock)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InsufficientMemory)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InsufficientSpace)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotDirectory)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotEmpty)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotRecoverable)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Unsupported)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoTty)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoSuchDevice)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Overflow)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotPermitted)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Pipe)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::ReadOnly)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InvalidSeek)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::TextFileBusy)),
        LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::CrossDevice)),
        anyhow_strat().prop_map(FsError::trap),
    }
}

fn uuid_strat() -> impl Strategy<Value = Uuid> {
    (any::<u64>(), any::<u64>()).prop_map(|(a, b)| Uuid::from_u64_pair(a, b))
}

fn componentid_strat() -> impl Strategy<Value = ComponentId> {
    uuid_strat().prop_map(ComponentId)
}

fn workerid_strat() -> impl Strategy<Value = WorkerId> {
    (componentid_strat(), ".+").prop_map(|(component_id, worker_name)| WorkerId {
        component_id,
        worker_name,
    })
}

fn promiseid_strat() -> impl Strategy<Value = PromiseId> {
    (workerid_strat(), any::<u64>()).prop_map(|(worker_id, oplog_idx)| PromiseId {
        worker_id,
        oplog_idx: OplogIndex::from_u64(oplog_idx),
    })
}

fn shardid_strat() -> impl Strategy<Value = ShardId> {
    any::<i64>().prop_map(ShardId::new)
}

fn workererror_strat() -> impl Strategy<Value = WorkerError> {
    prop_oneof! {
        Just(WorkerError::OutOfMemory),
        Just(WorkerError::StackOverflow),
        ".*".prop_map(WorkerError::InvalidRequest),
        ".*".prop_map(WorkerError::Unknown),
    }
}

fn socketerror_strat() -> impl Strategy<Value = SocketError> {
    prop_oneof! {
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::Unknown)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AccessDenied)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NotSupported)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::InvalidArgument)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::OutOfMemory)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::Timeout)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConcurrencyConflict)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NotInProgress)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::WouldBlock)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::InvalidState)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NewSocketLimit)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AddressNotBindable)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AddressInUse)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::RemoteUnreachable)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionRefused)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionReset)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionAborted)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::DatagramTooLarge)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NameUnresolvable)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::TemporaryResolverFailure)),
        LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::PermanentResolverFailure)),
        anyhow_strat().prop_map(SocketError::trap),
    }
}

fn streamerror_strat() -> impl Strategy<Value = StreamError> {
    prop_oneof! {
        LazyJust::new(|| StreamError::Closed),
        anyhow_strat().prop_map(StreamError::LastOperationFailed),
        anyhow_strat().prop_map(StreamError::Trap)
    }
}

fn ipaddress_strat() -> impl Strategy<Value = IpAddress> {
    prop_oneof! {
        (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c, d)| IpAddress::Ipv4((a, b, c, d))),
        (any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>()).prop_map(|(a, b, c, d, e, f, g, h)| IpAddress::Ipv6((a, b, c, d, e, f, g, h))),
    }
}

fn span_data_strat() -> impl Strategy<Value = SpanData> {
    prop_oneof![
        (
            any::<u64>().prop_map(|x| SpanId(NonZeroU64::new(x + 1).unwrap())),
            any::<i64>(),
            of(any::<u64>().prop_map(|x| SpanId(NonZeroU64::new(x + 1).unwrap()))),
            vec((any::<String>(), any::<String>()), 0..5).prop_map(|v| {
                v.into_iter().map(|(k, v)| (k, AttributeValue::String(v))).collect()
            }),
            any::<bool>()
        ).prop_map(|(span_id, _start, parent_id, attributes, inherited)| SpanData::LocalSpan {
            span_id,
            start: Timestamp(iso_ts::Timestamp::parse("2023-01-01T00:00:00Z").unwrap()),
            parent_id,
            linked_context: None,
            attributes,
            inherited,
        }),
        any::<u64>().prop_map(|x| SpanId(NonZeroU64::new(x + 1).unwrap())).prop_map(|span_id| SpanData::ExternalSpan { span_id }),
    ]
}

proptest! {
    #[test]
    fn roundtrip_wall_clock_datetime(value in datetime_strat()) {
        let serialized: SerializableDateTime = value.into();
        let result: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime = serialized.into();
        prop_assert_eq!(value.seconds, result.seconds);
        prop_assert_eq!(value.nanoseconds, result.nanoseconds);
    }

    #[test]
    fn roundtrip_systemtime(value in systemtime_strat()) {
        let serialized: SerializableDateTime = value.into();
        let result: SystemTime = serialized.into();
        prop_assert_eq!(value, result);
    }

    #[test]
    fn roundtrip_ipaddress(value in ipaddress_strat()) {
        let serialized: SerializableIpAddress = value.into();
        let result: IpAddress = serialized.into();

        match (value, result) {
            (IpAddress::Ipv4(value), IpAddress::Ipv4(result)) => {
                prop_assert_eq!(value, result);
            },
            (IpAddress::Ipv6(value), IpAddress::Ipv6(result)) => {
                prop_assert_eq!(value, result);
            },
            _ => prop_assert!(false),
        }
    }

    #[test]
    fn roundtrip_ipaddresses(value in vec(ipaddress_strat(), 0..100)) {
        let serialized: SerializableIpAddresses = value.clone().into();
        let result: Vec<IpAddress> = serialized.into();

        for (value, result) in value.into_iter().zip(result.into_iter()) {
            match (value, result) {
                (IpAddress::Ipv4(value), IpAddress::Ipv4(result)) => {
                    prop_assert_eq!(value, result);
                },
                (IpAddress::Ipv6(value), IpAddress::Ipv6(result)) => {
                    prop_assert_eq!(value, result);
                },
                _ => prop_assert!(false),
            }
        }
    }
}

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

    #[test]
    fn roundtrip_span_data(value in vec(span_data_strat(), 0..10)) {
        let encoded = super::types::encode_span_data(&value);
        let decoded = super::types::decode_span_data(encoded);
        prop_assert_eq!(value, decoded);
    }
}
