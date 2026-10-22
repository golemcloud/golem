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

//! The TLS side of a gRPC server. tonic builds one fixed rustls configuration, so this does the
//! handshake itself, with the configuration that is current when the connection arrives, and
//! hands tonic finished TLS streams.

use super::material::{TLS_RELOAD_INTERVAL, TlsLoaded, TlsMaterial, TlsSources};
use super::pem::TlsError;
use crate::grpc::server::GrpcServerTlsConfig;
use futures::{Stream, StreamExt};
use rustls::RootCertStore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::server::{Connected, TcpConnectInfo};
use tracing::{debug, error};

/// How long one TLS handshake may take before the connection is dropped.
pub const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many finished handshakes wait for tonic to take them.
const ACCEPTED_QUEUE: usize = 256;

pub struct GrpcServerTls {
    material: Arc<TlsMaterial>,
    config: Mutex<(Arc<TlsLoaded>, Arc<rustls::ServerConfig>)>,
}

impl GrpcServerTls {
    /// `None` when TLS is disabled. Loads and validates the material (a bad item fails the
    /// start), builds the first configuration and starts the file reloader.
    pub fn from_config(
        config: &GrpcServerTlsConfig,
        use_name: &str,
    ) -> anyhow::Result<Option<Arc<Self>>> {
        match config {
            GrpcServerTlsConfig::Disabled(_) => Ok(None),
            GrpcServerTlsConfig::Enabled(enabled) => {
                let sources = enabled.sources()?;
                Ok(Some(Self::from_sources(
                    use_name,
                    sources,
                    TLS_RELOAD_INTERVAL,
                )?))
            }
        }
    }

    #[doc(hidden)]
    pub fn from_sources(
        use_name: &str,
        sources: TlsSources,
        reload_interval: Duration,
    ) -> Result<Arc<Self>, TlsError> {
        let material = TlsMaterial::load(use_name, sources)?;
        let loaded = material.current();
        let config = build_server_config(&loaded).map_err(|reason| TlsError::BadCertificate {
            source: use_name.to_string(),
            reason,
        })?;
        material.start_reloader(reload_interval);
        Ok(Arc::new(Self {
            material,
            config: Mutex::new((loaded, Arc::new(config))),
        }))
    }

    pub fn material(&self) -> &Arc<TlsMaterial> {
        &self.material
    }

    /// The configuration for the material that is current now. It is rebuilt only when the
    /// material changed. If a rebuild fails, the previous configuration stays in use.
    pub fn current_config(&self) -> Arc<rustls::ServerConfig> {
        let loaded = self.material.current();
        let mut guard = self.config.lock().unwrap();
        if !Arc::ptr_eq(&guard.0, &loaded) {
            match build_server_config(&loaded) {
                Ok(config) => *guard = (loaded, Arc::new(config)),
                Err(reason) => {
                    // Remember the material, so one bad set is reported once.
                    error!(
                        r#use = %self.material.name(),
                        error = %reason,
                        "The reloaded TLS material cannot build a server configuration; keeping the previous one"
                    );
                    guard.0 = loaded;
                }
            }
        }
        guard.1.clone()
    }

    /// Wraps accepted TCP connections. With `tls`, each connection gets a TLS handshake with the
    /// configuration current at that moment. Handshakes run concurrently, each limited by
    /// `TLS_HANDSHAKE_TIMEOUT`; a failed one is logged at debug level and dropped, and never ends
    /// the stream. Without `tls`, streams pass through unchanged.
    pub fn incoming<S>(
        tls: Option<&Arc<Self>>,
        incoming: S,
    ) -> impl Stream<Item = io::Result<MaybeTlsStream>> + Send + 'static
    where
        S: Stream<Item = io::Result<TcpStream>> + Send + 'static,
    {
        match tls {
            None => futures::future::Either::Left(incoming.map(|r| r.map(MaybeTlsStream::Plain))),
            Some(tls) => {
                let (tx, rx) = tokio::sync::mpsc::channel(ACCEPTED_QUEUE);
                tokio::spawn(accept_loop(tls.clone(), incoming, tx));
                futures::future::Either::Right(ReceiverStream::new(rx))
            }
        }
    }
}

async fn accept_loop<S>(
    tls: Arc<GrpcServerTls>,
    incoming: S,
    tx: tokio::sync::mpsc::Sender<io::Result<MaybeTlsStream>>,
) where
    S: Stream<Item = io::Result<TcpStream>> + Send + 'static,
{
    let mut incoming = Box::pin(incoming);
    let mut handshakes: JoinSet<io::Result<tokio_rustls::server::TlsStream<TcpStream>>> =
        JoinSet::new();
    let mut incoming_ended = false;
    loop {
        if incoming_ended && handshakes.is_empty() {
            break;
        }
        tokio::select! {
            _ = tx.closed() => break,
            next = incoming.next(), if !incoming_ended => match next {
                Some(Ok(tcp)) => {
                    let acceptor = tokio_rustls::TlsAcceptor::from(tls.current_config());
                    handshakes.spawn(async move {
                        match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                            Ok(result) => result,
                            Err(_) => Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "TLS handshake timed out",
                            )),
                        }
                    });
                }
                Some(Err(error)) => {
                    if tx.send(Err(error)).await.is_err() {
                        break;
                    }
                }
                None => incoming_ended = true,
            },
            Some(done) = handshakes.join_next(), if !handshakes.is_empty() => match done {
                Ok(Ok(stream)) => {
                    if tx.send(Ok(MaybeTlsStream::Tls(Box::new(stream)))).await.is_err() {
                        break;
                    }
                }
                Ok(Err(error)) => debug!("TLS handshake failed: {error}"),
                Err(error) => debug!("TLS handshake task failed: {error}"),
            },
        }
    }
}

fn build_server_config(loaded: &TlsLoaded) -> Result<rustls::ServerConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(
        CertificateDer::pem_slice_iter(&loaded.ca_pem).filter_map(Result::ok),
    );
    // The client certificate is required; there is no switch that turns this off.
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|e| e.to_string())?;
    let certs = CertificateDer::pem_slice_iter(&loaded.cert_pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let key = PrivateKeyDer::from_pem_slice(&loaded.key_pem).map_err(|e| e.to_string())?;
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

pub enum MaybeTlsStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for MaybeTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Tls(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(s) => s.is_write_vectored(),
            Self::Tls(s) => s.is_write_vectored(),
        }
    }
}

impl Connected for MaybeTlsStream {
    type ConnectInfo = TcpConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        match self {
            Self::Plain(s) => s.connect_info(),
            Self::Tls(s) => s.get_ref().0.connect_info(),
        }
    }
}
