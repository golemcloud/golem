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
use crate::durable_host::HostFailureKind;
use golem_common::model::oplog::host_functions::{
    P3SocketsTypesUdpSocketReceive, P3SocketsTypesUdpSocketSend,
};
use golem_common::model::oplog::types::{
    SerializableP3IpAddress, SerializableP3IpAddresses, SerializableP3IpNameLookupError,
    SerializableP3IpSocketAddress, SerializableP3SocketErrorCode, SerializableP3UdpDatagram,
};
use golem_common::model::oplog::{
    HostPayloadPair, HostRequest, HostRequestNoInput, HostRequestP3SocketsUdpSend, HostResponse,
    HostResponseP3SocketsUdpReceive, HostResponseP3SocketsUdpSend,
};
use test_r::test;
use test_r::timeout;
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;
use wasmtime::component::StreamReader;
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p3::bindings::sockets::ip_name_lookup;

#[test]
fn p3_ip_name_lookup_address_payload_mapping_roundtrips() {
    let ipv4 = types::IpAddress::Ipv4((127, 0, 0, 1));
    let ipv6 = types::IpAddress::Ipv6((0, 0, 0, 0, 0, 0, 0, 1));

    let serialized = SerializableP3IpAddresses::from(vec![ipv4, ipv6]);
    let replayed = Vec::<types::IpAddress>::from(serialized);

    assert_p3_ip_address_eq(replayed[0], ipv4);
    assert_p3_ip_address_eq(replayed[1], ipv6);
}

#[test]
fn p3_ip_name_lookup_error_payload_mapping_roundtrips_named_codes() {
    assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::AccessDenied);
    assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::InvalidArgument);
    assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::NameUnresolvable);
    assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::TemporaryResolverFailure);
    assert_p3_ip_name_lookup_error_roundtrip(ip_name_lookup::ErrorCode::PermanentResolverFailure);
}

/// Transient resolver failures must route through the worker-level retry machinery instead
/// of surfacing as guest-visible error values; failures that cannot succeed on a retry must
/// be persisted and returned to the guest (mirroring the P2 `resolve_addresses`
/// classification).
#[test]
fn p3_ip_name_lookup_error_classification_matches_p2_semantics() {
    assert_eq!(
        classify_p3_ip_name_lookup_error(&SerializableP3IpNameLookupError::AccessDenied),
        HostFailureKind::Permanent
    );
    assert_eq!(
        classify_p3_ip_name_lookup_error(&SerializableP3IpNameLookupError::InvalidArgument),
        HostFailureKind::Permanent
    );
    assert_eq!(
        classify_p3_ip_name_lookup_error(&SerializableP3IpNameLookupError::NameUnresolvable),
        HostFailureKind::Permanent
    );
    assert_eq!(
        classify_p3_ip_name_lookup_error(
            &SerializableP3IpNameLookupError::PermanentResolverFailure
        ),
        HostFailureKind::Permanent
    );
    assert_eq!(
        classify_p3_ip_name_lookup_error(
            &SerializableP3IpNameLookupError::TemporaryResolverFailure
        ),
        HostFailureKind::Transient
    );
    assert_eq!(
        classify_p3_ip_name_lookup_error(&SerializableP3IpNameLookupError::Other(Some(
            "resolver said maybe later".to_string()
        ))),
        HostFailureKind::Transient
    );
    assert_eq!(
        classify_p3_ip_name_lookup_error(&SerializableP3IpNameLookupError::Other(None)),
        HostFailureKind::Transient
    );
}

#[test]
fn p3_ip_name_lookup_other_error_payload_mapping_preserves_message() {
    let error = ip_name_lookup::ErrorCode::Other(Some("resolver said no".to_string()));
    let serialized = SerializableP3IpNameLookupError::from(error);
    let replayed = ip_name_lookup::ErrorCode::from(serialized);

    match replayed {
        ip_name_lookup::ErrorCode::Other(Some(message)) => {
            assert_eq!(message, "resolver said no")
        }
        other => panic!("unexpected replayed error: {other:?}"),
    }
}

