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

//! HTTP clients for cloud mode that spread the calls over the zones of the gateway.
//!
//! A golem-dev NLB has one address per zone, and each address sends only to the proxy pods
//! in its zone. One reqwest client opens one HTTP/2 connection to a host, so all calls of a
//! benchmark reach one proxy pod. With zone-aware routing, that proxy pod sends them all to
//! the backend pods of its zone. `SpreadClients` has `CLOUD_CONNECTIONS` clients, each
//! connected to one address, and gives the calls to them in turn.

use async_trait::async_trait;
use http::Extensions;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::{Client, ClientBuilder, Request, Response};
use reqwest_middleware::{Middleware, Next};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// The number of clients, and so of connections to each host. A multiple of the three
/// golem-dev zones, so that each zone gets the same number of connections.
pub const CLOUD_CONNECTIONS: usize = 12;

/// The tuned client settings for cloud-mode benchmark connections.
///
/// `http2_prior_knowledge()` is deliberately **not** set. Prior knowledge is for h2c
/// (HTTP/2 over plain HTTP). All cloud endpoints are HTTPS, where HTTP/2 is negotiated
/// through ALPN during the TLS handshake (TLS termination happens at Envoy).
pub fn cloud_client_builder() -> ClientBuilder {
    Client::builder()
        .pool_max_idle_per_host(1024)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .timeout(Duration::from_secs(180))
}

/// Clients that each connect to one address of a host.
pub struct SpreadClients {
    clients: Vec<Client>,
    next: AtomicUsize,
}

impl SpreadClients {
    /// `CLOUD_CONNECTIONS` clients from `cloud_client_builder`.
    pub fn new() -> Self {
        Self::with_builder(CLOUD_CONNECTIONS, cloud_client_builder)
    }

    pub fn with_builder(count: usize, builder: impl Fn() -> ClientBuilder) -> Self {
        Self::from_clients(
            (0..count.max(1))
                .map(|index| {
                    builder()
                        .dns_resolver(OneAddress { index })
                        .build()
                        .expect("Failed to build cloud HTTP client")
                })
                .collect(),
        )
    }

    /// Clients that the caller built; with one client, all calls use it.
    pub fn from_clients(clients: Vec<Client>) -> Self {
        assert!(
            !clients.is_empty(),
            "SpreadClients needs at least one client"
        );
        Self {
            clients,
            next: AtomicUsize::new(0),
        }
    }

    /// The clients in turn.
    pub fn next(&self) -> &Client {
        self.for_index(self.next.fetch_add(1, Ordering::Relaxed))
    }

    /// The client for `index`, for callers that keep one caller on one connection.
    pub fn for_index(&self, index: usize) -> &Client {
        &self.clients[index % self.clients.len()]
    }
}

impl Default for SpreadClients {
    fn default() -> Self {
        Self::new()
    }
}

/// Sends each request with the next client, not with the client of the middleware stack.
/// Add it last, after the middleware that changes the request.
#[async_trait]
impl Middleware for SpreadClients {
    async fn handle(
        &self,
        req: Request,
        _extensions: &mut Extensions,
        _next: Next<'_>,
    ) -> reqwest_middleware::Result<Response> {
        Ok(self.next().execute(req).await?)
    }
}

/// Resolves a host to one of its addresses: the address at `index` mod the number of
/// addresses, in sorted order. The sort makes client `index` connect to the same zone in
/// every run, and the clients spread evenly over the zones.
struct OneAddress {
    index: usize,
}

impl Resolve for OneAddress {
    fn resolve(&self, name: Name) -> Resolving {
        let index = self.index;
        Box::pin(async move {
            // The connector sets the port of the URL on the address.
            let addresses = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            let address = pick(addresses, index)
                .ok_or_else(|| format!("{} has no addresses", name.as_str()))?;
            Ok(Box::new(std::iter::once(address)) as Addrs)
        })
    }
}

fn pick(mut addresses: Vec<SocketAddr>, index: usize) -> Option<SocketAddr> {
    // A dual-stack NLB also has IPv6 addresses; the runner may have no IPv6 route.
    if addresses.iter().any(SocketAddr::is_ipv4) {
        addresses.retain(SocketAddr::is_ipv4);
    }
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty() {
        None
    } else {
        Some(addresses[index % addresses.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn the_clients_spread_evenly_over_the_ipv4_addresses() {
        let zones: Vec<SocketAddr> = ["10.0.3.1:0", "10.0.1.1:0", "10.0.2.1:0"]
            .iter()
            .map(|a| a.parse().unwrap())
            .collect();
        let mut addresses = zones.clone();
        addresses.push("[2600::1]:0".parse().unwrap());
        addresses.push(zones[0]);

        let mut per_zone = [0; 3];
        for index in 0..CLOUD_CONNECTIONS {
            let address = pick(addresses.clone(), index).unwrap();
            assert!(address.is_ipv4(), "an IPv4 address when the host has one");
            per_zone[zones.iter().position(|zone| *zone == address).unwrap()] += 1;
        }
        assert_eq!(per_zone, [CLOUD_CONNECTIONS / 3; 3]);
        assert_eq!(pick(vec![], 0), None);
    }

    /// Each client opens its own connection, and the middleware gives the requests to the
    /// clients in turn: 6 requests over 3 clients open 3 connections, each used twice.
    #[test]
    async fn the_middleware_sends_the_requests_over_all_clients() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let accepted = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = accepted.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = socket.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        let _ = socket
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                            .await;
                    }
                });
            }
        });

        let client = reqwest_middleware::ClientBuilder::new(Client::new())
            .with(SpreadClients::with_builder(3, Client::builder))
            .build();
        for _ in 0..6 {
            let response = client
                .get(format!("http://localhost:{port}/"))
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success());
        }
        assert_eq!(accepted.load(Ordering::SeqCst), 3);
    }
}
