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

use crate::custom_api::http_envelope;
use ipnet::{IpNet, Ipv4Net};
use std::sync::Arc;

/// The externally visible scheme and authority of a request on a public listener.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicOrigin {
    pub scheme: String,
    /// The normalized authority. Deployment domains are looked up by this value.
    pub authority: String,
    /// True when the scheme and authority come from the `X-Forwarded-Proto` and
    /// `X-Forwarded-Host` headers of a trusted ingress.
    pub forwarded: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid request origin")]
pub struct InvalidOrigin;

/// Decides which peers may set the public origin of a request with `X-Forwarded-*` headers.
///
/// A peer may set it when its address is in one of the configured address ranges. Requests from
/// other peers use the `Host` header, or the `:authority` of an HTTP/2 request, and ignore every
/// forwarded header. Peers and ranges are compared in canonical form, so an IPv4 ingress that
/// reaches a dual-stack listener as `::ffff:a.b.c.d` matches an IPv4 range, and a configured
/// `::ffff:a.b.c.d/n` range with `n >= 96` matches IPv4 peers.
#[derive(Clone, Debug, Default)]
pub struct TrustedIngress {
    ranges: Arc<[IpNet]>,
}

impl TrustedIngress {
    pub fn new(ranges: &[IpNet]) -> Self {
        Self {
            ranges: ranges.iter().copied().map(canonical_range).collect(),
        }
    }

    pub fn is_trusted(&self, request: &poem::Request) -> bool {
        request
            .remote_addr()
            .as_socket_addr()
            .is_some_and(|address| {
                let ip = address.ip().to_canonical();
                self.ranges.iter().any(|range| range.contains(&ip))
            })
    }

    pub fn public_origin(&self, request: &poem::Request) -> Result<PublicOrigin, InvalidOrigin> {
        let forwarded = self.is_trusted(request);
        let origin =
            http_envelope::origin_from_request(request, forwarded).map_err(|_| InvalidOrigin)?;
        Ok(PublicOrigin {
            scheme: origin.scheme,
            authority: origin.authority,
            forwarded,
        })
    }
}

