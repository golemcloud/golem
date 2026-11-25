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
#[derive(Debug)]
pub struct ConnectionPolicy {
    pub allowed_private_ranges: Box<[IpNet]>,
    pub allow_loopback: bool,
    pub protected_endpoints: Box<[http::Uri]>,
}

impl ConnectionPolicy {
    pub fn new(protected_endpoints: Box<[http::Uri]>) -> Self {
        Self {
            allowed_private_ranges: Box::new([]),
            allow_loopback: false,
            protected_endpoints,
        }
    }
}

pub(super) fn destination(uri: &http::Uri) -> std::io::Result<(&str, u16)> {
    let port = match uri.scheme_str() {
        Some("http") => 80,
        Some("https") => 443,
        _ => return Err(std::io::Error::other("invalid volume endpoint scheme")),
    };
    Ok((
        uri.host()
            .ok_or_else(|| std::io::Error::other("missing volume endpoint host"))?,
        uri.port_u16().unwrap_or(port),
    ))
}

pub(super) fn protected_port(policy: &ConnectionPolicy, port: u16) -> bool {
    policy.protected_endpoints.iter().any(|endpoint| {
        destination(endpoint).is_ok_and(|(_, protected_port)| protected_port == port)
    })
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
        destination(endpoint).is_ok_and(|(own_host, own_port)| {
            own_port == port
                && host
                    .trim_end_matches('.')
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
    local_addresses: &[IpAddr],
) -> Result<(), ConnectionRefusal> {
    let ip = canonical(address.ip());
    let class = if ip == IpAddr::V6(std::net::Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x23)) {
        AddressClass::Metadata
    } else {
        AddressClass::of(ip)
    };
    let refusal = match class {
        AddressClass::Metadata => Some(ConnectionRefusal::Metadata),
        AddressClass::LinkLocal => Some(ConnectionRefusal::LinkLocal),
        AddressClass::Other => Some(ConnectionRefusal::Other),
        _ if (protected_port(policy, address.port())
            && (ip.is_loopback() || local_addresses.iter().any(|own| canonical(*own) == ip)))
            || protected
                .iter()
                .any(|own| canonical(own.ip()) == ip && own.port() == address.port()) =>
        {
            Some(ConnectionRefusal::GolemEndpoint)
        }
        AddressClass::Private
            if !policy
                .allowed_private_ranges
                .iter()
                .any(|range| range.contains(&address.ip()) || range.contains(&ip)) =>
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
    fn private_exceptions_cannot_admit_the_eks_host_credential_endpoint() {
        let ranges: [Box<[IpNet]>; 3] = [
            Box::new([]),
            ["::/0".parse().unwrap()].into(),
            ["fd00:ec2::23/128".parse().unwrap()].into(),
        ];
        ranges.into_iter().for_each(|allowed_private_ranges| {
            let policy = ConnectionPolicy {
                allowed_private_ranges,
                ..ConnectionPolicy::new(Box::new([]))
            };
            [
                "[fd00:ec2::23]:80",
                "[FD00:0EC2:0000:0000:0000:0000:0000:0023]:443",
                "[fd00:ec2::23]:9000",
            ]
            .into_iter()
            .for_each(|address| {
                let address = address.parse().unwrap();
                [&[][..], std::slice::from_ref(&address)]
                    .into_iter()
                    .for_each(|protected| {
                        assert_eq!(
                            check_address(&policy, address, protected, &[]),
                            Err(ConnectionRefusal::Metadata),
                            "{address}"
                        );
                    });
            });
        });
        let policy = ConnectionPolicy {
            allowed_private_ranges: ["::/0".parse().unwrap()].into(),
            ..ConnectionPolicy::new(Box::new([]))
        };
        ["[fd00:ec2::22]:80", "[fd00:ec2::24]:443"]
            .into_iter()
            .for_each(|address| {
                assert_eq!(
                    check_address(&policy, address.parse().unwrap(), &[], &[]),
                    Ok(()),
                    "{address}"
                );
            });
    }

    #[test]
    fn private_ranges_match_the_original_or_canonical_address_without_expansion() {
        [
            (
                "::ffff:10.3.2.1/128",
                "[::ffff:10.3.2.1]:9000",
                "[::ffff:10.3.2.2]:9000",
            ),
            ("::a03:201/128", "[::a03:201]:9000", "[::a03:202]:9000"),
            (
                "64:ff9b::a03:201/128",
                "[64:ff9b::a03:201]:9000",
                "[64:ff9b::a03:202]:9000",
            ),
            (
                "2002:a03:201:abcd::7/128",
                "[2002:a03:201:abcd::7]:9000",
                "[2002:a03:201:abcd::8]:9000",
            ),
            (
                "::ffff:10.3.2.0/120",
                "[::ffff:10.3.2.255]:9000",
                "[::ffff:10.3.3.0]:9000",
            ),
            ("::a03:200/120", "[::a03:2ff]:9000", "[::a03:300]:9000"),
            (
                "64:ff9b::a03:200/120",
                "[64:ff9b::a03:2ff]:9000",
                "[64:ff9b::a03:300]:9000",
            ),
            (
                "2002:a03:200::/40",
                "[2002:a03:2ff:abcd::7]:9000",
                "[2002:a03:300:abcd::7]:9000",
            ),
        ]
        .into_iter()
        .for_each(|(range, allowed, refused)| {
            let policy = ConnectionPolicy {
                allowed_private_ranges: [range.parse().unwrap()].into(),
                ..ConnectionPolicy::new(Box::new([]))
            };
            assert_eq!(
                check_address(&policy, allowed.parse().unwrap(), &[], &[]),
                Ok(()),
                "{range}: {allowed}"
            );
            assert_eq!(
                check_address(&policy, refused.parse().unwrap(), &[], &[]),
                Err(ConnectionRefusal::Private),
                "{range}: {refused}"
            );
            assert_eq!(
                check_address(&policy, "10.3.2.1:9000".parse().unwrap(), &[], &[]),
                Err(ConnectionRefusal::Private),
                "an encoded exception must not expand to native IPv4: {range}"
            );
        });
        let native = ConnectionPolicy {
            allowed_private_ranges: ["10.3.2.0/24".parse().unwrap()].into(),
            ..ConnectionPolicy::new(Box::new([]))
        };
        [
            ("[::ffff:10.3.2.255]:9000", "[::ffff:10.3.3.0]:9000"),
            ("[::a03:2ff]:9000", "[::a03:300]:9000"),
            ("[64:ff9b::a03:2ff]:9000", "[64:ff9b::a03:300]:9000"),
            ("[2002:a03:2ff:abcd::7]:9000", "[2002:a03:300:abcd::7]:9000"),
        ]
        .into_iter()
        .for_each(|(allowed, refused)| {
            assert_eq!(
                check_address(&native, allowed.parse().unwrap(), &[], &[]),
                Ok(()),
                "{allowed}"
            );
            assert_eq!(
                check_address(&native, refused.parse().unwrap(), &[], &[]),
                Err(ConnectionRefusal::Private),
                "{refused}"
            );
        });
        let broad = ConnectionPolicy {
            allowed_private_ranges: ["::/0".parse().unwrap()].into(),
            ..ConnectionPolicy::new(Box::new([]))
        };
        [
            ("[::ffff:169.254.169.254]:9000", ConnectionRefusal::Metadata),
            ("[::a9fe:a9fe]:9000", ConnectionRefusal::Metadata),
            ("[64:ff9b::a9fe:a9fe]:9000", ConnectionRefusal::Metadata),
            ("[2002:a9fe:a9fe:abcd::7]:9000", ConnectionRefusal::Metadata),
            ("[::ffff:169.254.42.7]:9000", ConnectionRefusal::LinkLocal),
            ("[64:ff9b::7f00:2]:9000", ConnectionRefusal::Loopback),
            ("[2002:7f00:2:abcd::7]:9000", ConnectionRefusal::Loopback),
        ]
        .into_iter()
        .for_each(|(address, refusal)| {
            assert_eq!(
                check_address(&broad, address.parse().unwrap(), &[], &[]),
                Err(refusal),
                "{address}"
            );
        });
        assert_eq!(
            check_address(
                &broad,
                "[2002:a03:201:abcd::7]:9000".parse().unwrap(),
                &["10.3.2.1:9000".parse().unwrap()],
                &[]
            ),
            Err(ConnectionRefusal::GolemEndpoint)
        );
    }

    #[test]
    fn protected_ports_refuse_loopback_and_local_aliases_not_unrelated_remote_hosts() {
        let policy = ConnectionPolicy {
            allowed_private_ranges: ["0.0.0.0/0".parse().unwrap(), "::/0".parse().unwrap()].into(),
            allow_loopback: true,
            ..ConnectionPolicy::new(
                [
                    "http://localhost:9881".parse().unwrap(),
                    "https://registry.invalid".parse().unwrap(),
                ]
                .into(),
            )
        };
        let local = ["10.20.0.7".parse().unwrap(), "fd12::7".parse().unwrap()];
        let protected = ["10.30.0.5:443".parse().unwrap()];
        [
            "127.0.0.2:9881",
            "127.255.255.254:443",
            "[::1]:9881",
            "[::ffff:127.0.0.2]:9881",
            "[64:ff9b::7f00:2]:443",
            "[2002:7f00:2::]:9881",
            "10.20.0.7:9881",
            "[fd12::7]:443",
            "[::ffff:10.20.0.7]:9881",
            "10.30.0.5:443",
        ]
        .into_iter()
        .for_each(|address| {
            assert_eq!(
                check_address(&policy, address.parse().unwrap(), &protected, &local),
                Err(ConnectionRefusal::GolemEndpoint),
                "{address}"
            );
        });
        [
            "127.0.0.2:9882",
            "[::1]:9882",
            "10.20.0.7:9882",
            "[fd12::7]:9882",
            "10.20.0.8:9881",
            "[fd12::8]:443",
            "10.30.0.5:9881",
            "8.8.8.8:443",
        ]
        .into_iter()
        .for_each(|address| {
            assert_eq!(
                check_address(&policy, address.parse().unwrap(), &protected, &local),
                Ok(()),
                "{address}"
            );
        });
    }

    #[test]
    fn address_policy_fails_closed_and_exceptions_do_not_open_protected_addresses() {
        let default = ConnectionPolicy::new(Box::new([]));
        let permitted = ConnectionPolicy {
            allowed_private_ranges: ["0.0.0.0/0".parse().unwrap(), "::/0".parse().unwrap()].into(),
            allow_loopback: true,
            ..ConnectionPolicy::new(Box::new([]))
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
            assert!(
                check_address(&default, address, &own, &[]).is_err(),
                "{address}"
            );
            assert!(
                check_address(&permitted, address, &own, &[]).is_err(),
                "{address}"
            );
        });
        ["127.0.0.1:80", "[::1]:80", "10.3.2.1:9001", "[fc00::1]:443"]
            .into_iter()
            .for_each(|address| {
                let address = address.parse().unwrap();
                assert!(
                    check_address(&default, address, &own, &[]).is_err(),
                    "{address}"
                );
                assert!(
                    check_address(&permitted, address, &own, &[]).is_ok(),
                    "{address}"
                );
            });
        ["8.8.8.8:443", "100.63.255.255:80", "100.128.0.0:80"]
            .into_iter()
            .for_each(|address| {
                assert!(check_address(&default, address.parse().unwrap(), &own, &[]).is_ok());
            });
        let narrow = ConnectionPolicy {
            allowed_private_ranges: ["10.3.2.0/24".parse().unwrap()].into(),
            ..ConnectionPolicy::new(Box::new([]))
        };
        assert!(check_address(&narrow, "10.3.2.255:80".parse().unwrap(), &[], &[]).is_ok());
        assert!(check_address(&narrow, "10.3.3.0:80".parse().unwrap(), &[], &[]).is_err());
        assert!(check_address(&narrow, "[::ffff:10.3.2.1]:80".parse().unwrap(), &[], &[]).is_ok());
        assert!(check_address(&narrow, "127.0.0.1:80".parse().unwrap(), &[], &[]).is_err());
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
                    check_address(policy, address.parse().unwrap(), &own, &[]),
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
                check_address(&default, address.parse().unwrap(), &own, &[]),
                Err(refusal),
                "{address}"
            );
            assert_eq!(
                check_address(&permitted, address.parse().unwrap(), &own, &[]),
                Ok(()),
                "{address}"
            );
        });
        assert_eq!(
            check_address(&narrow, "10.3.3.0:80".parse().unwrap(), &[], &[]),
            Err(ConnectionRefusal::Private)
        );
        assert_eq!(
            check_address(&narrow, "127.0.0.1:80".parse().unwrap(), &[], &[]),
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
                check_address(&permitted, embedded.parse().unwrap(), &own, &[]),
                Err(ConnectionRefusal::GolemEndpoint)
            );
            assert_eq!(
                check_address(&permitted, own[0], &[embedded.parse().unwrap()], &[]),
                Err(ConnectionRefusal::GolemEndpoint)
            );
        });
    }
}