#[test]
fn p3_socket_error_payload_mapping_roundtrips_named_codes() {
    assert_p3_socket_error_roundtrip(types::ErrorCode::AccessDenied);
    assert_p3_socket_error_roundtrip(types::ErrorCode::NotSupported);
    assert_p3_socket_error_roundtrip(types::ErrorCode::InvalidArgument);
    assert_p3_socket_error_roundtrip(types::ErrorCode::OutOfMemory);
    assert_p3_socket_error_roundtrip(types::ErrorCode::Timeout);
    assert_p3_socket_error_roundtrip(types::ErrorCode::InvalidState);
    assert_p3_socket_error_roundtrip(types::ErrorCode::AddressNotBindable);
    assert_p3_socket_error_roundtrip(types::ErrorCode::AddressInUse);
    assert_p3_socket_error_roundtrip(types::ErrorCode::RemoteUnreachable);
    assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionRefused);
    assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionBroken);
    assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionReset);
    assert_p3_socket_error_roundtrip(types::ErrorCode::ConnectionAborted);
    assert_p3_socket_error_roundtrip(types::ErrorCode::DatagramTooLarge);
}

#[test]
fn p3_socket_other_error_payload_mapping_preserves_message() {
    let error = types::ErrorCode::Other(Some("socket said no".to_string()));
    let serialized = SerializableP3SocketErrorCode::from(error);
    let replayed = types::ErrorCode::from(serialized);

    match replayed {
        types::ErrorCode::Other(Some(message)) => assert_eq!(message, "socket said no"),
        other => panic!("unexpected replayed error: {other:?}"),
    }
}

#[test]
fn p3_udp_socket_address_payload_mapping_roundtrips_ipv4() {
    let address = types::IpSocketAddress::Ipv4(types::Ipv4SocketAddress {
        port: 1234,
        address: (127, 0, 0, 1),
    });

    let serialized = SerializableP3IpSocketAddress::from(address);
    let replayed = types::IpSocketAddress::from(serialized);

    assert_p3_socket_address_eq(replayed, address);
}

#[test]
fn p3_udp_socket_address_payload_mapping_roundtrips_ipv6() {
    let address = types::IpSocketAddress::Ipv6(types::Ipv6SocketAddress {
        port: 1234,
        flow_info: 56,
        address: (0, 1, 2, 3, 4, 5, 6, 7),
        scope_id: 78,
    });

    let serialized = SerializableP3IpSocketAddress::from(address);
    let replayed = types::IpSocketAddress::from(serialized);

    assert_p3_socket_address_eq(replayed, address);
}

#[test]
fn p3_udp_socket_host_payload_pair_roundtrips() {
    let remote_address = SerializableP3IpSocketAddress {
        address: SerializableP3IpAddress::IPv4 {
            address: [127, 0, 0, 1],
        },
        port: 1234,
        flow_info: None,
        scope_id: None,
    };

    assert_host_payload_pair_roundtrip::<P3SocketsTypesUdpSocketSend>(
        HostRequestP3SocketsUdpSend {
            data: b"outgoing udp bytes".to_vec(),
            remote_address: Some(remote_address.clone()),
        },
        HostResponseP3SocketsUdpSend { result: Ok(()) },
    );
    assert_host_payload_pair_roundtrip::<P3SocketsTypesUdpSocketReceive>(
        HostRequestNoInput {},
        HostResponseP3SocketsUdpReceive {
            result: Ok(SerializableP3UdpDatagram {
                data: b"incoming udp bytes".to_vec(),
                remote_address,
            }),
        },
    );
}

/// A finite upstream stream piped into [`TcpReceiveForwardConsumer`] must
/// forward its bytes and then close the bounded channel once the upstream
/// reaches EOF, so the durable receive task observes `bytes_rx.recv() ==
/// None` and can finalize the terminal instead of hanging.
///
/// EOF closes the channel because the wasmtime host-to-host driver drops the
/// consumer (and thus its [`PollSender`]) once the upstream producer reports
/// `StreamResult::Dropped`. That transfer only runs while the store's event
/// loop is driven, so the channel is drained from inside the driven
/// `run_concurrent` closure (the event loop polls the closure and the pushed
/// transfer future together). Draining outside the event loop — e.g. after a
/// non-draining `run_concurrent` returns — would never run the transfer.
///
/// Because the consumer is demand-gated, the driver grants one permit before
/// each read (mirroring the durable receive task), so a chunk is forwarded
/// only when demanded and EOF still closes the channel.
#[test]
#[timeout("10s")]
async fn tcp_receive_forward_consumer_closes_channel_after_source_eof() {
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());

    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>(1);
    let (permit_tx, permit_rx) = mpsc::channel::<()>(1);

    let chunks = store
        .run_concurrent(async move |accessor| -> wasmtime::Result<Vec<Vec<u8>>> {
            let mut chunk_rx = chunk_rx;
            accessor.with(|mut store| {
                let stream = StreamReader::new(&mut store, b"abc".to_vec())?;
                stream.pipe(
                    &mut store,
                    TcpReceiveForwardConsumer::new(PollSender::new(chunk_tx), permit_rx),
                )
            })?;

            let mut chunks = Vec::new();
            loop {
                // Grant one permit per demand, exactly like the durable task.
                let _ = permit_tx.try_send(());
                match chunk_rx.recv().await {
                    Some(chunk) => chunks.push(chunk),
                    None => break,
                }
            }
            Ok(chunks)
        })
        .await
        .unwrap()
        .unwrap();

    assert_eq!(chunks, vec![b"abc".to_vec()]);
}