/// Rewrites a range of IPv4-mapped IPv6 addresses as the IPv4 range it covers, because peers are
/// compared in canonical form.
fn canonical_range(range: IpNet) -> IpNet {
    match range {
        IpNet::V6(v6) if v6.prefix_len() >= 96 => match v6.addr().to_ipv4_mapped() {
            Some(v4) => IpNet::V4(
                Ipv4Net::new(v4, v6.prefix_len() - 96).expect("prefix length is at most 32"),
            ),
            None => range,
        },
        _ => range,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use poem::http::Version;
    use poem::{Body, Request};
    use test_r::test;

    /// Builds a request that arrived from `peer` on a listener.
    pub(crate) fn request_from_peer(
        peer: &str,
        builder: poem::http::request::Builder,
        body: Body,
    ) -> Request {
        let (parts, _) = builder.body(()).unwrap().into_parts();
        let parts = poem::RequestParts::from((
            parts,
            poem::web::LocalAddr(poem::Addr::socket(([127, 0, 0, 1], 9006).into())),
            poem::web::RemoteAddr(poem::Addr::SocketAddr(peer.parse().unwrap())),
            poem::http::uri::Scheme::HTTP,
        ));
        Request::from_parts(parts, body)
    }

    fn from_peer(peer: &str, builder: poem::http::request::Builder) -> Request {
        request_from_peer(peer, builder, Body::empty())
    }

    fn headers() -> poem::http::request::Builder {
        poem::http::Request::builder()
            .header("Host", "worker-service:9006")
            .header("X-Forwarded-Host", "tools.example.com")
            .header("X-Forwarded-Proto", "https")
    }

    fn forwarded(ingress: &TrustedIngress, peer: &str) -> bool {
        ingress
            .public_origin(&from_peer(peer, headers()))
            .unwrap()
            .forwarded
    }

    #[test]
    fn untrusted_peer_cannot_set_the_origin_with_forwarded_headers() {
        let ingress = TrustedIngress::new(&["10.0.0.1/32".parse().unwrap()]);
        let request = from_peer(
            "10.0.0.2:5000",
            poem::http::Request::builder()
                .header("Host", "shop.example.com")
                .header("X-Forwarded-Host", "victim.example.com")
                .header("X-Forwarded-Proto", "https"),
        );

        assert_eq!(
            ingress.public_origin(&request).unwrap(),
            PublicOrigin {
                scheme: "http".to_string(),
                authority: "shop.example.com".to_string(),
                forwarded: false,
            }
        );
    }

    #[test]
    fn trusted_peer_sets_the_origin_with_forwarded_headers() {
        let ingress = TrustedIngress::new(&["10.0.0.1/32".parse().unwrap()]);
        let request = from_peer(
            "10.0.0.1:5000",
            poem::http::Request::builder()
                .header("Host", "worker-service:9006")
                .header("X-Forwarded-Host", "Tools.Example.com")
                .header("X-Forwarded-Proto", "https"),
        );

        assert_eq!(
            ingress.public_origin(&request).unwrap(),
            PublicOrigin {
                scheme: "https".to_string(),
                authority: "tools.example.com".to_string(),
                forwarded: true,
            }
        );
    }

    #[test]
    fn ipv4_ingress_is_trusted_through_a_dual_stack_listener() {
        let mapped_peer = "[::ffff:10.0.0.1]:5000";

        let configured_ipv4 = TrustedIngress::new(&["10.0.0.1/32".parse().unwrap()]);
        assert!(forwarded(&configured_ipv4, mapped_peer));

        let configured_ipv4_range = TrustedIngress::new(&["10.0.0.0/24".parse().unwrap()]);
        assert!(forwarded(&configured_ipv4_range, mapped_peer));

        let configured_mapped = TrustedIngress::new(&["::ffff:10.0.0.1/128".parse().unwrap()]);
        assert!(forwarded(&configured_mapped, "10.0.0.1:5000"));
        assert!(!forwarded(&configured_mapped, "[::ffff:10.0.0.2]:5000"));
    }

    #[test]
    fn peer_in_a_trusted_range_sets_the_origin_with_forwarded_headers() {
        let ingress = TrustedIngress::new(&["172.18.128.0/19".parse().unwrap()]);

        assert!(forwarded(&ingress, "172.18.128.0:5000"));
        assert!(forwarded(&ingress, "172.18.159.255:5000"));
        assert!(!forwarded(&ingress, "172.18.160.0:5000"));
        assert!(!forwarded(&ingress, "172.18.127.255:5000"));
    }

    #[test]
    fn a_range_with_host_bits_trusts_its_whole_network() {
        let ingress = TrustedIngress::new(&["10.0.0.1/8".parse().unwrap()]);

        assert!(forwarded(&ingress, "10.255.0.7:5000"));
        assert!(!forwarded(&ingress, "11.0.0.1:5000"));
    }

    #[test]
    fn ipv6_range_is_trusted() {
        let ingress = TrustedIngress::new(&["fd00:10::/64".parse().unwrap()]);

        assert!(forwarded(&ingress, "[fd00:10::5]:5000"));
        assert!(!forwarded(&ingress, "[fd00:11::5]:5000"));
    }

    #[test]
    fn ipv4_mapped_range_matches_ipv4_peers() {
        let ingress = TrustedIngress::new(&["::ffff:10.0.0.0/120".parse().unwrap()]);

        assert!(forwarded(&ingress, "10.0.0.5:5000"));
        assert!(forwarded(&ingress, "[::ffff:10.0.0.5]:5000"));
        assert!(!forwarded(&ingress, "10.1.0.5:5000"));
    }

    #[test]
    fn ipv6_ranges_outside_the_mapped_block_do_not_match_ipv4_peers() {
        // An IPv6 range with the same low 32 bits as 10.0.0.0/24, but not IPv4-mapped.
        let unique_local = TrustedIngress::new(&["fd00::a00:0/120".parse().unwrap()]);
        assert!(!forwarded(&unique_local, "10.0.0.5:5000"));
        assert!(!forwarded(&unique_local, "[::ffff:10.0.0.5]:5000"));

        // The deprecated IPv4-compatible form ::a.b.c.d is not an IPv4-mapped address.
        let ipv4_compatible = TrustedIngress::new(&["::10.0.0.0/120".parse().unwrap()]);
        assert!(!forwarded(&ipv4_compatible, "10.0.0.5:5000"));
        assert!(!forwarded(&ipv4_compatible, "[::ffff:10.0.0.5]:5000"));

        // A range shorter than /96 stays an IPv6 range, even when it contains the mapped block.
        let wider_than_mapped = TrustedIngress::new(&["::ffff:0:0/95".parse().unwrap()]);
        assert!(!forwarded(&wider_than_mapped, "10.0.0.5:5000"));
        assert!(!forwarded(&wider_than_mapped, "[::ffff:10.0.0.5]:5000"));
    }

    #[test]
    fn http2_request_uses_the_authority_pseudo_header() {
        let ingress = TrustedIngress::default();
        let request = from_peer(
            "10.0.0.2:5000",
            poem::http::Request::builder()
                .version(Version::HTTP_2)
                .uri("http://tools.example.com/mcp")
                .header("X-Forwarded-Host", "victim.example.com"),
        );

        assert_eq!(
            ingress.public_origin(&request).unwrap().authority,
            "tools.example.com"
        );
    }

    #[test]
    fn missing_host_is_invalid() {
        let ingress = TrustedIngress::default();
        let request = from_peer("10.0.0.2:5000", poem::http::Request::builder());

        assert!(ingress.public_origin(&request).is_err());
    }
}
