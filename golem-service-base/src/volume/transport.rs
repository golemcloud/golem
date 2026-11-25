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

use super::connection::{ConnectionPolicy, check_address, check_host, destination, protected_port};
use aws_smithy_runtime_api::client::http::{HttpConnector, HttpConnectorFuture};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_types::body::SdkBody;
use futures::future::BoxFuture;
use futures::{StreamExt, TryStreamExt};
use http::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpStream;
use tower::Service;

pub(super) trait Resolve: Send + Sync + std::fmt::Debug {
    fn resolve(&self, host: &str, port: u16) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>>;
}

#[derive(Debug)]
pub(super) struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>> {
        let host: Box<str> = host.trim_start_matches('[').trim_end_matches(']').into();
        Box::pin(async move {
            tokio::net::lookup_host((host.as_ref(), port))
                .await
                .map(Iterator::collect)
        })
    }
}

trait DiscoverInterfaces: Send + Sync + std::fmt::Debug {
    fn discover(&self) -> BoxFuture<'static, io::Result<Box<[IpAddr]>>>;
}

#[derive(Debug)]
struct SystemInterfaces;

impl DiscoverInterfaces for SystemInterfaces {
    fn discover(&self) -> BoxFuture<'static, io::Result<Box<[IpAddr]>>> {
        Box::pin(async {
            let addresses = tokio::task::spawn_blocking(if_addrs::get_if_addrs)
                .await
                .map_err(|_| io::Error::other("volume interface discovery failed"))??;
            Ok(addresses
                .into_iter()
                .map(|interface| interface.ip())
                .collect())
        })
    }
}

#[derive(Debug, Clone)]
struct CheckedConnector {
    policy: Arc<ConnectionPolicy>,
    resolver: Arc<dyn Resolve>,
    interfaces: Arc<dyn DiscoverInterfaces>,
}

impl Service<Uri> for CheckedConnector {
    type Response = TokioIo<TcpStream>;
    type Error = io::Error;
    type Future = BoxFuture<'static, io::Result<Self::Response>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let policy = self.policy.clone();
        let resolver = self.resolver.clone();
        let interfaces = self.interfaces.clone();
        Box::pin(async move {
            tokio::time::timeout(Duration::from_secs(10), async {
                let (host, port) = destination(&uri)?;
                check_host(&policy, host, port).map_err(io::Error::other)?;
                let protected = futures::stream::iter(policy.protected_endpoints.iter())
                    .then(|endpoint| {
                        let resolver = &resolver;
                        async move {
                            let (host, port) = destination(endpoint)?;
                            resolver.resolve(host, port).await
                        }
                    })
                    .try_fold(Vec::new(), |mut all, addresses| async move {
                        all.extend_from_slice(&addresses);
                        Ok(all)
                    })
                    .await?
                    .into_boxed_slice();
                let addresses = resolver.resolve(host, port).await?;
                let local = if protected_port(&policy, port) {
                    let local = interfaces.discover().await?;
                    if local.is_empty() {
                        return Err(io::Error::other(
                            "volume interface discovery returned no addresses",
                        ));
                    }
                    local
                } else {
                    Box::new([])
                };
                addresses.iter().try_for_each(|address| {
                    check_address(&policy, *address, &protected, &local).map_err(io::Error::other)
                })?;
                // Connect to these addresses directly. A second hostname lookup could change
                // the destination after the check.
                TcpStream::connect(addresses.as_ref())
                    .await
                    .map(TokioIo::new)
            })
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "volume connection timed out"))?
        })
    }
}

type GuardedClient = Client<hyper_rustls::HttpsConnector<CheckedConnector>, SdkBody>;

#[derive(Debug, Clone)]
pub(super) struct Transport(GuardedClient);

impl Transport {
    pub(super) fn new(policy: ConnectionPolicy, resolver: Arc<dyn Resolve>) -> io::Result<Self> {
        policy
            .protected_endpoints
            .iter()
            .try_for_each(|uri| destination(uri).map(|_| ()))?;
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .map_err(io::Error::other)?
            .https_or_http()
            .enable_http1()
            .wrap_connector(CheckedConnector {
                policy: Arc::new(policy),
                resolver,
                interfaces: Arc::new(SystemInterfaces),
            });
        let client = Client::builder(TokioExecutor::new())
            .retry_canceled_requests(false)
            .build(connector);
        Ok(Self(client))
    }
}

