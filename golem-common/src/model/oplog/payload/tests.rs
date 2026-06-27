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

use test_r::test;

use crate::model::Timestamp;
use crate::model::invocation_context::{AttributeValue, SpanId};
use crate::model::oplog::raw_types::SpanData;
use crate::model::oplog::types::{
    SerializableDateTime, SerializableFileTimes, SerializableHttpErrorCode, SerializableHttpMethod,
    SerializableHttpVersion, SerializableIpAddress, SerializableIpAddresses,
    SerializableP3CliErrorCode, SerializableP3DescriptorType, SerializableP3DirectoryEntry,
    SerializableP3FileSystemError, SerializableP3FsErrorCode, SerializableP3HttpClientSend,
    SerializableP3HttpClientSendResult, SerializableP3HttpConsumeBodyResult,
    SerializableP3HttpRequestOptions, SerializableP3HttpScheme,
    SerializableP3IpAddress, SerializableP3IpSocketAddress, SerializableP3SocketErrorCode,
    SerializableP3UdpDatagram, SerializableResponseHeaders, SerializableStreamError,
};
use crate::model::oplog::{
    HostPayloadPair, HostRequest, HostRequestFileSystemPath, HostRequestFileSystemPathAndOffset,
    HostRequestKVCacheKey, HostRequestKVCacheKeyAndTtl, HostRequestKVCacheKeyValueAndTtl,
    HostRequestMonotonicClockDuration, HostRequestMonotonicClockTimestamp, HostRequestNoInput,
    HostRequestP3HttpClientSend, HostRequestP3SocketsUdpSend, HostRequestRandomBytes, HostResponse,
    HostResponseKVDelete, HostResponseKVGet, HostResponseKVUnit,
    HostResponseMonotonicClockTimestamp, HostResponseP3BlobstoreIncomingValueStream,
    HostResponseP3CliStream, HostResponseP3FileSystemByteStream,
    HostResponseP3FileSystemDirectoryEntryStream, HostResponseP3FileSystemStat,
    HostResponseP3HttpClientConsumeBodyResult, HostResponseP3HttpClientSendResult,
    HostResponseP3KeyvalueIncomingValueStream,
    HostResponseP3MonotonicClockUnit, HostResponseP3SocketsTcpStream,
    HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend, HostResponseRandomBytes,
    HostResponseRandomSeed, HostResponseRandomU64, HostResponseWallClock, host_functions,
};
use http::Version;
use iso8601_timestamp as iso_ts;
use proptest::collection::vec;
use proptest::option::of;
use proptest::prelude::*;
use proptest::strategy::LazyJust;
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::ops::Add;
use std::time::{Duration, SystemTime};
use wasmtime_wasi::StreamError;
use wasmtime_wasi::p2::bindings::sockets::network::IpAddress;
use wasmtime_wasi_http::p2::bindings::http::types::{
    DnsErrorPayload, ErrorCode, FieldSizePayload, TlsAlertReceivedPayload,
};

fn datetime_strat()
-> impl Strategy<Value = wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime> {
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
                v.into_iter()
                    .map(|(k, v)| (k, AttributeValue::String(v)))
                    .collect()
            }),
            any::<bool>()
        )
            .prop_map(|(span_id, _start, parent_id, attributes, inherited)| {
                SpanData::LocalSpan {
                    span_id,
                    start: Timestamp(iso_ts::Timestamp::parse("2023-01-01T00:00:00Z").unwrap()),
                    parent_id,
                    linked_context: None,
                    attributes,
                    inherited,
                }
            }),
        any::<u64>()
            .prop_map(|x| SpanId(NonZeroU64::new(x + 1).unwrap()))
            .prop_map(|span_id| SpanData::ExternalSpan { span_id }),
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

