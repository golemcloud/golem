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

//! Classification of destination IP addresses for network grants and egress checks.
//!
//! IPv6 forms that carry an IPv4 address (IPv4-mapped, IPv4-compatible, NAT64 `64:ff9b::/96`
//! and 6to4 `2002::/16`) are classified by the embedded IPv4 address, so an encoded form of an
//! internal address is never `Public`. Every address that is not known to be globally reachable
//! unicast is `Other`, never `Public`.

use std::fmt::{Display, Formatter};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressClass {
    /// `127.0.0.0/8`, `::1`.
    Loopback,
    /// RFC 1918 (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`) and RFC 4193 (`fc00::/7`).
    Private,
    /// `169.254.0.0/16` and `fe80::/10`, except the metadata addresses.
    LinkLocal,
    /// The cloud metadata addresses `169.254.169.254` and `fd00:ec2::254`.
    Metadata,
    /// Globally reachable unicast.
    Public,
    /// Unspecified, multicast, broadcast, documentation, shared (CGNAT), benchmarking and
    /// reserved addresses.
    Other,
}

const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

impl AddressClass {
    pub fn of(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => Self::of_v4(v4),
            IpAddr::V6(v6) => match embedded_ipv4(&v6) {
                Some(v4) => Self::of_v4(v4),
                None => Self::of_v6(v6),
            },
        }
    }

    /// The label value used for this class in metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::LinkLocal => "link_local",
            Self::Metadata => "metadata",
            Self::Public => "public",
            Self::Other => "other",
        }
    }

    fn of_v4(ip: Ipv4Addr) -> Self {
        let bits = ip.to_bits();
        let within = |ranges: &[(Ipv4Addr, u32)]| {
            ranges
                .iter()
                .any(|(network, len)| bits >> (32 - len) == network.to_bits() >> (32 - len))
        };
        if ip == METADATA_V4 {
            Self::Metadata
        } else if within(&[(Ipv4Addr::new(127, 0, 0, 0), 8)]) {
            Self::Loopback
        } else if within(&[(Ipv4Addr::new(169, 254, 0, 0), 16)]) {
            Self::LinkLocal
        } else if within(&V4_PRIVATE) {
            Self::Private
        } else if within(&V4_OTHER) {
            Self::Other
        } else {
            Self::Public
        }
    }

    fn of_v6(ip: Ipv6Addr) -> Self {
        let bits = ip.to_bits();
        let within = |ranges: &[(Ipv6Addr, u32)]| {
            ranges
                .iter()
                .any(|(network, len)| bits >> (128 - len) == network.to_bits() >> (128 - len))
        };
        if ip == METADATA_V6 {
            Self::Metadata
        } else if ip == Ipv6Addr::LOCALHOST {
            Self::Loopback
        } else if within(&[(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10)]) {
            Self::LinkLocal
        } else if within(&[(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7)]) {
            Self::Private
        } else if !within(&[(Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3)]) || within(&V6_OTHER) {
            Self::Other
        } else {
            Self::Public
        }
    }
}

const V4_PRIVATE: [(Ipv4Addr, u32); 3] = [
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
];

