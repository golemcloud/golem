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

use super::connection::{ConnectionPolicy, check_address, check_host};
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

#[derive(Debug, Clone)]
struct CheckedConnector {
    policy: Arc<ConnectionPolicy>,
    resolver: Arc<dyn Resolve>,
}

fn destination(uri: &Uri) -> io::Result<(&str, u16)> {
    let port = match uri.scheme_str() {
        Some("http") => 80,
        Some("https") => 443,
        _ => return Err(io::Error::other("invalid volume endpoint scheme")),
    };
    Ok((
        uri.host()
            .ok_or_else(|| io::Error::other("missing volume endpoint host"))?,
        uri.port_u16().unwrap_or(port),
    ))
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
                addresses.iter().try_for_each(|address| {
                    check_address(&policy, *address, &protected).map_err(io::Error::other)
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
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::{test, timeout};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Debug)]
    struct ChangingDns {
        port: u16,
        answers: AtomicUsize,
        mixed: bool,
    }

    fn dns_answers(attempt: usize, port: u16, mixed: bool) -> Box<[SocketAddr]> {
        let allowed = SocketAddr::from(([127, 0, 0, 1], port));
        let refused = SocketAddr::from(([169, 254, 169, 254], port));
        if mixed {
            [allowed, refused].into()
        } else if attempt.is_multiple_of(2) {
            [allowed].into()
        } else {
            [refused].into()
        }
    }

    impl Resolve for ChangingDns {
        fn resolve(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Box<[SocketAddr]>>> {
            let attempt = self.answers.fetch_add(1, Ordering::Relaxed);
            let answers = dns_answers(attempt, self.port, self.mixed);
            Box::pin(async move { Ok(answers) })
        }
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
            mixed: false,
        });
        let transport = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                ..Default::default()
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
                ..Default::default()
            },
            Arc::new(ChangingDns {
                port,
                answers: AtomicUsize::new(0),
                mixed: true,
            }),
        )
        .unwrap();
        assert!(mixed.call(HttpRequest::get(uri).unwrap()).await.is_err());
        let own = Transport::new(
            ConnectionPolicy {
                allow_loopback: true,
                protected_endpoints: [format!("http://VOLUME.INVALID.:{port}").parse().unwrap()]
                    .into(),
                ..Default::default()
            },
            Arc::new(ChangingDns {
                port,
                answers: AtomicUsize::new(1),
                mixed: false,
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