#[test]
#[timeout("10s")]
async fn tcp_receive_forward_consumer_does_not_prefetch_before_durable_demand() {
    let mut config = Config::new();
    config.concurrency_support(true);
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());

    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>(1);
    let (permit_tx, permit_rx) = mpsc::channel::<()>(1);

    store
        .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
            let mut chunk_rx = chunk_rx;
            // Keep the permit sender alive but never grant a permit: the
            // consumer must not forward (prefetch) any chunk before a demand.
            // (Dropping it would close `permit_rx`, making the consumer tear
            // the channel down and `try_recv` return `Disconnected`.)
            let _permit_tx = permit_tx;
            accessor.with(|mut store| {
                let stream = StreamReader::new(&mut store, b"prefetched".to_vec())?;
                stream.pipe(
                    &mut store,
                    TcpReceiveForwardConsumer::new(PollSender::new(chunk_tx), permit_rx),
                )
            })?;

            tokio::time::sleep(std::time::Duration::from_millis(25)).await;

            assert_eq!(chunk_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
            Ok(())
        })
        .await
        .unwrap()
        .unwrap();
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
    let function_name_roundtrip: golem_common::model::oplog::host_functions::HostFunctionName =
        desert_rust::deserialize(&function_name_bytes).unwrap();
    assert_eq!(function_name_roundtrip, Pair::HOST_FUNCTION_NAME);
}

fn assert_p3_ip_name_lookup_error_roundtrip(error: ip_name_lookup::ErrorCode) {
    let expected = format!("{error:?}");
    let serialized = SerializableP3IpNameLookupError::from(error);
    let replayed = ip_name_lookup::ErrorCode::from(serialized);
    assert_eq!(format!("{replayed:?}"), expected);
}

fn assert_p3_socket_error_roundtrip(error: types::ErrorCode) {
    let expected = format!("{error:?}");
    let serialized = SerializableP3SocketErrorCode::from(error);
    let replayed = types::ErrorCode::from(serialized);
    assert_eq!(format!("{replayed:?}"), expected);
}

fn assert_p3_ip_address_eq(actual: types::IpAddress, expected: types::IpAddress) {
    match (actual, expected) {
        (types::IpAddress::Ipv4(actual), types::IpAddress::Ipv4(expected)) => {
            assert_eq!(actual, expected)
        }
        (types::IpAddress::Ipv6(actual), types::IpAddress::Ipv6(expected)) => {
            assert_eq!(actual, expected)
        }
        (actual, expected) => panic!("IP address mismatch: {actual:?} != {expected:?}"),
    }
}

fn assert_p3_socket_address_eq(actual: types::IpSocketAddress, expected: types::IpSocketAddress) {
    match (actual, expected) {
        (types::IpSocketAddress::Ipv4(actual), types::IpSocketAddress::Ipv4(expected)) => {
            assert_eq!(actual.port, expected.port);
            assert_eq!(actual.address, expected.address);
        }
        (types::IpSocketAddress::Ipv6(actual), types::IpSocketAddress::Ipv6(expected)) => {
            assert_eq!(actual.port, expected.port);
            assert_eq!(actual.flow_info, expected.flow_info);
            assert_eq!(actual.address, expected.address);
            assert_eq!(actual.scope_id, expected.scope_id);
        }
        (actual, expected) => {
            panic!("IP socket address mismatch: {actual:?} != {expected:?}")
        }
    }
}