impl HttpConnector for Transport {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let client = self.0.clone();
        HttpConnectorFuture::new(async move {
            let mut request = request
                .try_into_http1x()
                .map_err(|_| ConnectorError::other("invalid volume request".into(), None))?;
            ["authorization", "x-amz-security-token"]
                .into_iter()
                .for_each(|name| {
                    if let Some(value) = request.headers_mut().get_mut(name) {
                        value.set_sensitive(true);
                    }
                });
            let response = client
                .request(request)
                .await
                .map_err(|_| ConnectorError::other("volume connection failed".into(), None))?;
            // Redirects are refused, not followed. No credential or write is forwarded or resent.
            if response.status().is_redirection() {
                return Err(ConnectorError::other(
                    "volume redirect refused".into(),
                    None,
                ));
            }
            response
                .map(SdkBody::from_body_1_x)
                .try_into()
                .map_err(|_| ConnectorError::other("invalid volume response".into(), None))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::connection::ConnectionRefusal;
    use super::*;
    use golem_common::base_model::address_class::AddressClass;
    use ipnet::IpNet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::{test, timeout};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, oneshot};

    #[derive(Debug)]
    struct DnsLookup {
        host: Box<str>,
        port: u16,
        answer: oneshot::Sender<io::Result<Box<[SocketAddr]>>>,
    }

    #[derive(Debug)]
    struct ControlledDns(mpsc::UnboundedSender<DnsLookup>);

    impl Resolve for ControlledDns {
        fn resolve(
            &self,
            host: &str,
            port: u16,
        ) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>> {
            let (answer, response) = oneshot::channel();
            self.0
                .send(DnsLookup {
                    host: host.into(),
                    port,
                    answer,
                })
                .unwrap();
            Box::pin(async move { response.await.expect("DNS gate must receive an answer") })
        }
    }

    type Observation = oneshot::Sender<io::Result<Box<[IpAddr]>>>;

    #[derive(Debug)]
    struct ControlledInterfaces(mpsc::UnboundedSender<Observation>);

    impl DiscoverInterfaces for ControlledInterfaces {
        fn discover(&self) -> BoxFuture<'static, io::Result<Box<[IpAddr]>>> {
            let (answer, response) = oneshot::channel();
            self.0.send(answer).unwrap();
            Box::pin(async move {
                response
                    .await
                    .expect("interface gate must receive an answer")
            })
        }
    }

    fn controlled_connector(
        protected_endpoints: Box<[Uri]>,
    ) -> (
        CheckedConnector,
        mpsc::UnboundedReceiver<DnsLookup>,
        mpsc::UnboundedReceiver<Observation>,
    ) {
        let (dns, lookups) = mpsc::unbounded_channel();
        let (interfaces, observations) = mpsc::unbounded_channel();
        (
            CheckedConnector {
                policy: Arc::new(ConnectionPolicy {
                    allow_loopback: true,
                    allowed_private_ranges: ["0.0.0.0/0".parse().unwrap()].into(),
                    protected_endpoints,
                }),
                resolver: Arc::new(ControlledDns(dns)),
                interfaces: Arc::new(ControlledInterfaces(interfaces)),
            },
            lookups,
            observations,
        )
    }

    fn lookup(
        lookups: &mut mpsc::UnboundedReceiver<DnsLookup>,
        host: &str,
        port: u16,
    ) -> oneshot::Sender<io::Result<Box<[SocketAddr]>>> {
        let request = lookups.try_recv().expect("the connector must enter DNS");
        assert_eq!(request.host.as_ref(), host);
        assert_eq!(request.port, port);
        request.answer
    }

