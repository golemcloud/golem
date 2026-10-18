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

//! The answer a shard manager replica gives when it is not the leader, shared by the server that
//! sends it and the client that follows it.

use golem_api_grpc::proto::golem::shardmanager::v1::NotLeader;
use golem_common::grpc_uri;
use http::Uri;
use prost::Message;
use serde::{Deserialize, Serialize};
use std::fmt;
use tonic::{Code, Status};

/// Where a shard manager replica serves gRPC, as the other processes reach it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderAddress {
    pub host: String,
    pub port: u16,
}

impl LeaderAddress {
    pub fn uri(&self, tls: bool) -> Uri {
        grpc_uri(&self.host, self.port, tls)
    }
}

/// Whether `host` and `port` form an address that a client can dial: the host becomes the
/// authority of a URI, so a space, a second colon outside an IPv6 literal, a zone such as `%eth0`
/// or a user part would make it a different address or none at all.
///
/// A replica checks the host it advertises with this at startup, and a client names no leader for
/// an answer that fails it. [`LeaderAddress::uri`] cannot be built from a host that fails it.
pub fn valid_leader_address(host: &str, port: u16) -> bool {
    let authority = if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    Uri::builder()
        .scheme("http")
        .authority(format!("{authority}:{port}"))
        .path_and_query("/")
        .build()
        .is_ok_and(|uri| uri.host() == Some(authority.as_str()) && uri.port_u16() == Some(port))
}

impl fmt::Display for LeaderAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.parse::<std::net::Ipv6Addr>().is_ok() {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

const NOT_LEADER_TYPE_URL: &str = "type.googleapis.com/golem.shardmanager.v1.NotLeader";

/// The detail of a refusal that happened before the call ran and names no leader. A type of its
/// own, rather than a `NotLeader` with no host: a reader that looks only for the type of a
/// redirect cannot take it for one, and a client that does not know it sees a plain `UNAVAILABLE`,
/// as it would from any replica that cannot serve.
const REFUSED_BEFORE_RUNNING_TYPE_URL: &str =
    "type.googleapis.com/golem.shardmanager.v1.RefusedBeforeRunning";

/// `google.rpc.Status`, the standard layout of `grpc-status-details-bin`. Using it rather than
/// the bare message lets grpcurl, through the server's reflection service, show the detail.
#[derive(Clone, PartialEq, prost::Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
    #[prost(message, repeated, tag = "3")]
    details: Vec<prost_types::Any>,
}

/// The answer of a standby that knows a ready leader. `UNAVAILABLE`, so that a caller which does
/// not read the detail retries, as it would against any replica that cannot serve.
pub fn not_leader_status(leader: &LeaderAddress) -> Status {
    status_with(
        NotLeader {
            leader_host: leader.host.clone(),
            leader_port: u32::from(leader.port),
        },
        format!("this shard manager is not the leader; the leader is at {leader}"),
    )
}

/// The answer of a replica that refuses a call before running it and cannot name a leader: it is
/// starting or stopping, its belief that it leads has lapsed, or no leader is ready. `UNAVAILABLE`
/// like [`not_leader_status`]. A client that knows this detail sees that the call did not run, so
/// it may go to another replica even when it must not repeat the call.
pub fn refusal_status(message: impl Into<String>) -> Status {
    status_with_detail(
        prost_types::Any {
            type_url: REFUSED_BEFORE_RUNNING_TYPE_URL.to_string(),
            value: Vec::new(),
        },
        message.into(),
    )
}

/// Whether the status is a [`refusal_status`]: the call was refused before it ran.
pub fn refused_before_running(status: &Status) -> bool {
    status.code() == Code::Unavailable
        && RpcStatus::decode(status.details()).is_ok_and(|details| {
            details
                .details
                .iter()
                .any(|any| any.type_url == REFUSED_BEFORE_RUNNING_TYPE_URL)
        })
}

fn status_with(detail: NotLeader, message: String) -> Status {
    status_with_detail(
        prost_types::Any {
            type_url: NOT_LEADER_TYPE_URL.to_string(),
            value: detail.encode_to_vec(),
        },
        message,
    )
}

fn status_with_detail(detail: prost_types::Any, message: String) -> Status {
    let details = RpcStatus {
        code: Code::Unavailable as i32,
        message: message.clone(),
        details: vec![detail],
    };
    Status::with_details(Code::Unavailable, message, details.encode_to_vec().into())
}

/// The leader a status names, if it is a usable "not the leader" answer.
pub fn leader_of(status: &Status) -> Option<LeaderAddress> {
    if status.code() != Code::Unavailable {
        return None;
    }
    let details = RpcStatus::decode(status.details()).ok()?;
    let any = details
        .details
        .into_iter()
        .find(|any| any.type_url == NOT_LEADER_TYPE_URL)?;
    let detail = NotLeader::decode(any.value.as_slice()).ok()?;
    let port = u16::try_from(detail.leader_port)
        .ok()
        .filter(|port| *port != 0)?;
    let host = detail.leader_host.trim();
    // A host that is no URI authority names nobody a client could call, and building the leader's
    // URI from it would panic.
    (!host.is_empty() && valid_leader_address(host, port)).then(|| LeaderAddress {
        host: host.to_string(),
        port,
    })
}

