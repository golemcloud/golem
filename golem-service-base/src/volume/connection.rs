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

use golem_common::base_model::address_class::{AddressClass, embedded_ipv4};
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};

/// Installation restrictions on volume connections. Private and loopback addresses are refused
/// by default. Protected endpoints are checked even when a private range or loopback is allowed.
#[derive(Debug, Default)]
pub struct ConnectionPolicy {
    pub allowed_private_ranges: Box<[IpNet]>,
    pub allow_loopback: bool,
    pub protected_endpoints: Box<[http::Uri]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConnectionRefusal {
    #[error("volume destination is cloud metadata")]
    Metadata,
    #[error("volume destination is link-local")]
    LinkLocal,
    #[error("volume destination is a Golem service endpoint")]
    GolemEndpoint,
    #[error("volume destination is a private address outside the allowed ranges")]
    Private,
    #[error("volume destination is loopback")]
    Loopback,
    #[error("volume destination is not a supported unicast address")]
    Other,
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => embedded_ipv4(&ip).map(IpAddr::V4).unwrap_or(IpAddr::V6(ip)),
        ip => ip,
    }
}

pub(super) fn check_host(
    policy: &ConnectionPolicy,
    host: &str,
    port: u16,
) -> Result<(), ConnectionRefusal> {
    if policy.protected_endpoints.iter().any(|endpoint| {
        let own_port = endpoint.port_u16().unwrap_or_else(|| {
            if endpoint.scheme_str() == Some("https") {
                443
            } else {
                80
            }
        });
        own_port == port
            && endpoint.host().is_some_and(|own_host| {
                host.trim_end_matches('.')
                    .eq_ignore_ascii_case(own_host.trim_end_matches('.'))
            })
    }) {
        Err(ConnectionRefusal::GolemEndpoint)
    } else {
        Ok(())
    }
}

