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

use super::{lease_to_proto, registration_to_proto, resolve_pod};
use crate::sharding::{
    RegisterAck, ShardAssignmentPush, ShardEpoch, ShardLeaseGrant, ShardLeaseRevision,
};
use golem_api_grpc::proto::golem::shardmanager::v1::ExecutorEndpoint;
use golem_common::model::Pod;
use golem_common::model::protobuf::ip_addr_to_proto;
use jiff::Timestamp;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use test_r::test;

const EXECUTOR_V4: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 5);

#[test]
fn registration_renewal_and_assignment_encode_the_delivered_executor_count() {
    let now = Timestamp::new(1_700_000_000, 0).unwrap();
    [
        (4_u32, 2_u32, 3_u32),
        (7, 5, 6),
        (u32::MAX, u32::MAX - 1, u32::MAX - 2),
    ]
    .into_iter()
    .for_each(|(registration_count, renewal_count, assignment_count)| {
        let grant = ShardLeaseGrant {
            shard_epochs: BTreeMap::from([(golem_common::model::ShardId::new(2), ShardEpoch(9))]),
            expires_at: now + jiff::SignedDuration::from_secs(45),
            executor_count: registration_count as usize,
            revision: ShardLeaseRevision(7),
        };
        let registration = registration_to_proto(
            RegisterAck {
                number_of_shards: 8,
                grant: grant.clone(),
            },
            now,
        )
        .unwrap();
        assert_eq!(registration.executor_count, registration_count);
        assert_eq!(registration.number_of_shards, 8);
        assert_eq!(registration.revision, 7);
        assert_eq!(registration.lease_ttl.unwrap().seconds, 45);

        let assignment = ShardAssignmentPush {
            shard_epochs: grant.shard_epochs.clone(),
            number_of_shards: 8,
            executor_count: assignment_count as usize,
            revision: ShardLeaseRevision(9),
        }
        .to_proto()
        .unwrap();
        assert_eq!(assignment.executor_count, assignment_count);
        assert_eq!(assignment.number_of_shards, 8);
        assert_eq!(assignment.revision, 9);

        let renewal = lease_to_proto(
            ShardLeaseGrant {
                executor_count: renewal_count as usize,
                revision: ShardLeaseRevision(8),
                ..grant
            },
            now,
        )
        .unwrap();
        assert_eq!(renewal.executor_count, renewal_count);
        assert_eq!(renewal.revision, 8);
        assert_eq!(renewal.lease_ttl.unwrap().seconds, 45);
    });
}

#[test]
fn registration_and_renewal_refuse_invalid_executor_counts_as_internal_errors() {
    use golem_api_grpc::proto::golem::shardmanager::v1::{
        ShardLeaseError, ShardManagerError, shard_lease_error, shard_manager_error,
    };
    use golem_common::base_model::api::error_code::INTERNAL_UNKNOWN;

    let now = Timestamp::new(1_700_000_000, 0).unwrap();
    let invalid_counts = std::iter::once((0_usize, "must not be 0"));
    #[cfg(target_pointer_width = "64")]
    let invalid_counts = invalid_counts.chain([
        (4_294_967_296, "4294967296 does not fit a u32"),
        (4_294_967_299, "4294967299 does not fit a u32"),
    ]);

    invalid_counts.for_each(|(executor_count, detail)| {
        let grant = ShardLeaseGrant {
            shard_epochs: BTreeMap::from([(golem_common::model::ShardId::new(2), ShardEpoch(9))]),
            expires_at: now + jiff::SignedDuration::from_secs(45),
            executor_count,
            revision: ShardLeaseRevision(7),
        };
        let registration_error: ShardManagerError = registration_to_proto(
            RegisterAck {
                number_of_shards: 8,
                grant: grant.clone(),
            },
            now,
        )
        .unwrap_err()
        .into();
        let Some(shard_manager_error::Error::Unknown(body)) = registration_error.error else {
            panic!("invalid registration count must be an internal error");
        };
        assert_eq!(body.code, INTERNAL_UNKNOWN);
        assert_eq!(
            body.error,
            format!("RegisterSuccess.executor_count {detail}")
        );

        let renewal_error: ShardLeaseError = lease_to_proto(grant, now).unwrap_err().into();
        let Some(shard_lease_error::Error::Internal(body)) = renewal_error.error else {
            panic!("invalid renewal count must not become LeaseNotFound");
        };
        assert_eq!(body.code, INTERNAL_UNKNOWN);
        assert_eq!(body.error, format!("ShardLease.executor_count {detail}"));
    });
}

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
