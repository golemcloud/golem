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

//! The address policy of agent sockets, shared by P2 and P3.
//!
//! Connects and outgoing datagrams are checked twice. The host functions check the destination
//! against [`SocketPolicy::refusal`] and then against the network grants, in a recorded durable
//! call, so replay returns the recorded decision. The Wasmtime `socket_addr_check`
//! ([`SocketPolicy::allows`]) is a backstop on every native bind, listen, connect and datagram;
//! it depends only on its arguments and the executor configuration.

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::concurrent::resolve_current_observational_owner;
use crate::durable_host::durability::CustomInvocationContext;
use crate::durable_host::sockets::resolved_names::ResolvedNames;
use crate::services::golem_config::GolemConfig;
use crate::workerctx::WorkerCtx;
use golem_common::base_model::address_class::AddressClass;
use golem_common::model::card::PermissionTarget;
use golem_common::model::card::network_target::socket_target;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;
use wasmtime_wasi::sockets::SocketAddrUse;

#[derive(Debug, Clone)]
pub struct SocketPolicy {
    own_ports: Arc<[u16]>,
}

impl SocketPolicy {
    /// The executor's gRPC and HTTP ports, and the configured `agent_sockets.refused_ports`.
    pub fn new(config: &GolemConfig) -> Self {
        let mut own_ports = [config.grpc.port, config.http_port]
            .into_iter()
            .chain(config.agent_sockets.refused_ports.iter().copied())
            .filter(|port| *port != 0)
            .collect::<Vec<_>>();
        own_ports.sort_unstable();
        own_ports.dedup();
        Self {
            own_ports: own_ports.into(),
        }
    }

    /// Why connects and datagrams to `address` are refused whatever the grants say: cloud
    /// metadata and link-local addresses, and the ports of this executor on an address that
    /// reaches this host.
    pub fn refusal(&self, address: SocketAddr) -> Option<&'static str> {
        let ip = address.ip().to_canonical();
        match AddressClass::of(ip) {
            AddressClass::Metadata => Some("socket destination is a cloud metadata address"),
            AddressClass::LinkLocal => Some("socket destination is a link-local address"),
            _ if self.own_ports.contains(&address.port()) && reaches_this_host(ip) => {
                Some("socket destination is a port of the Golem executor")
            }
            _ => None,
        }
    }

    /// The permission target of a connect or outgoing datagram, with the hostnames that the
    /// Store's lookups linked to the address, or the refusal of [`Self::refusal`]. A public
    /// address of this host's interfaces is marked as this host, so that only a grant that names
    /// it, or an exact hostname linked to it, admits it, like a loopback address.
    pub fn target(
        &self,
        address: SocketAddr,
        resolved_names: &ResolvedNames,
    ) -> Result<PermissionTarget, String> {
        match self.refusal(address) {
            Some(refusal) => Err(refusal.to_string()),
            None => {
                let ip = address.ip().to_canonical();
                let this_host =
                    AddressClass::of(ip) == AddressClass::Public && reaches_this_host(ip);
                Ok(socket_target(
                    address,
                    this_host,
                    resolved_names.hostnames(address.ip()),
                ))
            }
        }
    }

    /// Agents expose no ports: TCP never listens, and a bind takes port 0 or a loopback address.
    pub fn allows(&self, address: SocketAddr, usage: SocketAddrUse) -> bool {
        match usage {
            SocketAddrUse::TcpBind | SocketAddrUse::UdpBind => {
                address.port() == 0
                    || AddressClass::of(address.ip().to_canonical()) == AddressClass::Loopback
            }
            SocketAddrUse::TcpListen | SocketAddrUse::TcpAccept => false,
            SocketAddrUse::TcpConnect | SocketAddrUse::UdpSend => self.refusal(address).is_none(),
            SocketAddrUse::UdpReceive => true,
        }
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// The permission target of a connect or outgoing datagram from this Store to `address`, or
    /// the refusal of [`SocketPolicy::refusal`].
    pub(crate) fn socket_target(&self, address: SocketAddr) -> Result<PermissionTarget, String> {
        SocketPolicy::new(&self.state.config).target(address, &self.state.resolved_names)
    }

    /// Links the addresses that a completed lookup of `name` returned to its hostname. Only a
    /// lookup that replay returns again links: neither a lookup in snapshotting mode, which is not
    /// recorded, nor a physical call of the custom durable invocation `custom_invocation`, which
    /// replay does not claim, does. So live execution, replay and the rebuild after a snapshot
    /// agree on the links.
    pub(crate) fn link_resolved_addresses(
        &mut self,
        custom_invocation: Option<&CustomInvocationContext>,
        name: &str,
        addresses: impl IntoIterator<Item = IpAddr>,
    ) -> Result<(), WorkerExecutorError> {
        if !self.state.snapshotting_mode
            && resolve_current_observational_owner(self, custom_invocation)?.is_none()
        {
            self.state.resolved_names.insert(name, addresses);
        }
        Ok(())
    }
}