const V4_OTHER: [(Ipv4Addr, u32); 9] = [
    // "This network", including the unspecified address
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    // Shared address space (CGNAT)
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    // IETF protocol assignments
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    // Documentation (TEST-NET-1, -2, -3)
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    // Benchmarking
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    // Multicast
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    // Reserved, including the limited broadcast address
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// Ranges inside global unicast (`2000::/3`) that are not globally reachable. Every address
/// outside `2000::/3` that is not loopback, link-local or unique local is `Other` too.
const V6_OTHER: [(Ipv6Addr, u32); 3] = [
    // IETF protocol assignments, including Teredo and benchmarking
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    // Documentation
    (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
];

impl Display for AddressClass {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The IPv4 address carried by an IPv4-mapped (`::ffff:0:0/96`), IPv4-compatible (`::/96`,
/// except `::` and `::1`), NAT64 (`64:ff9b::/96`) or 6to4 (`2002::/16`) address.
pub fn embedded_ipv4(ip: &Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let low =
        |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    match s {
        [0, 0, 0, 0, 0, 0xffff, hi, lo] => Some(low(hi, lo)),
        [0, 0, 0, 0, 0, 0, 0, 0 | 1] => None,
        [0, 0, 0, 0, 0, 0, hi, lo] => Some(low(hi, lo)),
        [0x0064, 0xff9b, 0, 0, 0, 0, hi, lo] => Some(low(hi, lo)),
        [0x2002, hi, lo, ..] => Some(low(hi, lo)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::AddressClass::{self, *};
    use test_r::test;

    fn assert_classes(cases: &[(&str, AddressClass)]) {
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(address, expected)| {
                let actual = AddressClass::of(address.parse().unwrap());
                (actual != *expected)
                    .then(|| format!("{address}: {actual:?}, expected {expected:?}"))
            })
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn ipv4_range_boundaries() {
        assert_classes(&[
            ("0.0.0.0", Other),
            ("0.255.255.255", Other),
            ("1.0.0.0", Public),
            ("8.8.8.8", Public),
            ("9.255.255.255", Public),
            ("10.0.0.0", Private),
            ("10.255.255.255", Private),
            ("11.0.0.0", Public),
            ("100.63.255.255", Public),
            ("100.64.0.0", Other),
            ("100.127.255.255", Other),
            ("100.128.0.0", Public),
            ("126.255.255.255", Public),
            ("127.0.0.0", Loopback),
            ("127.0.0.1", Loopback),
            ("127.255.255.255", Loopback),
            ("128.0.0.0", Public),
            ("169.253.255.255", Public),
            ("169.254.0.0", LinkLocal),
            ("169.254.169.253", LinkLocal),
            ("169.254.169.254", Metadata),
            ("169.254.169.255", LinkLocal),
            ("169.254.255.255", LinkLocal),
            ("169.255.0.0", Public),
            ("172.15.255.255", Public),
            ("172.16.0.0", Private),
            ("172.31.255.255", Private),
            ("172.32.0.0", Public),
            ("191.255.255.255", Public),
            ("192.0.0.0", Other),
            ("192.0.0.255", Other),
            ("192.0.1.0", Public),
            ("192.0.1.255", Public),
            ("192.0.2.0", Other),
            ("192.0.2.255", Other),
            ("192.0.3.0", Public),
            ("192.167.255.255", Public),
            ("192.168.0.0", Private),
            ("192.168.255.255", Private),
            ("192.169.0.0", Public),
            ("198.17.255.255", Public),
            ("198.18.0.0", Other),
            ("198.19.255.255", Other),
            ("198.20.0.0", Public),
            ("198.51.99.255", Public),
            ("198.51.100.0", Other),
            ("198.51.100.255", Other),
            ("198.51.101.0", Public),
            ("203.0.112.255", Public),
            ("203.0.113.0", Other),
            ("203.0.113.255", Other),
            ("203.0.114.0", Public),
            ("223.255.255.255", Public),
            ("224.0.0.0", Other),
            ("239.255.255.255", Other),
            ("240.0.0.0", Other),
            ("255.255.255.254", Other),
            ("255.255.255.255", Other),
        ]);
    }

    #[test]
    fn ipv6_range_boundaries() {
        assert_classes(&[
            ("::", Other),
            ("::1", Loopback),
            ("100::1", Other),
            ("1fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Other),
            ("2000::", Public),
            ("2001::1", Other),
            ("2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff", Other),
            ("2001:200::", Public),
            ("2001:db7:ffff:ffff:ffff:ffff:ffff:ffff", Public),
            ("2001:db8::", Other),
            ("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff", Other),
            ("2001:db9::", Public),
            ("2606:4700:4700::1111", Public),
            ("3ffe:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Public),
            ("3fff::", Other),
            ("3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff", Other),
            ("3fff:1000::", Public),
            ("4000::", Other),
            ("5f00::1", Other),
            ("fbff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Other),
            ("fc00::", Private),
            ("fd00:ec2::253", Private),
            ("fd00:ec2::254", Metadata),
            ("fd00:ec2::255", Private),
            ("fd00:ec3::254", Private),
            ("fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Private),
            ("fe00::", Other),
            ("fe7f:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Other),
            ("fe80::", LinkLocal),
            ("fe80::1", LinkLocal),
            ("febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff", LinkLocal),
            ("fec0::", Other),
            ("ff00::", Other),
            ("ff02::1", Other),
        ]);
    }

    #[test]
    fn ipv4_mapped_addresses_use_the_embedded_address() {
        assert_classes(&[
            ("::ffff:127.0.0.1", Loopback),
            ("::ffff:10.0.0.1", Private),
            ("::ffff:172.15.255.255", Public),
            ("::ffff:172.16.0.0", Private),
            ("::ffff:169.254.1.1", LinkLocal),
            ("::ffff:169.254.169.254", Metadata),
            ("::ffff:0.0.0.0", Other),
            ("::ffff:8.8.8.8", Public),
        ]);
    }

    #[test]
    fn ipv4_compatible_addresses_use_the_embedded_address() {
        assert_classes(&[
            ("::0.0.0.2", Other),
            ("::127.0.0.1", Loopback),
            ("::10.0.0.1", Private),
            ("::169.254.169.254", Metadata),
            ("::169.254.0.1", LinkLocal),
            ("::8.8.8.8", Public),
        ]);
    }

    #[test]
    fn nat64_addresses_use_the_embedded_address() {
        assert_classes(&[
            ("64:ff9b::127.0.0.1", Loopback),
            ("64:ff9b::a00:1", Private),
            ("64:ff9b::192.168.0.1", Private),
            ("64:ff9b::a9fe:101", LinkLocal),
            ("64:ff9b::169.254.169.254", Metadata),
            ("64:ff9b::8.8.8.8", Public),
            // Outside 64:ff9b::/96: not decoded, and not public.
            ("64:ff9b::1:a00:1", Other),
            ("64:ff9b:1::a00:1", Other),
        ]);
    }

    #[test]
    fn six_to_four_addresses_use_the_embedded_address() {
        assert_classes(&[
            ("2002:7f00:1::", Loopback),
            ("2002:a00:1::", Private),
            ("2002:a00:1:1234:5678::1", Private),
            ("2002:ac0f:ffff::1", Public),
            ("2002:ac10::1", Private),
            ("2002:a9fe:101::", LinkLocal),
            ("2002:a9fe:a9fe::", Metadata),
            ("2002:c000:201::", Other),
            ("2002:808:808::", Public),
        ]);
    }

    #[test]
    fn metric_labels() {
        let labels: Vec<&str> = [Loopback, Private, LinkLocal, Metadata, Public, Other]
            .iter()
            .map(|class| class.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "loopback",
                "private",
                "link_local",
                "metadata",
                "public",
                "other"
            ]
        );
    }
}