/// Checks one resolved address. An exception admits only private or loopback addresses, never
/// metadata, link-local, protected endpoints, or addresses of class `Other`.
pub fn check_address(
    policy: &ConnectionPolicy,
    address: SocketAddr,
    protected: &[SocketAddr],
) -> Result<(), ConnectionRefusal> {
    let ip = canonical(address.ip());
    let refusal = match AddressClass::of(ip) {
        AddressClass::Metadata => Some(ConnectionRefusal::Metadata),
        AddressClass::LinkLocal => Some(ConnectionRefusal::LinkLocal),
        AddressClass::Other => Some(ConnectionRefusal::Other),
        _ if protected
            .iter()
            .any(|own| canonical(own.ip()) == ip && own.port() == address.port()) =>
        {
            Some(ConnectionRefusal::GolemEndpoint)
        }
        AddressClass::Private
            if !policy
                .allowed_private_ranges
                .iter()
                .any(|range| range.contains(&ip)) =>
        {
            Some(ConnectionRefusal::Private)
        }
        AddressClass::Loopback if !policy.allow_loopback => Some(ConnectionRefusal::Loopback),
        _ => None,
    };
    refusal.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn address_policy_fails_closed_and_exceptions_do_not_open_protected_addresses() {
        let default = ConnectionPolicy::default();
        let permitted = ConnectionPolicy {
            allowed_private_ranges: ["0.0.0.0/0".parse().unwrap(), "::/0".parse().unwrap()].into(),
            allow_loopback: true,
            ..Default::default()
        };
        let own = ["10.3.2.1:9000".parse().unwrap()];
        [
            "169.254.169.254:80",
            "[fd00:ec2::254]:80",
            "169.254.2.3:443",
            "[fe80::1]:443",
            "0.0.0.0:80",
            "100.64.0.0:80",
            "100.127.255.255:80",
            "224.0.0.1:80",
            "[::]:80",
            "[::ffff:100.64.1.1]:80",
            "[64:ff9b::169.254.169.254]:80",
            "[2002:a9fe:a9fe::]:80",
            "10.3.2.1:9000",
        ]
        .into_iter()
        .for_each(|address| {
            let address = address.parse().unwrap();
            assert!(check_address(&default, address, &own).is_err(), "{address}");
            assert!(
                check_address(&permitted, address, &own).is_err(),
                "{address}"
            );
        });
        ["127.0.0.1:80", "[::1]:80", "10.3.2.1:9001", "[fc00::1]:443"]
            .into_iter()
            .for_each(|address| {
                let address = address.parse().unwrap();
                assert!(check_address(&default, address, &own).is_err(), "{address}");
                assert!(
                    check_address(&permitted, address, &own).is_ok(),
                    "{address}"
                );
            });
        ["8.8.8.8:443", "100.63.255.255:80", "100.128.0.0:80"]
            .into_iter()
            .for_each(|address| {
                assert!(check_address(&default, address.parse().unwrap(), &own).is_ok());
            });
        let narrow = ConnectionPolicy {
            allowed_private_ranges: ["10.3.2.0/24".parse().unwrap()].into(),
            ..Default::default()
        };
        assert!(check_address(&narrow, "10.3.2.255:80".parse().unwrap(), &[]).is_ok());
        assert!(check_address(&narrow, "10.3.3.0:80".parse().unwrap(), &[]).is_err());
        assert!(check_address(&narrow, "[::ffff:10.3.2.1]:80".parse().unwrap(), &[]).is_ok());
        assert!(check_address(&narrow, "127.0.0.1:80".parse().unwrap(), &[]).is_err());
        [
            ("169.254.169.254:80", ConnectionRefusal::Metadata),
            ("[fd00:ec2::254]:80", ConnectionRefusal::Metadata),
            ("[64:ff9b::169.254.169.254]:80", ConnectionRefusal::Metadata),
            ("169.254.2.3:443", ConnectionRefusal::LinkLocal),
            ("[fe80::1]:443", ConnectionRefusal::LinkLocal),
            ("100.64.0.0:80", ConnectionRefusal::Other),
            ("[::]:80", ConnectionRefusal::Other),
            ("10.3.2.1:9000", ConnectionRefusal::GolemEndpoint),
        ]
        .into_iter()
        .for_each(|(address, refusal)| {
            [&default, &permitted].into_iter().for_each(|policy| {
                assert_eq!(
                    check_address(policy, address.parse().unwrap(), &own),
                    Err(refusal),
                    "{address}"
                );
            });
        });
        [
            ("127.0.0.1:80", ConnectionRefusal::Loopback),
            ("[::1]:80", ConnectionRefusal::Loopback),
            ("10.3.2.1:9001", ConnectionRefusal::Private),
            ("[fc00::1]:443", ConnectionRefusal::Private),
        ]
        .into_iter()
        .for_each(|(address, refusal)| {
            assert_eq!(
                check_address(&default, address.parse().unwrap(), &own),
                Err(refusal),
                "{address}"
            );
            assert_eq!(
                check_address(&permitted, address.parse().unwrap(), &own),
                Ok(()),
                "{address}"
            );
        });
        assert_eq!(
            check_address(&narrow, "10.3.3.0:80".parse().unwrap(), &[]),
            Err(ConnectionRefusal::Private)
        );
        assert_eq!(
            check_address(&narrow, "127.0.0.1:80".parse().unwrap(), &[]),
            Err(ConnectionRefusal::Loopback)
        );
        [
            "[::ffff:10.3.2.1]:9000",
            "[64:ff9b::a03:201]:9000",
            "[2002:a03:201::]:9000",
        ]
        .into_iter()
        .for_each(|embedded| {
            assert_eq!(
                check_address(&permitted, embedded.parse().unwrap(), &own),
                Err(ConnectionRefusal::GolemEndpoint)
            );
            assert_eq!(
                check_address(&permitted, own[0], &[embedded.parse().unwrap()]),
                Err(ConnectionRefusal::GolemEndpoint)
            );
        });
    }
}
