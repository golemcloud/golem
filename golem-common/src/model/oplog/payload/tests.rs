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
    SerializableDateTime, SerializableHttpVersion, SerializableIpAddress, SerializableStreamError,
};
use http::Version;
use iso8601_timestamp as iso_ts;
use proptest::collection::vec;
use proptest::prelude::*;
use std::net::IpAddr;
use std::num::NonZeroU64;
use std::ops::Add;
use std::time::{Duration, SystemTime};
use wasmtime_wasi::StreamError;

fn datetime_strat()
-> impl Strategy<Value = wasmtime_wasi::p3::bindings::clocks::system_clock::Instant> {
    (0..(i64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
        wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
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

/// Produces `IpAddr` instances directly so the property test no longer depends on the
/// wasmtime-wasi p2 `IpAddress` binding.
fn ipaddr_strat() -> impl Strategy<Value = IpAddr> {
    prop_oneof! {
        (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>())
            .prop_map(|(a, b, c, d)| IpAddr::from([a, b, c, d])),
        (any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>())
            .prop_map(|(a, b, c, d, e, f, g, h)| IpAddr::from([a, b, c, d, e, f, g, h])),
    }
}

fn span_data_strat() -> impl Strategy<Value = SpanData> {
    prop_oneof![
        (
            any::<u64>().prop_map(|x| SpanId(NonZeroU64::new(x + 1).unwrap())),
            any::<i64>(),
            proptest::option::of(any::<u64>().prop_map(|x| SpanId(NonZeroU64::new(x + 1).unwrap()))),
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
    fn roundtrip_system_clock_instant(value in datetime_strat()) {
        let serialized: SerializableDateTime = value.into();
        let result: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant = serialized.into();
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
    fn roundtrip_ipaddr(value in ipaddr_strat()) {
        let serialized: SerializableIpAddress = value.into();
        let result: IpAddr = serialized.into();
        prop_assert_eq!(value, result);
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

// TODO(p3) Blocker 3: re-add round-trip tests against p3 wasmtime types when HTTP durability is
// re-implemented. The previous `error_code_strat` / `field_size_payload_strat` strategies and
// the `test_error_code_roundtrip` test built `wasmtime_wasi_http::p2::bindings::http::types::ErrorCode`
// values and round-tripped them through `SerializableHttpErrorCode`; both have been deleted along
// with the wasmtime-side `From` conversions in `types.rs`.

proptest! {
    #[test]
    fn test_http_version_roundtrip(version in version_strat()) {
        let serialized: SerializableHttpVersion = version.try_into().unwrap();
        let deserialized: Version = serialized.into();
        prop_assert_eq!(version, deserialized);
    }

    #[test]
    fn roundtrip_span_data(value in vec(span_data_strat(), 0..10)) {
        let encoded = super::types::encode_span_data(&value);
        let decoded = super::types::decode_span_data(encoded);
        prop_assert_eq!(value, decoded);
    }

    #[test]
    fn roundtrip_ip_address_binary_serialization(value in ipaddr_strat()) {
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