    fn non_loopback_listener() -> TcpListener {
        let listener = if_addrs::get_if_addrs()
            .expect("interface-observation fixture must enumerate host interfaces")
            .into_iter()
            .map(|interface| interface.ip())
            .filter(|ip| ip.is_ipv4() && matches!(AddressClass::of(*ip), AddressClass::Private | AddressClass::Public))
            .find_map(|ip| std::net::TcpListener::bind((ip, 0)).ok())
            .expect("interface-observation fixture requires a bindable non-loopback IPv4 host interface of class Private or Public");
        listener.set_nonblocking(true).unwrap();
        TcpListener::from_std(listener).unwrap()
    }

    #[test]
    #[timeout("30s")]
    async fn private_connections_require_the_matching_range_and_check_every_answer() {
        let listener = if_addrs::get_if_addrs()
            .expect("private-connection fixture must enumerate host interfaces")
            .into_iter()
            .map(|interface| interface.ip())
            .filter(|ip| ip.is_ipv4() && AddressClass::of(*ip) == AddressClass::Private)
            .find_map(|ip| std::net::TcpListener::bind((ip, 0)).ok())
            .expect("private-connection fixture requires a bindable Private IPv4 host interface");
        listener.set_nonblocking(true).unwrap();
        let listener = TcpListener::from_std(listener).unwrap();
        let address = listener.local_addr().unwrap();
        let IpAddr::V4(ip) = address.ip() else {
            unreachable!()
        };
        let port = address.port();
        let loopback = SocketAddr::from(([127, 0, 0, 1], port));
        let loopback_sink = TcpListener::bind(loopback).await.ok();
        let exact = IpNet::new(ip.into(), 32).unwrap();
        let adjacent = IpNet::new(std::net::Ipv4Addr::from(u32::from(ip) ^ 1).into(), 32).unwrap();
        let subnet = (1..32)
            .rev()
            .map(|prefix| IpNet::new(ip.into(), prefix).unwrap().trunc())
            .find(|network| {
                network.network() != address.ip()
                    && AddressClass::of(network.network()) == AddressClass::Private
            })
            .expect(
                "private-connection fixture must have a non-network address in a private subnet",
            );
        let disjoint = IpNet::new(
            std::net::Ipv4Addr::from(u32::from(ip) ^ (1 << (32 - subnet.prefix_len()))).into(),
            subnet.prefix_len(),
        )
        .unwrap()
        .trunc();
        assert!(subnet.contains(&address.ip()));
        assert_ne!(subnet.network(), address.ip());
        assert!(!disjoint.contains(&address.ip()));
        let cases = [
            (&[][..], &[address][..], Some(ConnectionRefusal::Private)),
            (&[adjacent], &[address], Some(ConnectionRefusal::Private)),
            (&[exact], &[address], None),
            (&[disjoint], &[address], Some(ConnectionRefusal::Private)),
            (&[subnet], &[address], None),
            (
                &[subnet],
                &[address, loopback],
                Some(ConnectionRefusal::Loopback),
            ),
            (
                &[subnet],
                &[loopback, address],
                Some(ConnectionRefusal::Loopback),
            ),
        ];
        futures::stream::iter(cases)
            .enumerate()
            .then(|(case, (allowed_private_ranges, answers, refusal))| {
                let listener = &listener;
                let loopback_sink = &loopback_sink;
                async move {
                    let protected_port = if port == 80 { 81 } else { 80 };
                    let (mut connector, mut dns, mut observations) = controlled_connector(
                        [format!("http://registry.invalid:{protected_port}")
                            .parse()
                            .unwrap()]
                        .into(),
                    );
                    connector.policy = Arc::new(ConnectionPolicy {
                        allowed_private_ranges: allowed_private_ranges.into(),
                        allow_loopback: false,
                        protected_endpoints: connector.policy.protected_endpoints.clone(),
                    });
                    let mut connection = connector
                        .call(format!("http://volume.invalid:{port}/key").parse().unwrap());
                    assert!(futures::poll!(connection.as_mut()).is_pending());
                    lookup(&mut dns, "registry.invalid", protected_port)
                        .send(Ok(
                            [SocketAddr::from(([127, 0, 0, 1], protected_port))].into()
                        ))
                        .unwrap();
                    assert!(futures::poll!(connection.as_mut()).is_pending());
                    lookup(&mut dns, "volume.invalid", port)
                        .send(Ok(answers.into()))
                        .unwrap();
                    match refusal {
                        Some(refusal) => {
                            let error = connection.await.unwrap_err();
                            assert_eq!(
                                error.get_ref().unwrap().downcast_ref::<ConnectionRefusal>(),
                                Some(&refusal),
                                "case {case}"
                            );
                            assert!(
                                tokio::time::timeout(Duration::from_millis(20), listener.accept())
                                    .await
                                    .is_err()
                            );
                        }
                        None => {
                            let connected = connection.await.unwrap();
                            let (accepted, _) =
                                tokio::time::timeout(Duration::from_secs(1), listener.accept())
                                    .await
                                    .unwrap()
                                    .unwrap();
                            drop((connected, accepted));
                        }
                    }
                    if let Some(sink) = loopback_sink {
                        assert!(
                            tokio::time::timeout(Duration::from_millis(20), sink.accept())
                                .await
                                .is_err()
                        );
                    }
                    assert!(dns.try_recv().is_err());
                    assert!(
                        observations.try_recv().is_err(),
                        "unrelated protected port must not require interface discovery"
                    );
                }
            })
            .collect::<()>()
            .await;
    }