/// For `GrpcClient::with_redirect`.
pub fn is_not_leader(status: &Status) -> bool {
    leader_of(status).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use test_r::test;
    use tonic::{Code, Status};

    fn leader() -> LeaderAddress {
        LeaderAddress {
            host: "10.0.3.7".to_string(),
            port: 9002,
        }
    }

    #[test]
    fn the_leader_address_survives_the_status() {
        assert_eq!(leader_of(&not_leader_status(&leader())), Some(leader()));
    }

    #[test]
    fn an_ipv6_leader_address_survives_the_status() {
        let leader = LeaderAddress {
            host: "fd00::7".to_string(),
            port: 9002,
        };
        assert_eq!(leader_of(&not_leader_status(&leader)), Some(leader.clone()));
        assert_eq!(leader.uri(false).to_string(), "http://[fd00::7]:9002/");
        assert_eq!(leader.to_string(), "[fd00::7]:9002");
    }

    #[test]
    fn the_answer_is_unavailable_and_names_the_leader_for_people() {
        let status = not_leader_status(&leader());
        assert_eq!(status.code(), Code::Unavailable);
        assert!(
            status.message().contains("10.0.3.7:9002"),
            "{}",
            status.message()
        );
    }

    #[test]
    fn a_plain_unavailable_names_no_leader() {
        assert_eq!(
            leader_of(&Status::unavailable("no shard manager leader is ready yet")),
            None
        );
    }

    #[test]
    fn the_detail_on_another_code_names_no_leader() {
        let detail = not_leader_status(&leader()).details().to_vec();
        let status = Status::with_details(Code::Internal, "boom", Bytes::from(detail));
        assert_eq!(leader_of(&status), None);
    }

    #[test]
    fn unreadable_details_name_no_leader() {
        let status =
            Status::with_details(Code::Unavailable, "x", Bytes::from_static(b"\xff\xff\xff"));
        assert_eq!(leader_of(&status), None);
    }

    #[test]
    fn a_detail_without_a_host_or_port_names_no_leader() {
        let status = |host: &str, port| {
            status_with(
                NotLeader {
                    leader_host: host.to_string(),
                    leader_port: port,
                },
                "x".to_string(),
            )
        };
        assert_eq!(leader_of(&status("10.0.3.7", 0)), None);
        assert_eq!(leader_of(&status(" ", 9002)), None);
        assert_eq!(leader_of(&status("h", 70_000)), None);
    }

    #[test]
    fn a_refusal_is_marked_and_names_no_leader() {
        let status = refusal_status("this shard manager is stopping");
        assert_eq!(status.code(), Code::Unavailable);
        assert_eq!(status.message(), "this shard manager is stopping");
        assert!(refused_before_running(&status));
        assert_eq!(leader_of(&status), None);
        assert!(!is_not_leader(&status));
    }

    #[test]
    fn other_answers_are_not_marked_as_refused_before_running() {
        assert!(!refused_before_running(&Status::unavailable("x")));
        assert!(!refused_before_running(&not_leader_status(&leader())));
        let on_another_code = Status::with_details(
            Code::Internal,
            "x",
            Bytes::from(refusal_status("x").details().to_vec()),
        );
        assert!(!refused_before_running(&on_another_code));
        assert!(!refused_before_running(&Status::with_details(
            Code::Unavailable,
            "x",
            Bytes::from_static(b"\xff\xff\xff")
        )));
    }

    #[test]
    fn a_host_that_is_no_uri_authority_names_no_leader() {
        for host in [
            "x # c",
            "shard manager",
            "fe80::1%eth0",
            "fe80::1%25eth0",
            "a:b:c",
            "host:9002",
            "user@host",
            "a/b",
            "a?b",
            "[::1",
        ] {
            let bad = LeaderAddress {
                host: host.to_string(),
                port: 9002,
            };
            assert_eq!(leader_of(&not_leader_status(&bad)), None, "{host:?}");
            assert!(!valid_leader_address(host, 9002), "{host:?}");
        }
    }

    #[test]
    fn usable_hosts_are_valid_leader_addresses() {
        for host in [
            "10.0.3.7",
            "shard-manager-1.golem.svc",
            "fd00::7",
            "::1",
            "localhost",
        ] {
            assert!(valid_leader_address(host, 9002), "{host:?}");
            let address = LeaderAddress {
                host: host.to_string(),
                port: 9002,
            };
            assert_eq!(leader_of(&not_leader_status(&address)), Some(address));
        }
    }
}