/// Loopback, unspecified (which Linux connects to the local host) and the addresses of this
/// host's interfaces, which are the addresses a socket can bind to.
fn reaches_this_host(ip: IpAddr) -> bool {
    ip.is_loopback() || ip.is_unspecified() || UdpSocket::bind((ip, 0)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn policy(own_ports: &[u16]) -> SocketPolicy {
        SocketPolicy {
            own_ports: own_ports.into(),
        }
    }

    fn address(value: &str) -> SocketAddr {
        value.parse().unwrap()
    }

    #[test]
    fn metadata_link_local_and_own_ports_are_refused_before_grants() {
        let policy = policy(&[9000, 8082]);
        for refused in [
            "169.254.169.254:80",
            "[fd00:ec2::254]:80",
            "[::ffff:169.254.169.254]:80",
            "169.254.1.1:5432",
            "[fe80::1]:5432",
            "127.0.0.1:9000",
            "127.1.2.3:8082",
            "0.0.0.0:9000",
            "[::1]:9000",
            "[::]:9000",
            "[::ffff:127.0.0.1]:9000",
        ] {
            assert!(
                policy.refusal(address(refused)).is_some(),
                "{refused} must be refused"
            );
            assert!(
                policy
                    .target(address(refused), &ResolvedNames::default())
                    .is_err()
            );
            for usage in [SocketAddrUse::TcpConnect, SocketAddrUse::UdpSend] {
                assert!(!policy.allows(address(refused), usage), "{refused}");
            }
        }
        for allowed in [
            "127.0.0.1:9001",
            "10.0.0.1:9000",
            "93.184.215.14:9000",
            "[2606:4700::1111]:9000",
        ] {
            assert_eq!(policy.refusal(address(allowed)), None, "{allowed}");
            assert!(policy.allows(address(allowed), SocketAddrUse::TcpConnect));
        }
    }

    #[test]
    fn executor_ports_without_a_configured_port_are_not_refused() {
        assert_eq!(policy(&[]).refusal(address("127.0.0.1:9000")), None);
    }

    #[test]
    fn binds_take_port_zero_or_loopback_and_listen_is_never_allowed() {
        let policy = policy(&[]);
        for usage in [SocketAddrUse::TcpBind, SocketAddrUse::UdpBind] {
            for allowed in [
                "0.0.0.0:0",
                "[::]:0",
                "10.0.0.1:0",
                "127.0.0.1:8080",
                "[::1]:8080",
            ] {
                assert!(policy.allows(address(allowed), usage), "{allowed}");
            }
            for denied in ["0.0.0.0:8080", "[::]:8080", "10.0.0.1:8080"] {
                assert!(!policy.allows(address(denied), usage), "{denied}");
            }
        }
        for listen in ["0.0.0.0:0", "127.0.0.1:8080"] {
            assert!(!policy.allows(address(listen), SocketAddrUse::TcpListen));
        }
    }
}