    #[test]
    #[timeout("30s")]
    async fn each_connection_uses_a_fresh_interface_observation_in_both_orders() {
        let listener = non_loopback_listener();
        let address = listener.local_addr().unwrap();
        let port = address.port();
        futures::stream::iter([[true, false], [false, true]])
            .then(|order| {
                let listener = &listener;
                async move {
                    let (connector, dns, observations) = controlled_connector(
                        [format!("http://registry.invalid:{port}").parse().unwrap()].into(),
                    );
                    let (_, mut dns, mut observations) = futures::stream::iter(order)
                        .fold(
                            (connector, dns, observations),
                            |(mut connector, mut dns, mut observations), refused| async move {
                                let mut connection = connector.call(
                                    format!("http://volume.invalid:{port}/key").parse().unwrap(),
                                );
                                assert!(futures::poll!(connection.as_mut()).is_pending());
                                lookup(&mut dns, "registry.invalid", port)
                                    .send(Ok([SocketAddr::from(([127, 0, 0, 1], port))].into()))
                                    .unwrap();
                                assert!(futures::poll!(connection.as_mut()).is_pending());
                                lookup(&mut dns, "volume.invalid", port)
                                    .send(Ok([address].into()))
                                    .unwrap();
                                assert!(futures::poll!(connection.as_mut()).is_pending());
                                let observation = observations
                                    .try_recv()
                                    .expect("each connection must request a fresh observation");
                                let observed = if refused {
                                    address.ip()
                                } else {
                                    "127.0.0.7".parse().unwrap()
                                };
                                observation.send(Ok([observed].into())).unwrap();
                                let result = connection.await;
                                if refused {
                                    let error = result.unwrap_err();
                                    assert_eq!(
                                        error
                                            .get_ref()
                                            .unwrap()
                                            .downcast_ref::<ConnectionRefusal>(),
                                        Some(&ConnectionRefusal::GolemEndpoint)
                                    );
                                    assert!(
                                        tokio::time::timeout(
                                            Duration::from_millis(20),
                                            listener.accept()
                                        )
                                        .await
                                        .is_err()
                                    );
                                } else {
                                    let connected = result.unwrap();
                                    let (accepted, _) = listener.accept().await.unwrap();
                                    drop((connected, accepted));
                                }
                                (connector, dns, observations)
                            },
                        )
                        .await;
                    assert!(dns.try_recv().is_err());
                    assert!(observations.try_recv().is_err());
                }
            })
            .collect::<()>()
            .await;
    }

