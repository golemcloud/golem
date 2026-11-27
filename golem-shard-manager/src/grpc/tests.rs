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

use super::resolve_pod;
use golem_api_grpc::proto::golem::shardmanager::v1::ExecutorEndpoint;
use golem_common::model::Pod;
use golem_common::model::protobuf::ip_addr_to_proto;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use test_r::test;

const EXECUTOR_V4: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 5);

fn endpoint(port: i32, advertised_ip: Option<IpAddr>) -> Option<ExecutorEndpoint> {
    Some(ExecutorEndpoint {
        port,
        advertised_ip: advertised_ip.map(ip_addr_to_proto),
    })
}

fn remote(ip: IpAddr) -> Option<SocketAddr> {
    Some(SocketAddr::new(ip, 51234))
}

fn pod(ip: IpAddr, port: u16) -> Pod {
    Pod { ip, port }
}

#[test]
fn source_ip_is_used_without_advertised_ip() {
    assert_eq!(
        resolve_pod(remote(EXECUTOR_V4.into()), endpoint(9000, None)).unwrap(),
        pod(EXECUTOR_V4.into(), 9000)
    );

    let v6: IpAddr = "fdaa:0:1:a7b::2".parse().unwrap();
    assert_eq!(
        resolve_pod(remote(v6), endpoint(9000, None)).unwrap(),
        pod(v6, 9000)
    );
}

#[test]
fn ipv4_mapped_source_ip_is_canonicalized() {
    let mapped = IpAddr::V6(EXECUTOR_V4.to_ipv6_mapped());

    assert_eq!(
        resolve_pod(remote(mapped), endpoint(9000, None)).unwrap(),
        resolve_pod(remote(EXECUTOR_V4.into()), endpoint(9000, None)).unwrap(),
    );
}

#[test]
fn advertised_ip_overrides_source_ip() {
    let advertised: IpAddr = "fdaa:0:1:a7b::2".parse().unwrap();

    assert_eq!(
        resolve_pod(
            remote(Ipv4Addr::new(172, 16, 0, 1).into()),
            endpoint(9000, Some(advertised))
        )
        .unwrap(),
        pod(advertised, 9000)
    );
    assert_eq!(
        resolve_pod(None, endpoint(9000, Some(advertised))).unwrap(),
        pod(advertised, 9000)
    );
}

#[test]
fn ipv4_mapped_advertised_ip_is_canonicalized() {
    let mapped = IpAddr::V6(EXECUTOR_V4.to_ipv6_mapped());

    assert_eq!(
        resolve_pod(
            remote(Ipv6Addr::LOCALHOST.into()),
            endpoint(9000, Some(mapped))
        )
        .unwrap(),
        pod(EXECUTOR_V4.into(), 9000)
    );
}

#[test]
fn invalid_endpoints_are_rejected() {
    let source = remote(EXECUTOR_V4.into());
    let invalid_argument = |result: Result<Pod, tonic::Status>| {
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument)
    };

    invalid_argument(resolve_pod(source, None));
    invalid_argument(resolve_pod(None, endpoint(9000, None)));
    invalid_argument(resolve_pod(source, endpoint(-1, None)));
    invalid_argument(resolve_pod(source, endpoint(65536, None)));
    invalid_argument(resolve_pod(source, endpoint(0, None)));
    invalid_argument(resolve_pod(
        source,
        endpoint(9000, Some(Ipv4Addr::BROADCAST.into())),
    ));
    invalid_argument(resolve_pod(
        source,
        endpoint(9000, Some(Ipv4Addr::new(224, 0, 0, 1).into())),
    ));
    invalid_argument(resolve_pod(
        source,
        endpoint(9000, Some("ff02::1".parse().unwrap())),
    ));
    invalid_argument(resolve_pod(
        source,
        endpoint(9000, Some("fe80::1".parse().unwrap())),
    ));
    invalid_argument(resolve_pod(
        source,
        endpoint(9000, Some(Ipv4Addr::UNSPECIFIED.into())),
    ));
    invalid_argument(resolve_pod(
        source,
        endpoint(9000, Some(Ipv6Addr::UNSPECIFIED.into())),
    ));
    invalid_argument(resolve_pod(
        source,
        endpoint(
            9000,
            Some(IpAddr::V6(Ipv4Addr::UNSPECIFIED.to_ipv6_mapped())),
        ),
    ));
    invalid_argument(resolve_pod(
        source,
        Some(ExecutorEndpoint {
            port: 9000,
            advertised_ip: Some(golem_api_grpc::proto::golem::shardmanager::IpAddress {
                kind: Some(
                    golem_api_grpc::proto::golem::shardmanager::ip_address::Kind::Ipv6(vec![0; 4]),
                ),
            }),
        }),
    ));
}