#[test]
fn p3_clock_host_payload_pairs_roundtrip() {
    assert_host_payload_pair_roundtrip::<host_functions::P3SystemClockNow>(
        HostRequestNoInput {},
        HostResponseWallClock {
            time: SerializableDateTime {
                seconds: 123,
                nanoseconds: 456,
            },
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3SystemClockGetResolution>(
        HostRequestNoInput {},
        HostResponseMonotonicClockTimestamp { nanos: 1 },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3MonotonicClockNow>(
        HostRequestNoInput {},
        HostResponseMonotonicClockTimestamp { nanos: 2 },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3MonotonicClockGetResolution>(
        HostRequestNoInput {},
        HostResponseMonotonicClockTimestamp { nanos: 3 },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3MonotonicClockWaitUntil>(
        HostRequestMonotonicClockTimestamp { nanos: 4 },
        HostResponseP3MonotonicClockUnit {},
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3MonotonicClockWaitFor>(
        HostRequestMonotonicClockDuration {
            duration_in_nanos: 5,
        },
        HostResponseP3MonotonicClockUnit {},
    );
}

#[test]
fn p3_random_host_payload_pairs_roundtrip() {
    assert_host_payload_pair_roundtrip::<host_functions::P3RandomRandomGetRandomBytes>(
        HostRequestRandomBytes { length: 4 },
        HostResponseRandomBytes {
            bytes: vec![1, 2, 3, 4],
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3RandomRandomGetRandomU64>(
        HostRequestNoInput {},
        HostResponseRandomU64 { value: 42 },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3RandomInsecureGetInsecureRandomBytes>(
        HostRequestRandomBytes { length: 3 },
        HostResponseRandomBytes {
            bytes: vec![5, 6, 7],
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3RandomInsecureGetInsecureRandomU64>(
        HostRequestNoInput {},
        HostResponseRandomU64 { value: 43 },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3RandomInsecureSeedGetInsecureSeed>(
        HostRequestNoInput {},
        HostResponseRandomSeed { lo: 44, hi: 45 },
    );
}

#[test]
fn p3_cli_host_payload_pairs_roundtrip() {
    assert_host_payload_pair_roundtrip::<host_functions::P3CliStdinReadViaStream>(
        HostRequestNoInput {},
        HostResponseP3CliStream {
            contents: b"stdin prefix".to_vec(),
            result: Err(SerializableP3CliErrorCode::Io),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3CliStdoutWriteViaStream>(
        HostRequestNoInput {},
        HostResponseP3CliStream {
            contents: b"stdout bytes".to_vec(),
            result: Ok(()),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3CliStderrWriteViaStream>(
        HostRequestNoInput {},
        HostResponseP3CliStream {
            contents: b"stderr prefix".to_vec(),
            result: Err(SerializableP3CliErrorCode::Pipe),
        },
    );
}

#[test]
fn p3_tcp_socket_host_payload_pairs_roundtrip() {
    assert_host_payload_pair_roundtrip::<host_functions::P3SocketsTypesTcpSocketSend>(
        HostRequestNoInput {},
        HostResponseP3SocketsTcpStream {
            contents: b"outgoing tcp bytes".to_vec(),
            result: Ok(()),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3SocketsTypesTcpSocketReceive>(
        HostRequestNoInput {},
        HostResponseP3SocketsTcpStream {
            contents: b"incoming tcp bytes".to_vec(),
            result: Err(SerializableP3SocketErrorCode::ConnectionReset),
        },
    );
}

#[test]
fn p3_udp_socket_host_payload_pairs_roundtrip() {
    let remote_address = SerializableP3IpSocketAddress {
        address: SerializableP3IpAddress::IPv4 {
            address: [127, 0, 0, 1],
        },
        port: 1234,
        flow_info: None,
        scope_id: None,
    };

    assert_host_payload_pair_roundtrip::<host_functions::P3SocketsTypesUdpSocketSend>(
        HostRequestP3SocketsUdpSend {
            data: b"outgoing udp bytes".to_vec(),
            remote_address: Some(remote_address.clone()),
        },
        HostResponseP3SocketsUdpSend { result: Ok(()) },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3SocketsTypesUdpSocketReceive>(
        HostRequestNoInput {},
        HostResponseP3SocketsUdpReceive {
            result: Ok(SerializableP3UdpDatagram {
                data: b"incoming udp bytes".to_vec(),
                remote_address,
            }),
        },
    );
}

#[test]
fn p3_http_client_send_host_payload_pairs_roundtrip() {
    let request = SerializableP3HttpClientSend {
        method: SerializableHttpMethod::Post,
        scheme: Some(SerializableP3HttpScheme::Https),
        authority: Some("example.com".to_string()),
        path_with_query: Some("/path?query=1".to_string()),
        headers: HashMap::from([(
            "x-custom".to_string(),
            vec![b"value".to_vec(), b"other".to_vec()],
        )]),
        options: Some(SerializableP3HttpRequestOptions {
            connect_timeout_nanos: Some(5_000_000_000),
            first_byte_timeout_nanos: Some(10_000_000_000),
            between_bytes_timeout_nanos: None,
        }),
    };

    // Success result: response head (status + headers) round-trips.
    assert_host_payload_pair_roundtrip::<host_functions::P3HttpClientSend>(
        HostRequestP3HttpClientSend {
            request: request.clone(),
        },
        HostResponseP3HttpClientSendResult {
            result: SerializableP3HttpClientSendResult::Success(SerializableResponseHeaders {
                status: 200,
                headers: HashMap::from([(
                    "content-type".to_string(),
                    vec![b"application/json".to_vec()],
                )]),
            }),
        },
    );

    // Error result (blocker #2): a replayed transport/protocol ErrorCode
    // round-trips back to the guest instead of being flattened to a generic
    // failure.
    assert_host_payload_pair_roundtrip::<host_functions::P3HttpClientSend>(
        HostRequestP3HttpClientSend {
            request: request.clone(),
        },
        HostResponseP3HttpClientSendResult {
            result: SerializableP3HttpClientSendResult::HttpError(
                SerializableHttpErrorCode::ConnectionRefused,
            ),
        },
    );

    // Empty/None edge cases on the request head and an error-code variant
    // carrying a payload, to exercise the nested serializable types.
    assert_host_payload_pair_roundtrip::<host_functions::P3HttpClientSend>(
        HostRequestP3HttpClientSend {
            request: SerializableP3HttpClientSend {
                method: SerializableHttpMethod::Other("LINK".to_string()),
                scheme: Some(SerializableP3HttpScheme::Other("foo".to_string())),
                authority: None,
                path_with_query: None,
                headers: HashMap::new(),
                options: None,
            },
        },
        HostResponseP3HttpClientSendResult {
            result: SerializableP3HttpClientSendResult::HttpError(
                SerializableHttpErrorCode::InternalError(Some("boom".to_string())),
            ),
        },
    );
}

#[test]
fn p3_http_client_consume_body_host_payload_pairs_roundtrip() {
    // Clean close with trailers: body bytes + delivered trailers round-trip.
    assert_host_payload_pair_roundtrip::<host_functions::P3HttpClientConsumeBody>(
        HostRequestNoInput {},
        HostResponseP3HttpClientConsumeBodyResult {
            contents: b"hello body bytes".to_vec(),
            result: SerializableP3HttpConsumeBodyResult::Trailers(Some(HashMap::from([(
                "x-trailer".to_string(),
                vec![b"trailer-value".to_vec()],
            )]))),
        },
    );

    // Clean close without trailers.
    assert_host_payload_pair_roundtrip::<host_functions::P3HttpClientConsumeBody>(
        HostRequestNoInput {},
        HostResponseP3HttpClientConsumeBodyResult {
            contents: b"body without trailers".to_vec(),
            result: SerializableP3HttpConsumeBodyResult::Trailers(None),
        },
    );

    // Errored body: the partial bytes observed before the error are still
    // recorded and replay, and the ErrorCode round-trips (surfaced to the
    // guest via the trailers future in p3).
    assert_host_payload_pair_roundtrip::<host_functions::P3HttpClientConsumeBody>(
        HostRequestNoInput {},
        HostResponseP3HttpClientConsumeBodyResult {
            contents: b"partial".to_vec(),
            result: SerializableP3HttpConsumeBodyResult::HttpError(
                SerializableHttpErrorCode::HttpResponseBodySize(Some(123)),
            ),
        },
    );
}

#[test]
fn p3_keyvalue_cache_host_payload_pairs_roundtrip() {
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueTypesIncomingValueConsumeAsync>(
        HostRequestNoInput {},
        HostResponseP3KeyvalueIncomingValueStream {
            contents: b"incoming value bytes".to_vec(),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3BlobstoreTypesIncomingValueConsumeAsync>(
        HostRequestNoInput {},
        HostResponseP3BlobstoreIncomingValueStream {
            contents: b"incoming blob bytes".to_vec(),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueCacheGet>(
        HostRequestKVCacheKey {
            key: "cache-key".to_string(),
        },
        HostResponseKVGet {
            result: Ok(Some(b"cached".to_vec())),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueCacheExists>(
        HostRequestKVCacheKey {
            key: "cache-key".to_string(),
        },
        HostResponseKVDelete { result: Ok(true) },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueCacheSet>(
        HostRequestKVCacheKeyValueAndTtl {
            key: "cache-key".to_string(),
            length: 6,
            ttl_ms: Some(1_000),
        },
        HostResponseKVUnit { result: Ok(()) },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueCacheGetOrSet>(
        HostRequestKVCacheKey {
            key: "cache-key".to_string(),
        },
        HostResponseKVGet { result: Ok(None) },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueCacheDelete>(
        HostRequestKVCacheKey {
            key: "cache-key".to_string(),
        },
        HostResponseKVUnit { result: Ok(()) },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueCacheVacancyFill>(
        HostRequestKVCacheKeyAndTtl {
            key: "cache-key".to_string(),
            ttl_ms: Some(1_000),
        },
        HostResponseKVUnit { result: Ok(()) },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3KeyvalueCacheVacancyDrop>(
        HostRequestKVCacheKey {
            key: "cache-key".to_string(),
        },
        HostResponseKVUnit { result: Ok(()) },
    );
}

#[test]
fn p3_filesystem_host_payload_pairs_roundtrip() {
    assert_host_payload_pair_roundtrip::<host_functions::P3FilesystemTypesDescriptorReadViaStream>(
        HostRequestFileSystemPathAndOffset {
            path: "/tmp/file.txt".to_string(),
            offset: 12,
        },
        HostResponseP3FileSystemByteStream {
            contents: b"file bytes".to_vec(),
            result: Ok(()),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3FilesystemTypesDescriptorWriteViaStream>(
        HostRequestFileSystemPathAndOffset {
            path: "/tmp/file.txt".to_string(),
            offset: 5,
        },
        HostResponseP3FileSystemByteStream {
            contents: b"written".to_vec(),
            result: Err(SerializableP3FsErrorCode::NoEntry),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3FilesystemTypesDescriptorAppendViaStream>(
        HostRequestFileSystemPath {
            path: "/tmp/file.txt".to_string(),
        },
        HostResponseP3FileSystemByteStream {
            contents: b"appended".to_vec(),
            result: Ok(()),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3FilesystemTypesDescriptorReadDirectory>(
        HostRequestFileSystemPath {
            path: "/tmp".to_string(),
        },
        HostResponseP3FileSystemDirectoryEntryStream {
            entries: vec![SerializableP3DirectoryEntry {
                type_: SerializableP3DescriptorType::RegularFile,
                name: "file.txt".to_string(),
            }],
            result: Ok(()),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3FilesystemTypesDescriptorStat>(
        HostRequestFileSystemPath {
            path: "/tmp/file.txt".to_string(),
        },
        HostResponseP3FileSystemStat {
            result: Ok(SerializableFileTimes {
                data_access_timestamp: Some(SerializableDateTime {
                    seconds: 1,
                    nanoseconds: 2,
                }),
                data_modification_timestamp: None,
            }),
        },
    );
    assert_host_payload_pair_roundtrip::<host_functions::P3FilesystemTypesDescriptorStatAt>(
        HostRequestFileSystemPath {
            path: "/tmp/missing.txt".to_string(),
        },
        HostResponseP3FileSystemStat {
            result: Err(SerializableP3FileSystemError::ErrorCode(
                SerializableP3FsErrorCode::NoEntry,
            )),
        },
    );
}

fn assert_host_payload_pair_roundtrip<Pair>(request: Pair::Req, response: Pair::Resp)
where
    Pair: HostPayloadPair,
    Pair::Req: Clone + std::fmt::Debug + PartialEq + TryFrom<HostRequest, Error = String>,
    Pair::Resp: Clone + std::fmt::Debug + PartialEq,
{
    let request_payload: HostRequest = request.clone().into();
    let request_bytes = desert_rust::serialize_to_byte_vec(&request_payload).unwrap();
    let request_roundtrip: HostRequest = desert_rust::deserialize(&request_bytes).unwrap();
    assert_eq!(Pair::Req::try_from(request_roundtrip).unwrap(), request);

    let response_payload: HostResponse = response.clone().into();
    let response_bytes = desert_rust::serialize_to_byte_vec(&response_payload).unwrap();
    let response_roundtrip: HostResponse = desert_rust::deserialize(&response_bytes).unwrap();
    assert_eq!(Pair::Resp::try_from(response_roundtrip).unwrap(), response);

    let function_name_bytes =
        desert_rust::serialize_to_byte_vec(&Pair::HOST_FUNCTION_NAME).unwrap();
    let function_name_roundtrip: host_functions::HostFunctionName =
        desert_rust::deserialize(&function_name_bytes).unwrap();
    assert_eq!(function_name_roundtrip, Pair::HOST_FUNCTION_NAME);
}

#[test]
fn p3_http_payload_additions_keep_existing_host_request_binary_tags_stable() {
    let old_kv_bucket_and_key_bytes = [
        0, 25, 0, 0, 12, b'b', b'u', b'c', b'k', b'e', b't', 6, b'k', b'e', b'y',
    ];

    let decoded: HostRequest = desert_rust::deserialize(&old_kv_bucket_and_key_bytes).unwrap();

    assert_eq!(
        decoded,
        HostRequest::KVBucketAndKey(crate::model::oplog::HostRequestKVBucketAndKey {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
        })
    );
}

#[test]
fn p3_http_payload_additions_keep_existing_host_function_name_binary_tags_stable() {
    let old_random_get_random_bytes_name = [0, 36, 0];

    let decoded: host_functions::HostFunctionName =
        desert_rust::deserialize(&old_random_get_random_bytes_name).unwrap();

    assert_eq!(
        decoded,
        host_functions::HostFunctionName::RandomGetRandomBytes
    );
}

#[test]
fn p3_http_payload_additions_keep_preexisting_blobstore_create_container_tag_stable() {
    let old_blobstore_create_container_name = [0, 65, 0];

    let decoded: host_functions::HostFunctionName =
        desert_rust::deserialize(&old_blobstore_create_container_name).unwrap();

    assert_eq!(
        decoded,
        host_functions::HostFunctionName::BlobstoreBlobstoreCreateContainer
    );
}

#[test]
fn p3_http_payload_additions_p3_filesystem_stat_function_name_roundtrips_from_string() {
    let function_name = host_functions::HostFunctionName::P3FilesystemTypesDescriptorStat;
    let serialized = function_name.to_string();

    assert_eq!(
        host_functions::HostFunctionName::from(serialized.as_str()),
        function_name
    );
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
        let serialized: SerializableHttpVersion = version.try_into().unwrap();
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

    #[test]
    fn roundtrip_ip_address_binary_serialization(value in ipaddress_strat()) {
        let serialized: SerializableIpAddress = value.into();
        let bytes = desert_rust::serialize_to_byte_vec(&serialized).unwrap();
        let deserialized: SerializableIpAddress = desert_rust::deserialize(&bytes).unwrap();
        prop_assert_eq!(serialized, deserialized);
    }

    #[test]
    fn roundtrip_stream_error_closed(_dummy in Just(())) {
        let original = StreamError::Closed;
        let serializable: SerializableStreamError = original.into();
        let roundtripped: StreamError = serializable.into();
        prop_assert!(matches!(roundtripped, StreamError::Closed));
    }

}