    #[test]
    #[timeout("30s")]
    async fn failed_and_empty_interface_observations_return_their_exact_errors() {
        let cases: [(io::Result<Box<[IpAddr]>>, &str); 2] = [
            (
                Err(io::Error::other("injected interface failure")),
                "injected interface failure",
            ),
            (
                Ok(Box::new([])),
                "volume interface discovery returned no addresses",
            ),
        ];
        futures::stream::iter(cases)
            .then(|(observation, expected)| async move {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let port = address.port();
                let (mut connector, mut dns, mut observations) = controlled_connector(
                    [format!("http://registry.invalid:{port}").parse().unwrap()].into(),
                );
                let mut connection =
                    connector.call(format!("http://volume.invalid:{port}/key").parse().unwrap());
                assert!(futures::poll!(connection.as_mut()).is_pending());
                lookup(&mut dns, "registry.invalid", port)
                    .send(Ok([SocketAddr::from(([127, 0, 0, 2], port))].into()))
                    .unwrap();
                assert!(futures::poll!(connection.as_mut()).is_pending());
                lookup(&mut dns, "volume.invalid", port)
                    .send(Ok([address].into()))
                    .unwrap();
                assert!(futures::poll!(connection.as_mut()).is_pending());
                observations
                    .try_recv()
                    .expect("discovery must be requested")
                    .send(observation)
                    .unwrap();
                let error = connection.await.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::Other);
                assert_eq!(error.to_string(), expected);
                assert!(observations.try_recv().is_err());
                assert!(dns.try_recv().is_err());
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), listener.accept())
                        .await
                        .is_err()
                );
            })
            .collect::<()>()
            .await;
    }

    #[test]
    #[timeout("30s")]
    async fn discovery_waits_for_every_dns_phase_and_dns_errors_stop_it() {
        futures::stream::iter([None, Some(0), Some(1), Some(2)])
            .then(|failure_phase| async move {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let port = address.port();
                let (mut connector, dns, observations) = controlled_connector(
                    [
                        format!("http://registry.invalid:{port}").parse().unwrap(),
                        format!("http://worker.invalid:{port}").parse().unwrap(),
                    ]
                    .into(),
                );
                let connection =
                    connector.call(format!("http://volume.invalid:{port}/key").parse().unwrap());
                let phases = [
                    ("registry.invalid", "first protected DNS failed"),
                    ("worker.invalid", "second protected DNS failed"),
                    ("volume.invalid", "destination DNS failed"),
                ];
                let (mut connection, mut dns, mut observations) =
                    futures::stream::iter(phases)
                        .enumerate()
                        .take(failure_phase.map_or(3, |index| index + 1))
                        .fold(
                            (connection, dns, observations),
                            |(mut connection, mut dns, mut observations),
                             (index, (host, message))| async move {
                                assert!(futures::poll!(connection.as_mut()).is_pending());
                                let answer = lookup(&mut dns, host, port);
                                assert!(
                                    observations.try_recv().is_err(),
                                    "discovery started during {host}"
                                );
                                assert!(futures::poll!(connection.as_mut()).is_pending());
                                assert!(
                                    observations.try_recv().is_err(),
                                    "discovery started while {host} remained gated"
                                );
                                let response = if failure_phase == Some(index) {
                                    Err(io::Error::other(message))
                                } else {
                                    Ok([address].into())
                                };
                                answer.send(response).unwrap();
                                (connection, dns, observations)
                            },
                        )
                        .await;
                let expected = if let Some(index) = failure_phase {
                    phases[index].1
                } else {
                    assert!(futures::poll!(connection.as_mut()).is_pending());
                    observations
                        .try_recv()
                        .expect("discovery must start after all DNS answers")
                        .send(Err(io::Error::other("observation after all DNS")))
                        .unwrap();
                    "observation after all DNS"
                };
                let error = connection.await.unwrap_err();
                assert_eq!(error.to_string(), expected);
                assert!(observations.try_recv().is_err());
                assert!(dns.try_recv().is_err());
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), listener.accept())
                        .await
                        .is_err()
                );
            })
            .collect::<()>()
            .await;
    }

    #[test]
    #[timeout("30s")]
    async fn protected_hostname_refusal_precedes_dns_and_interface_discovery() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (mut connector, mut dns, mut observations) = controlled_connector(
            [format!("http://registry.invalid:{port}").parse().unwrap()].into(),
        );
        let error = connector
            .call(
                format!("http://REGISTRY.INVALID.:{port}/key")
                    .parse()
                    .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.get_ref().unwrap().downcast_ref::<ConnectionRefusal>(),
            Some(&ConnectionRefusal::GolemEndpoint)
        );
        assert!(dns.try_recv().is_err());
        assert!(observations.try_recv().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[derive(Debug)]
    struct FailingInterfaces(AtomicUsize);

    impl DiscoverInterfaces for FailingInterfaces {
        fn discover(&self) -> BoxFuture<'static, io::Result<Box<[IpAddr]>>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Box::pin(async {
                Err(io::Error::other(
                    "unrelated port requested interface discovery",
                ))
            })
        }
    }

    #[test]
    #[timeout("30s")]
    async fn unrelated_port_skips_discovery_but_still_resolves_protected_dns() {
        let protected_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let protected_address = protected_listener.local_addr().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        assert_ne!(address.port(), protected_address.port());
        let interfaces = Arc::new(FailingInterfaces(AtomicUsize::new(0)));
        let (resolver, mut dns) = mpsc::unbounded_channel();
        let mut connector = CheckedConnector {
            policy: Arc::new(ConnectionPolicy {
                allow_loopback: true,
                ..ConnectionPolicy::new(
                    [
                        format!("http://registry.invalid:{}", protected_address.port())
                            .parse()
                            .unwrap(),
                    ]
                    .into(),
                )
            }),
            resolver: Arc::new(ControlledDns(resolver)),
            interfaces: interfaces.clone(),
        };
        let uri: Uri = format!("http://volume.invalid:{}/key", address.port())
            .parse()
            .unwrap();
        let mut connection = connector.call(uri.clone());
        assert!(futures::poll!(connection.as_mut()).is_pending());
        lookup(&mut dns, "registry.invalid", protected_address.port())
            .send(Ok([protected_address].into()))
            .unwrap();
        assert!(futures::poll!(connection.as_mut()).is_pending());
        lookup(&mut dns, "volume.invalid", address.port())
            .send(Ok([address].into()))
            .unwrap();
        let connected = connection.await.unwrap();
        let (accepted, _) = listener.accept().await.unwrap();
        drop((connected, accepted));
        assert_eq!(interfaces.0.load(Ordering::Relaxed), 0);
        assert!(dns.try_recv().is_err());

        let mut connection = connector.call(uri);
        assert!(futures::poll!(connection.as_mut()).is_pending());
        lookup(&mut dns, "registry.invalid", protected_address.port())
            .send(Err(io::Error::other("unrelated-port protected DNS failed")))
            .unwrap();
        assert_eq!(
            connection.await.unwrap_err().to_string(),
            "unrelated-port protected DNS failed"
        );
        assert_eq!(interfaces.0.load(Ordering::Relaxed), 0);
        assert!(dns.try_recv().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), protected_listener.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn discovery_shares_the_dns_deadline_and_cancellation_prevents_a_late_dial() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let listener = non_loopback_listener();
                let address = listener.local_addr().unwrap();
                let port = address.port();
                futures::stream::iter([false, true])
                    .then(|release_before_deadline| {
                        let listener = &listener;
                        async move {
                            let (mut connector, mut dns, mut observations) = controlled_connector(
                                [format!("http://registry.invalid:{port}").parse().unwrap()].into(),
                            );
                            tokio::time::pause();
                            let started = tokio::time::Instant::now();
                            let mut connection = connector.call(format!("http://volume.invalid:{port}/key").parse().unwrap());
                            assert!(futures::poll!(connection.as_mut()).is_pending());
                            let protected = lookup(&mut dns, "registry.invalid", port);
                            assert!(observations.try_recv().is_err());
                            tokio::time::advance(Duration::from_secs(4)).await;
                            protected.send(Ok([SocketAddr::from(([127, 0, 0, 1], port))].into())).unwrap();
                            assert!(futures::poll!(connection.as_mut()).is_pending());
                            let destination = lookup(&mut dns, "volume.invalid", port);
                            assert!(observations.try_recv().is_err());
                            tokio::time::advance(Duration::from_secs(5)).await;
                            destination.send(Ok([address].into())).unwrap();
                            assert!(futures::poll!(connection.as_mut()).is_pending());
                            let observation = observations.try_recv().expect("discovery must start at nine seconds");
                            assert_eq!(started.elapsed(), Duration::from_secs(9));
                            assert!(!observation.is_closed());
                            let permitted = Ok(["127.0.0.7".parse().unwrap()].into());
                            if release_before_deadline {
                                tokio::time::resume();
                                observation.send(permitted).unwrap();
                                let connected = connection.await.unwrap();
                                let (accepted, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await.unwrap().unwrap();
                                drop((connected, accepted));
                            } else {
                                tokio::time::advance(Duration::from_millis(999)).await;
                                assert!(futures::poll!(connection.as_mut()).is_pending(), "the shared deadline must not expire at 9,999 milliseconds");
                                assert!(!observation.is_closed());
                                tokio::time::advance(Duration::from_millis(1)).await;
                                let Poll::Ready(Err(error)) = futures::poll!(connection.as_mut()) else {
                                    panic!("discovery must time out at the total ten-second deadline");
                                };
                                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                                assert_eq!(error.to_string(), "volume connection timed out");
                                assert_eq!(started.elapsed(), Duration::from_secs(10));
                                assert!(observation.is_closed(), "timeout must drop the observation receiver, not detach a dial continuation");
                                assert!(observation.send(permitted).is_err(), "late observation completion must have no receiver");
                                tokio::time::resume();
                                assert!(tokio::time::timeout(Duration::from_millis(20), listener.accept()).await.is_err());
                            }
                            assert!(dns.try_recv().is_err());
                            assert!(observations.try_recv().is_err());
                        }
                    })
                    .collect::<()>()
                    .await;
            });
    }

    #[derive(Debug, Clone, Copy)]
    enum DnsPhase {
        Allowed,
        Refused,
        Mixed,
    }

    #[derive(Debug)]
    struct ChangingDns {
        port: u16,
        answers: AtomicUsize,
        phase: std::sync::Mutex<DnsPhase>,
    }

    fn dns_transition(phase: DnsPhase, port: u16) -> (DnsPhase, Box<[SocketAddr]>) {
        let allowed = SocketAddr::from(([127, 0, 0, 1], port));
        let refused = SocketAddr::from(([169, 254, 169, 254], port));
        match phase {
            DnsPhase::Allowed => (DnsPhase::Refused, [allowed].into()),
            DnsPhase::Refused => (DnsPhase::Allowed, [refused].into()),
            DnsPhase::Mixed => (DnsPhase::Mixed, [allowed, refused].into()),
        }
    }

    impl Resolve for ChangingDns {
        fn resolve(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>> {
            let mut phase = self.phase.lock().unwrap();
            let (next, answers) = dns_transition(*phase, self.port);
            *phase = next;
            self.answers.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move { Ok(answers) })
        }
    }

    #[derive(Debug)]
    struct NeverAnsweringDns;

    impl Resolve for NeverAnsweringDns {
        fn resolve(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>> {
            Box::pin(std::future::pending())
        }
    }

    #[test]
    #[timeout("30s")]
    async fn transport_refuses_redirect_responses_explicitly() {
        futures::stream::iter([302, 307]).then(|status| async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                assert!(socket.read(&mut buffer).await.unwrap() > 0);
                socket.write_all(format!("HTTP/1.1 {status} Redirect\r\nLocation: http://169.254.169.254/\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
            });
            let transport = Transport::new(ConnectionPolicy {
                allow_loopback: true,
                ..ConnectionPolicy::new(Box::new([]))
            }, Arc::new(SystemResolver)).unwrap();
            let error = transport.call(HttpRequest::get(format!("http://{address}/key")).unwrap()).await.unwrap_err();
            assert_eq!(error.into_source().to_string(), "volume redirect refused");
            server.await.unwrap();
        }).collect::<()>().await;
    }

    #[test]
    #[timeout("30s")]
    async fn another_loopback_address_cannot_reach_a_protected_listener() {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let policy = Arc::new(ConnectionPolicy {
            allow_loopback: true,
            allowed_private_ranges: ["0.0.0.0/0".parse().unwrap()].into(),
            protected_endpoints: [format!("http://localhost:{port}").parse().unwrap()].into(),
        });
        let local = if_addrs::get_if_addrs().unwrap();
        futures::stream::iter(
            std::iter::once("127.0.0.2".parse().unwrap()).chain(
                local
                    .into_iter()
                    .map(|interface| interface.ip())
                    .filter(|ip| ip.is_ipv4() && !ip.is_loopback()),
            ),
        )
        .then(|ip| {
            let policy = policy.clone();
            async move {
                let mut connector = CheckedConnector {
                    policy,
                    resolver: Arc::new(SystemResolver),
                    interfaces: Arc::new(SystemInterfaces),
                };
                let error = connector
                    .call(format!("http://{ip}:{port}").parse().unwrap())
                    .await
                    .unwrap_err();
                assert_eq!(
                    error
                        .get_ref()
                        .unwrap()
                        .downcast_ref::<super::super::connection::ConnectionRefusal>(),
                    Some(&super::super::connection::ConnectionRefusal::GolemEndpoint),
                    "{ip}"
                );
            }
        })
        .collect::<()>()
        .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn connection_to_a_never_answering_resolver_times_out_at_ten_seconds() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(async {
                let mut connector = CheckedConnector {
                    policy: Arc::new(ConnectionPolicy::new(Box::new([]))),
                    resolver: Arc::new(NeverAnsweringDns),
                    interfaces: Arc::new(SystemInterfaces),
                };
                let started = tokio::time::Instant::now();
                let mut connection = connector.call("http://volume.invalid/key".parse().unwrap());
                assert!(futures::poll!(connection.as_mut()).is_pending());

                tokio::time::advance(Duration::from_millis(9_999)).await;
                assert!(
                    futures::poll!(connection.as_mut()).is_pending(),
                    "the connection must not time out before ten seconds"
                );

                tokio::time::advance(Duration::from_millis(1)).await;
                let Poll::Ready(Err(error)) = futures::poll!(connection.as_mut()) else {
                    panic!("the connection must fail at exactly ten seconds");
                };
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert_eq!(started.elapsed(), Duration::from_secs(10));
            });
    }

    #[test]
    #[timeout("30s")]
    async fn each_connection_checks_all_dns_answers_and_uses_the_checked_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            assert!(socket.read(&mut buffer).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            listener
        });
        let dns = Arc::new(ChangingDns {
            port,
            answers: AtomicUsize::new(0),
            phase: std::sync::Mutex::new(DnsPhase::Allowed),
        });
        let transport = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                ..ConnectionPolicy::new(Box::new([]))
            },
            dns.clone(),
        )
        .unwrap();
        // This hostname cannot resolve through system DNS. A successful request proves
        // that the transport connects to the checked answer without another lookup.
        let uri = format!("http://volume.invalid:{port}/key").into_boxed_str();
        assert!(
            transport
                .call(HttpRequest::get(&uri).unwrap())
                .await
                .is_ok()
        );
        let listener = server.await.unwrap();
        assert!(
            transport
                .call(HttpRequest::get(&uri).unwrap())
                .await
                .is_err()
        );
        assert_eq!(dns.answers.load(Ordering::Relaxed), 2);
        let mixed = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                ..ConnectionPolicy::new(Box::new([]))
            },
            Arc::new(ChangingDns {
                port,
                answers: AtomicUsize::new(0),
                phase: std::sync::Mutex::new(DnsPhase::Mixed),
            }),
        )
        .unwrap();
        assert!(mixed.call(HttpRequest::get(uri).unwrap()).await.is_err());
        let own = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                protected_endpoints: [format!("http://VOLUME.INVALID.:{port}").parse().unwrap()]
                    .into(),
                ..ConnectionPolicy::new(Box::new([]))
            },
            Arc::new(ChangingDns {
                port,
                answers: AtomicUsize::new(0),
                phase: std::sync::Mutex::new(DnsPhase::Refused),
            }),
        )
        .unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_secs(1),
                own.call(HttpRequest::get(format!("http://volume.invalid:{port}/key")).unwrap())
            )
            .await
            .is_ok_and(|result| result.is_err())
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }
}
