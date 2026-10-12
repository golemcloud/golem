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
use std::net::IpAddr;
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
/// Requests from other peers use the `Host` header, or the `:authority` of an HTTP/2 request,
/// and ignore every forwarded header.
#[derive(Clone, Debug, Default)]
pub struct TrustedIngress {
    addresses: Arc<[IpAddr]>,
}

impl TrustedIngress {
    pub fn new(addresses: &[IpAddr]) -> Self {
        Self {
            addresses: addresses.into(),
        }
    }

    pub fn is_trusted(&self, request: &poem::Request) -> bool {
        request
            .remote_addr()
            .as_socket_addr()
            .is_some_and(|address| self.addresses.contains(&address.ip()))
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

    #[test]
    fn untrusted_peer_cannot_set_the_origin_with_forwarded_headers() {
        let ingress = TrustedIngress::new(&["10.0.0.1".parse().unwrap()]);
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
        let ingress = TrustedIngress::new(&["10.0.0.1".parse().unwrap()]);
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
