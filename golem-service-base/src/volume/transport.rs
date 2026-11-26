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

use super::VolumeError;
use super::connection::{
    ConnectionPolicy, ConnectionRefusal, check_address, check_host, destination, protected_port,
};
use aws_smithy_runtime_api::client::http::{HttpConnector, HttpConnectorFuture};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_types::body::SdkBody;
use futures::future::BoxFuture;
use futures::{StreamExt, TryStreamExt};
use http::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::error::Error;
use std::io;
use std::net::SocketAddr;
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

trait Dial: Send + Sync + std::fmt::Debug {
    fn dial(&self, addresses: Box<[SocketAddr]>) -> BoxFuture<'static, io::Result<TcpStream>>;
}

#[derive(Debug)]
struct SystemDial;

impl Dial for SystemDial {
    fn dial(&self, addresses: Box<[SocketAddr]>) -> BoxFuture<'static, io::Result<TcpStream>> {
        Box::pin(async move { TcpStream::connect(addresses.as_ref()).await })
    }
}

#[derive(Debug, Clone)]
struct CheckedConnector {
    policy: Arc<ConnectionPolicy>,
    resolver: Arc<dyn Resolve>,
    dial: Arc<dyn Dial>,
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
        let dial = self.dial.clone();
        Box::pin(async move {
            // This deadline covers DNS and TCP. The HTTPS connector establishes TLS after it.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            tokio::time::timeout_at(deadline, async {
                let (host, port) = destination(&uri)?;
                check_host(&policy, host, port).map_err(io::Error::other)?;
                let protected = futures::stream::iter(policy.protected_endpoints.iter())
                    .map(destination)
                    .try_filter(|(_, protected_port)| {
                        futures::future::ready(*protected_port == port)
                    })
                    .and_then(|(protected_host, protected_port)| {
                        resolver.resolve(protected_host, protected_port)
                    })
                    .try_fold(Vec::new(), |mut protected, addresses| async move {
                        protected.extend_from_slice(&addresses);
                        Ok(protected)
                    })
                    .await?;
                let addresses = resolver.resolve(host, port).await?;
                let local = if protected_port(&policy, port) {
                    let interfaces = if_addrs::get_if_addrs()?;
                    if interfaces.is_empty() {
                        return Err(io::Error::other(
                            "volume interface discovery returned no addresses",
                        ));
                    }
                    interfaces
                        .into_iter()
                        .map(|interface| interface.ip())
                        .collect()
                } else {
                    Vec::new()
                };
                addresses.iter().try_for_each(|address| {
                    check_address(&policy, *address, &protected, &local).map_err(io::Error::other)
                })?;
                // Connect to these addresses directly. A second hostname lookup could change
                // the destination after the check.
                if tokio::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "volume connection timed out",
                    ));
                }
                let stream = dial.dial(addresses).await?;
                if tokio::time::Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "volume connection timed out",
                    ));
                }
                Ok(TokioIo::new(stream))
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
                dial: Arc::new(SystemDial),
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
                .map_err(|_| ConnectorError::user(VolumeError::InvalidRequest.into()))?;
            destination(request.uri())
                .map_err(|_| ConnectorError::user(VolumeError::InvalidRequest.into()))?;
            ["authorization", "x-amz-security-token"]
                .into_iter()
                .for_each(|name| {
                    if let Some(value) = request.headers_mut().get_mut(name) {
                        value.set_sensitive(true);
                    }
                });
            let response = client.request(request).await.map_err(|error| {
                std::iter::successors(Some(&error as &(dyn Error + 'static)), |error| {
                    (*error).source()
                })
                .find_map(|error| {
                    let refusal = error.downcast_ref::<ConnectionRefusal>().or_else(|| {
                        error
                            .downcast_ref::<io::Error>()?
                            .get_ref()?
                            .downcast_ref::<ConnectionRefusal>()
                    });
                    if let Some(refusal) = refusal {
                        Some(ConnectorError::other((*refusal).into(), None).never_connected())
                    } else if error
                        .downcast_ref::<hyper::Error>()
                        .is_some_and(|error| error.is_parse())
                    {
                        Some(ConnectorError::other(
                            VolumeError::InvalidResponse.into(),
                            None,
                        ))
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| ConnectorError::other(VolumeError::Unanswered.into(), None))
            })?;
            // Redirects are refused, not followed. No credential or write is forwarded or resent.
            if response.status().is_redirection() {
                return Err(ConnectorError::other(
                    VolumeError::InvalidResponse.into(),
                    None,
                ));
            }
            response
                .map(SdkBody::from_body_1_x)
                .try_into()
                .map_err(|_| ConnectorError::other(VolumeError::InvalidResponse.into(), None))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipnet::IpNet;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::{test, timeout};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, oneshot};

    const RECORDING_DIAL_ERROR: &str = "recording volume dial";

    #[derive(Debug)]
    struct RecordingDial(mpsc::UnboundedSender<Box<[SocketAddr]>>);

    impl Dial for RecordingDial {
        fn dial(&self, addresses: Box<[SocketAddr]>) -> BoxFuture<'static, io::Result<TcpStream>> {
            self.0.send(addresses).unwrap();
            Box::pin(async { Err(io::Error::other(RECORDING_DIAL_ERROR)) })
        }
    }

    #[derive(Debug)]
    struct FixedDns(Box<[SocketAddr]>);

    impl Resolve for FixedDns {
        fn resolve(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>> {
            let addresses = self.0.clone();
            Box::pin(async move { Ok(addresses) })
        }
    }

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

    fn controlled_connector(
        policy: ConnectionPolicy,
        dial: Arc<dyn Dial>,
    ) -> (CheckedConnector, mpsc::UnboundedReceiver<DnsLookup>) {
        let (dns, lookups) = mpsc::unbounded_channel();
        (
            CheckedConnector {
                policy: Arc::new(policy),
                resolver: Arc::new(ControlledDns(dns)),
                dial,
            },
            lookups,
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

    fn credentialed_request(uri: impl AsRef<str>) -> HttpRequest {
        let mut request = HttpRequest::get(uri.as_ref()).unwrap();
        request
            .headers_mut()
            .insert("authorization", "volume-access volume-secret");
        request
            .headers_mut()
            .insert("x-amz-security-token", "volume-token");
        request
    }

    fn assert_redacted(error: &(dyn Error + 'static)) {
        std::iter::successors(Some(error), |error| (*error).source()).for_each(|error| {
            let text = format!("{error} {error:?}");
            ["volume-access", "volume-secret", "volume-token"]
                .into_iter()
                .for_each(|secret| assert!(!text.contains(secret)));
        });
    }

    async fn read_request(socket: &mut TcpStream) {
        futures::stream::unfold((socket, Vec::new()), |(socket, mut request)| async move {
            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                return None;
            }
            let mut buffer = [0; 4096];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            Some(((), (socket, request)))
        })
        .collect::<()>()
        .await;
    }

    #[test]
    #[timeout("30s")]
    async fn private_connections_require_the_matching_range_and_check_every_answer() {
        let address: SocketAddr = "10.23.45.67:19081".parse().unwrap();
        let second: SocketAddr = "10.23.45.68:19081".parse().unwrap();
        let loopback: SocketAddr = "127.0.0.1:19081".parse().unwrap();
        let exact: IpNet = "10.23.45.67/32".parse().unwrap();
        let adjacent: IpNet = "10.23.45.66/32".parse().unwrap();
        let subnet: IpNet = "10.23.45.0/24".parse().unwrap();
        let disjoint: IpNet = "10.23.46.0/24".parse().unwrap();
        let cases = [
            (&[][..], &[address][..], Some(ConnectionRefusal::Private)),
            (&[adjacent], &[address], Some(ConnectionRefusal::Private)),
            (&[exact], &[address], None),
            (&[disjoint], &[address], Some(ConnectionRefusal::Private)),
            (&[subnet], &[address], None),
            (&[subnet], &[address, second], None),
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
            .for_each(|(allowed_private_ranges, answers, refusal)| async move {
                let (dial, mut entries) = mpsc::unbounded_channel();
                let mut connector = CheckedConnector {
                    policy: Arc::new(ConnectionPolicy {
                        allowed_private_ranges: allowed_private_ranges.into(),
                        ..ConnectionPolicy::new(Box::new([]))
                    }),
                    resolver: Arc::new(FixedDns(answers.into())),
                    dial: Arc::new(RecordingDial(dial)),
                };
                let error = connector
                    .call("http://volume.invalid:19081/key".parse().unwrap())
                    .await
                    .unwrap_err();
                match refusal {
                    Some(refusal) => assert_eq!(
                        error.get_ref().unwrap().downcast_ref::<ConnectionRefusal>(),
                        Some(&refusal)
                    ),
                    None => {
                        assert_eq!(error.to_string(), RECORDING_DIAL_ERROR);
                        assert_eq!(entries.try_recv().unwrap().as_ref(), answers);
                    }
                }
                assert!(entries.try_recv().is_err());
            })
            .await;
    }

    #[test]
    #[timeout("30s")]
    async fn only_matching_protected_ports_require_dns() {
        futures::stream::iter([
            ("http://volume.invalid:19081/key", "registry.invalid", 19081),
            ("https://volume.invalid/key", "worker.invalid", 443),
        ])
        .for_each(|(uri, protected_host, protected_port)| async move {
            let (dial, mut entries) = mpsc::unbounded_channel();
            let (mut connector, mut dns) = controlled_connector(
                ConnectionPolicy::new(
                    [
                        "http://unrelated.invalid:9000".parse().unwrap(),
                        "http://registry.invalid:19081".parse().unwrap(),
                        "https://worker.invalid".parse().unwrap(),
                    ]
                    .into(),
                ),
                Arc::new(RecordingDial(dial)),
            );
            let mut connection = connector.call(uri.parse().unwrap());
            assert!(futures::poll!(connection.as_mut()).is_pending());
            let address = SocketAddr::from(([8, 8, 8, 8], protected_port));
            lookup(&mut dns, protected_host, protected_port)
                .send(Ok([address].into()))
                .unwrap();
            assert!(futures::poll!(connection.as_mut()).is_pending());
            lookup(&mut dns, "volume.invalid", protected_port)
                .send(Ok([address].into()))
                .unwrap();
            let error = connection.await.unwrap_err();
            assert_eq!(
                error.get_ref().unwrap().downcast_ref::<ConnectionRefusal>(),
                Some(&ConnectionRefusal::GolemEndpoint)
            );
            assert!(dns.try_recv().is_err());
            assert!(entries.try_recv().is_err());
        })
        .await;
    }

    #[test]
    #[timeout("30s")]
    async fn unrelated_protected_dns_does_not_block_a_connection() {
        let (dial, mut entries) = mpsc::unbounded_channel();
        let (mut connector, mut dns) = controlled_connector(
            ConnectionPolicy::new(["http://registry.invalid:9000".parse().unwrap()].into()),
            Arc::new(RecordingDial(dial)),
        );
        let mut connection = connector.call("http://volume.invalid:19081/key".parse().unwrap());
        assert!(futures::poll!(connection.as_mut()).is_pending());
        let address = "8.8.8.8:19081".parse().unwrap();
        lookup(&mut dns, "volume.invalid", 19081)
            .send(Ok([address].into()))
            .unwrap();
        assert_eq!(
            connection.await.unwrap_err().to_string(),
            RECORDING_DIAL_ERROR
        );
        assert_eq!(entries.try_recv().unwrap().as_ref(), &[address]);
        assert!(entries.try_recv().is_err());
        assert!(dns.try_recv().is_err());
    }

    #[test]
    #[timeout("30s")]
    async fn protected_hostname_refusal_precedes_dns() {
        let (dial, mut entries) = mpsc::unbounded_channel();
        let (mut connector, mut dns) = controlled_connector(
            ConnectionPolicy::new(["http://registry.invalid:19081".parse().unwrap()].into()),
            Arc::new(RecordingDial(dial)),
        );
        let error = connector
            .call("http://REGISTRY.INVALID.:19081/key".parse().unwrap())
            .await
            .unwrap_err();
        assert_eq!(
            error.get_ref().unwrap().downcast_ref::<ConnectionRefusal>(),
            Some(&ConnectionRefusal::GolemEndpoint)
        );
        assert!(dns.try_recv().is_err());
        assert!(entries.try_recv().is_err());
    }

    #[test]
    #[timeout("30s")]
    async fn sdk_transport_preserves_typed_no_send_refusals() {
        futures::stream::iter([
            ("169.254.169.254:19081", ConnectionRefusal::Metadata),
            ("[fd00:ec2::23]:19081", ConnectionRefusal::Metadata),
            ("169.254.42.7:19081", ConnectionRefusal::LinkLocal),
            ("10.23.45.67:19081", ConnectionRefusal::Private),
            ("127.0.0.1:19081", ConnectionRefusal::Loopback),
            ("0.0.0.0:19081", ConnectionRefusal::Other),
        ])
        .for_each(|(address, refusal)| async move {
            let transport = Transport::new(
                ConnectionPolicy::new(Box::new([])),
                Arc::new(FixedDns([address.parse().unwrap()].into())),
            )
            .unwrap();
            let error = transport
                .call(credentialed_request("http://volume.invalid:19081/key"))
                .await
                .unwrap_err();
            assert_redacted(&error);
            let source = error.into_source();
            assert_eq!(source.downcast_ref::<ConnectionRefusal>(), Some(&refusal));
            assert!(source.to_string().contains("nothing was sent"));
        })
        .await;
        let transport = Transport::new(
            ConnectionPolicy::new(["http://registry.invalid:19081".parse().unwrap()].into()),
            Arc::new(SystemResolver),
        )
        .unwrap();
        let error = transport
            .call(credentialed_request("http://REGISTRY.INVALID.:19081/key"))
            .await
            .unwrap_err();
        assert_redacted(&error);
        let source = error.into_source();
        assert_eq!(
            source.downcast_ref::<ConnectionRefusal>(),
            Some(&ConnectionRefusal::GolemEndpoint)
        );
        assert!(source.to_string().contains("nothing was sent"));
    }

    #[test]
    #[timeout("30s")]
    async fn another_local_address_cannot_reach_a_protected_listener() {
        let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let transport = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                allowed_private_ranges: ["0.0.0.0/0".parse().unwrap(), "::/0".parse().unwrap()]
                    .into(),
                protected_endpoints: [format!("http://localhost:{port}").parse().unwrap()].into(),
            },
            Arc::new(SystemResolver),
        )
        .unwrap();
        let local = if_addrs::get_if_addrs().unwrap();
        let addresses = std::iter::once("127.0.0.2".parse().unwrap()).chain(
            local
                .into_iter()
                .map(|interface| interface.ip())
                .filter(|ip| !ip.is_loopback()),
        );
        futures::stream::iter(addresses)
            .for_each(|ip| {
                let transport = &transport;
                async move {
                    let address = SocketAddr::new(ip, port);
                    let source = transport
                        .call(credentialed_request(format!("http://{address}/key")))
                        .await
                        .unwrap_err()
                        .into_source();
                    assert!(source.is::<ConnectionRefusal>(), "{ip}");
                    assert!(source.to_string().contains("nothing was sent"));
                }
            })
            .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn dns_deadline_cancels_the_lookup_and_prevents_a_late_dial() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(async {
                futures::stream::iter([None, Some(9_999), Some(10_000), Some(10_001)])
                    .for_each(|release_at| async move {
                        let (dial, mut entries) = mpsc::unbounded_channel();
                        let (mut connector, mut dns) = controlled_connector(
                            ConnectionPolicy::new(Box::new([])),
                            Arc::new(RecordingDial(dial)),
                        );
                        let started = tokio::time::Instant::now();
                        let mut connection =
                            connector.call("http://volume.invalid:19081/key".parse().unwrap());
                        assert!(futures::poll!(connection.as_mut()).is_pending());
                        let answer = lookup(&mut dns, "volume.invalid", 19081);
                        tokio::time::advance(Duration::from_millis(9_999)).await;
                        assert!(futures::poll!(connection.as_mut()).is_pending());
                        let addresses = ["8.8.8.8:19081".parse().unwrap()].into();
                        let error = if let Some(release_at) = release_at {
                            tokio::time::advance(Duration::from_millis(release_at - 9_999)).await;
                            answer.send(Ok(addresses)).unwrap();
                            connection.await.unwrap_err()
                        } else {
                            tokio::time::advance(Duration::from_millis(1)).await;
                            let error = connection.await.unwrap_err();
                            assert!(answer.is_closed());
                            assert!(answer.send(Ok(addresses)).is_err());
                            error
                        };
                        if release_at.is_some_and(|at| at < 10_000) {
                            assert_eq!(error.to_string(), RECORDING_DIAL_ERROR);
                            assert_eq!(
                                entries.try_recv().unwrap().as_ref(),
                                &["8.8.8.8:19081".parse::<SocketAddr>().unwrap()]
                            );
                        } else {
                            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                        }
                        assert_eq!(
                            started.elapsed(),
                            Duration::from_millis(release_at.unwrap_or(10_000))
                        );
                        assert!(entries.try_recv().is_err());
                        assert!(dns.try_recv().is_err());
                    })
                    .await;
            });
    }

    #[derive(Debug)]
    struct PendingDial(Arc<AtomicUsize>);

    impl Dial for PendingDial {
        fn dial(&self, _: Box<[SocketAddr]>) -> BoxFuture<'static, io::Result<TcpStream>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Box::pin(std::future::pending())
        }
    }

    #[derive(Debug)]
    struct ControlledDial(Mutex<Option<oneshot::Receiver<TcpStream>>>);

    impl Dial for ControlledDial {
        fn dial(&self, _: Box<[SocketAddr]>) -> BoxFuture<'static, io::Result<TcpStream>> {
            let result = self.0.lock().unwrap().take().unwrap();
            Box::pin(async move { Ok(result.await.unwrap()) })
        }
    }

    #[test]
    fn a_ready_dial_succeeds_only_before_the_absolute_deadline() {
        [9_999, 10_000, 10_001].into_iter().for_each(|millis| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let address = listener.local_addr().unwrap();
                    let (stream, peer) =
                        tokio::join!(TcpStream::connect(address), listener.accept());
                    let stream = stream.unwrap();
                    let (mut peer, _) = peer.unwrap();
                    let (ready, result) = oneshot::channel();
                    let mut connector = CheckedConnector {
                        policy: Arc::new(ConnectionPolicy {
                            allow_loopback: true,
                            ..ConnectionPolicy::new(Box::new([]))
                        }),
                        resolver: Arc::new(FixedDns([address].into())),
                        dial: Arc::new(ControlledDial(Mutex::new(Some(result)))),
                    };
                    tokio::time::pause();
                    let mut connection =
                        connector.call(format!("http://{address}/key").parse().unwrap());
                    assert!(futures::poll!(connection.as_mut()).is_pending());
                    tokio::time::advance(Duration::from_millis(millis)).await;
                    ready.send(stream).unwrap();
                    match connection.await {
                        Ok(stream) => {
                            assert!(millis < 10_000, "accepted a late ready dial at {millis} ms");
                            drop(stream);
                        }
                        Err(error) => {
                            assert!(millis >= 10_000);
                            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                        }
                    }
                    assert_eq!(
                        tokio::time::timeout(Duration::from_secs(1), peer.read(&mut [0]))
                            .await
                            .unwrap()
                            .unwrap(),
                        0
                    );
                });
        });
    }

    #[test]
    fn protected_dns_destination_dns_and_dial_share_the_deadline() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(async {
                let dials = Arc::new(AtomicUsize::new(0));
                let (mut connector, mut dns) = controlled_connector(
                    ConnectionPolicy::new(
                        ["http://registry.invalid:19081".parse().unwrap()].into(),
                    ),
                    Arc::new(PendingDial(dials.clone())),
                );
                let started = tokio::time::Instant::now();
                let mut connection =
                    connector.call("http://volume.invalid:19081/key".parse().unwrap());
                assert!(futures::poll!(connection.as_mut()).is_pending());
                let protected = lookup(&mut dns, "registry.invalid", 19081);
                tokio::time::advance(Duration::from_secs(4)).await;
                protected
                    .send(Ok(["8.8.4.4:19081".parse().unwrap()].into()))
                    .unwrap();
                assert!(futures::poll!(connection.as_mut()).is_pending());
                let destination = lookup(&mut dns, "volume.invalid", 19081);
                tokio::time::advance(Duration::from_secs(5)).await;
                destination
                    .send(Ok(["8.8.8.8:19081".parse().unwrap()].into()))
                    .unwrap();
                assert!(futures::poll!(connection.as_mut()).is_pending());
                assert_eq!(dials.load(Ordering::Relaxed), 1);
                tokio::time::advance(Duration::from_millis(999)).await;
                assert!(futures::poll!(connection.as_mut()).is_pending());
                tokio::time::advance(Duration::from_millis(1)).await;
                let error = connection.await.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert_eq!(started.elapsed(), Duration::from_secs(10));
                assert_eq!(dials.load(Ordering::Relaxed), 1);
                assert!(dns.try_recv().is_err());
            });
    }

    #[test]
    #[timeout("30s")]
    async fn transport_refuses_redirect_responses_without_following_them() {
        futures::stream::iter([301, 302, 303, 307, 308]).for_each(|status| async move {
            let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target_address = target.local_addr().unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_request(&mut socket).await;
                socket.write_all(format!("HTTP/1.1 {status} volume-secret\r\nLocation: http://{target_address}/volume-token\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
            });
            let transport = Transport::new(
                ConnectionPolicy {
                    allow_loopback: true,
                    ..ConnectionPolicy::new(Box::new([]))
                },
                Arc::new(SystemResolver),
            )
            .unwrap();
            let error = transport
                .call(credentialed_request(format!("http://{address}/key")))
                .await
                .unwrap_err();
            assert_redacted(&error);
            assert_eq!(
                error.into_source().downcast_ref::<VolumeError>(),
                Some(&VolumeError::InvalidResponse)
            );
            server.await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(20), target.accept())
                    .await
                    .is_err()
            );
        }).await;
    }

    #[test]
    #[timeout("30s")]
    async fn transport_distinguishes_unanswered_requests_from_invalid_responses() {
        let cases: [(&[u8], VolumeError); 2] = [
            (b"", VolumeError::Unanswered),
            (
                b"HTTP/1.1 200 OK\r\nvolume-secret volume-token\r\n\r\n",
                VolumeError::InvalidResponse,
            ),
        ];
        futures::stream::iter(cases)
            .for_each(|(response, expected)| async move {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let server = tokio::spawn(async move {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    read_request(&mut socket).await;
                    socket.write_all(response).await.unwrap();
                });
                let transport = Transport::new(
                    ConnectionPolicy {
                        allow_loopback: true,
                        ..ConnectionPolicy::new(Box::new([]))
                    },
                    Arc::new(SystemResolver),
                )
                .unwrap();
                let error = transport
                    .call(credentialed_request(format!("http://{address}/key")))
                    .await
                    .unwrap_err();
                assert_redacted(&error);
                let source = error.into_source();
                assert_eq!(source.downcast_ref::<VolumeError>(), Some(&expected));
                assert!(!source.to_string().contains("nothing was sent"));
                server.await.unwrap();
            })
            .await;
    }

    #[test]
    #[timeout("30s")]
    async fn a_failed_dial_is_not_an_address_refusal() {
        let reservation = tokio::net::TcpSocket::new_v4().unwrap();
        reservation.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = reservation.local_addr().unwrap();
        let transport = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                ..ConnectionPolicy::new(Box::new([]))
            },
            Arc::new(SystemResolver),
        )
        .unwrap();
        let error = transport
            .call(credentialed_request(format!("http://{address}/key")))
            .await
            .unwrap_err();
        assert_redacted(&error);
        let source = error.into_source();
        assert_eq!(
            source.downcast_ref::<VolumeError>(),
            Some(&VolumeError::Unanswered)
        );
        assert!(!source.to_string().contains("nothing was sent"));
    }

    #[test]
    #[timeout("30s")]
    async fn dns_failure_text_is_not_exposed_as_a_refusal() {
        let (dns, mut lookups) = mpsc::unbounded_channel();
        let transport = Transport::new(
            ConnectionPolicy::new(Box::new([])),
            Arc::new(ControlledDns(dns)),
        )
        .unwrap();
        let request = tokio::spawn(async move {
            transport
                .call(credentialed_request("http://volume.invalid:19081/key"))
                .await
        });
        lookups
            .recv()
            .await
            .unwrap()
            .answer
            .send(Err(io::Error::other("volume-secret volume-token")))
            .unwrap();
        let error = request.await.unwrap().unwrap_err();
        assert_redacted(&error);
        assert_eq!(
            error.into_source().downcast_ref::<VolumeError>(),
            Some(&VolumeError::Unanswered)
        );
    }

    #[derive(Debug)]
    struct ChangingDns(Mutex<VecDeque<Box<[SocketAddr]>>>);

    impl Resolve for ChangingDns {
        fn resolve(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>> {
            let addresses = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected DNS lookup");
            Box::pin(async move { Ok(addresses) })
        }
    }

    #[test]
    #[timeout("30s")]
    async fn each_connection_checks_all_dns_answers_and_uses_the_checked_address() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            listener
        });
        let refused = SocketAddr::from(([169, 254, 169, 254], address.port()));
        let dns = Arc::new(ChangingDns(Mutex::new(
            [
                [address].into(),
                [refused].into(),
                [address, refused].into(),
                [refused, address].into(),
            ]
            .into(),
        )));
        let transport = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                ..ConnectionPolicy::new(Box::new([]))
            },
            dns.clone(),
        )
        .unwrap();
        // This hostname cannot resolve through system DNS. Success requires direct-IP dialing.
        let uri = format!("http://volume.invalid:{}/key", address.port());
        assert!(
            transport
                .call(HttpRequest::get(&uri).unwrap())
                .await
                .is_ok()
        );
        let listener = server.await.unwrap();
        futures::stream::iter(0..3)
            .then(|_| transport.call(HttpRequest::get(&uri).unwrap()))
            .for_each(|response| async move {
                let source = response.unwrap_err().into_source();
                assert_eq!(
                    source.downcast_ref::<ConnectionRefusal>(),
                    Some(&ConnectionRefusal::Metadata)
                );
            })
            .await;
        assert!(dns.0.lock().unwrap().is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }
}
