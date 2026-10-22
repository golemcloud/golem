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

use golem_service_base::tls::material::{ReloadOutcome, TlsSources};
use golem_service_base::tls::pem::PemSource;
use golem_service_base::tls::server::GrpcServerTls;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, ServerName};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use test_r::test;
use tokio::net::{TcpListener, TcpStream};
use tonic::transport::{Certificate, ClientTlsConfig, Identity};
use tonic_health::pb::HealthCheckRequest;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;

fn install_ring() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn test_ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

/// A certificate for `name` and its private key, both PEM.
fn signed_by(
    ca: &CertifiedIssuer<'static, KeyPair>,
    name: &str,
    usage: ExtendedKeyUsagePurpose,
) -> (String, String) {
    let mut params = CertificateParams::new(vec![name.to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![usage];
    let key = KeyPair::generate().unwrap();
    let issuer: &Issuer<'static, KeyPair> = ca;
    let cert = params.signed_by(&key, issuer).unwrap();
    (cert.pem(), key.serialize_pem())
}

struct Pki {
    ca: CertifiedIssuer<'static, KeyPair>,
    ca_pem: String,
    server_pem: String,
    server_key_pem: String,
    client_pem: String,
    client_key_pem: String,
}

impl Pki {
    fn new() -> Self {
        let ca = test_ca("tls reload test CA");
        let (server_pem, server_key_pem) =
            signed_by(&ca, "localhost", ExtendedKeyUsagePurpose::ServerAuth);
        let (client_pem, client_key_pem) =
            signed_by(&ca, "client", ExtendedKeyUsagePurpose::ClientAuth);
        Self {
            ca_pem: ca.pem(),
            ca,
            server_pem,
            server_key_pem,
            client_pem,
            client_key_pem,
        }
    }
}

struct Files {
    dir: tempfile::TempDir,
}

impl Files {
    fn new(pki: &Pki) -> Self {
        let files = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        files.write("server.crt", &pki.server_pem);
        files.write("server.key", &pki.server_key_pem);
        files.write("ca.crt", &pki.ca_pem);
        files
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn write(&self, name: &str, content: &str) {
        std::fs::write(self.path(name), content).unwrap();
    }

    fn sources(&self) -> TlsSources {
        let file = |setting: &str, name: &str| PemSource::File {
            setting: setting.to_string(),
            path: self.path(name),
        };
        TlsSources {
            cert: file("server_cert_file", "server.crt"),
            key: file("server_key_file", "server.key"),
            ca: file("client_ca_cert_file", "ca.crt"),
        }
    }
}

struct TestServer {
    addr: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    remote_addrs: Arc<Mutex<Vec<Option<SocketAddr>>>>,
    /// SHA-256 of the client certificate of each accepted TLS connection, in order.
    client_certs: Arc<Mutex<Vec<String>>>,
}

impl TestServer {
    async fn start(tls: Option<&Arc<GrpcServerTls>>) -> Self {
        install_ring();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let remote_addrs = Arc::new(Mutex::new(Vec::new()));
        let recorded = remote_addrs.clone();
        let (_, health) = tonic_health::server::health_reporter();
        let health = tonic::service::interceptor::InterceptedService::new(
            health,
            move |req: tonic::Request<()>| {
                recorded.lock().unwrap().push(req.remote_addr());
                Ok(req)
            },
        );
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let client_certs = Arc::new(Mutex::new(Vec::new()));
        let seen = client_certs.clone();
        let incoming = tokio_stream::StreamExt::map(
            GrpcServerTls::incoming(
                tls,
                tokio_stream::wrappers::TcpListenerStream::new(listener),
            ),
            move |stream| {
                if let Ok(golem_service_base::tls::server::MaybeTlsStream::Tls(tls_stream)) =
                    &stream
                    && let Some(first) = tls_stream
                        .get_ref()
                        .1
                        .peer_certificates()
                        .and_then(|certs| certs.first())
                {
                    seen.lock().unwrap().push(sha256(first));
                }
                stream
            },
        );
        let task = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(health)
                .serve_with_incoming_shutdown(incoming, async move {
                    let _ = stopped.await;
                })
                .await;
        });
        Self {
            addr,
            stop: Some(stop),
            task,
            remote_addrs,
            client_certs,
        }
    }

    async fn stop(mut self) {
        let _ = self.stop.take().unwrap().send(());
        let _ = tokio::time::timeout(Duration::from_secs(5), self.task).await;
    }
}

fn tls_for(files: &Files, interval: Duration) -> Arc<GrpcServerTls> {
    install_ring();
    GrpcServerTls::from_sources("grpc server", files.sources(), interval).unwrap()
}

async fn health_client(
    addr: SocketAddr,
    ca: &str,
    identity: Option<(&str, &str)>,
) -> Result<HealthClient<tonic::transport::Channel>, tonic::transport::Error> {
    let mut config = ClientTlsConfig::new()
        .ca_certificate(Certificate::from_pem(ca))
        .domain_name("localhost");
    if let Some((cert, key)) = identity {
        config = config.identity(Identity::from_pem(cert, key));
    }
    let channel =
        tonic::transport::Endpoint::from_shared(format!("https://localhost:{}", addr.port()))
            .unwrap()
            .tls_config(config)?
            .connect_timeout(Duration::from_secs(5))
            .connect()
            .await?;
    Ok(HealthClient::new(channel))
}

async fn check(
    client: &mut HealthClient<tonic::transport::Channel>,
) -> Result<ServingStatus, tonic::Status> {
    Ok(client
        .check(HealthCheckRequest { service: "".into() })
        .await?
        .into_inner()
        .status())
}

/// A raw TLS handshake; the certificates the server presented.
async fn handshake(
    addr: SocketAddr,
    client: Option<(&str, &str)>,
    ca: &str,
) -> Result<Vec<CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject;
    install_ring();
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(ca.as_bytes()) {
        roots.add(cert.unwrap()).unwrap();
    }
    let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
    let mut config = match client {
        Some((cert, key)) => builder
            .with_client_auth_cert(
                CertificateDer::pem_slice_iter(cert.as_bytes())
                    .collect::<Result<_, _>>()
                    .unwrap(),
                rustls::pki_types::PrivateKeyDer::from_pem_slice(key.as_bytes()).unwrap(),
            )
            .unwrap(),
        None => builder.with_no_client_auth(),
    };
    config.alpn_protocols = vec![b"h2".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
    let mut stream = connector
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .map_err(|e| e.to_string())?;
    // TLS 1.3 reports a refused client certificate on the first read, not on connect.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream.write_all(b"x").await.map_err(|e| e.to_string())?;
    let mut buf = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .map_err(|_| "read timed out".to_string())?;
    if let Err(e) = read {
        return Err(e.to_string());
    }
    Ok(stream
        .get_ref()
        .1
        .peer_certificates()
        .unwrap()
        .iter()
        .map(|c| c.clone().into_owned())
        .collect())
}

fn sha256(cert: &CertificateDer<'_>) -> String {
    hex::encode(Sha256::digest(cert.as_ref()))
}

fn pem_sha256(pem: &str) -> String {
    use rustls::pki_types::pem::PemObject;
    sha256(
        &CertificateDer::pem_slice_iter(pem.as_bytes())
            .next()
            .unwrap()
            .unwrap(),
    )
}

const NEVER: Duration = Duration::from_secs(3600);

#[test]
async fn a_server_serves_grpc_over_mtls_from_files() {
    let pki = Pki::new();
    let files = Files::new(&pki);
    let tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&tls)).await;

    let mut client = health_client(
        server.addr,
        &pki.ca_pem,
        Some((&pki.client_pem, &pki.client_key_pem)),
    )
    .await
    .unwrap();
    assert_eq!(check(&mut client).await.unwrap(), ServingStatus::Serving);

    let refused = handshake(server.addr, None, &pki.ca_pem).await;
    assert!(
        refused.is_err(),
        "a client without a certificate must be refused"
    );
    server.stop().await;
}

#[test]
async fn a_renewed_server_certificate_is_used_by_the_next_handshake() {
    let pki = Pki::new();
    let files = Files::new(&pki);
    let tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&tls)).await;
    let client_id = Some((pki.client_pem.as_str(), pki.client_key_pem.as_str()));

    let mut open = health_client(server.addr, &pki.ca_pem, client_id)
        .await
        .unwrap();
    assert_eq!(check(&mut open).await.unwrap(), ServingStatus::Serving);
    let before = handshake(server.addr, client_id, &pki.ca_pem)
        .await
        .unwrap();
    assert_eq!(sha256(&before[0]), pem_sha256(&pki.server_pem));

    // Renew: a new server certificate from the same CA.
    let renewed = signed_by(&pki.ca, "localhost", ExtendedKeyUsagePurpose::ServerAuth);
    files.write("server.crt", &renewed.0);
    files.write("server.key", &renewed.1);
    assert!(matches!(
        tls.material().reload_if_changed(),
        ReloadOutcome::Reloaded
    ));

    let after = handshake(server.addr, client_id, &pki.ca_pem)
        .await
        .unwrap();
    assert_eq!(sha256(&after[0]), pem_sha256(&renewed.0));
    assert_ne!(sha256(&after[0]), sha256(&before[0]));
    assert_eq!(check(&mut open).await.unwrap(), ServingStatus::Serving);
    server.stop().await;
}

#[test]
async fn a_rotated_client_ca_is_used_by_the_next_handshake() {
    let pki = Pki::new();
    let files = Files::new(&pki);
    let tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&tls)).await;

    let ca_b = test_ca("tls reload test CA B");
    let (client_b, client_b_key) =
        signed_by(&ca_b, "client-b", ExtendedKeyUsagePurpose::ClientAuth);
    let client_b_id = Some((client_b.as_str(), client_b_key.as_str()));
    assert!(
        handshake(server.addr, client_b_id, &pki.ca_pem)
            .await
            .is_err()
    );

    files.write("ca.crt", &format!("{}{}", pki.ca_pem, ca_b.pem()));
    assert!(matches!(
        tls.material().reload_if_changed(),
        ReloadOutcome::Reloaded
    ));
    handshake(server.addr, client_b_id, &pki.ca_pem)
        .await
        .unwrap();
    // The old client still works.
    handshake(
        server.addr,
        Some((&pki.client_pem, &pki.client_key_pem)),
        &pki.ca_pem,
    )
    .await
    .unwrap();
    server.stop().await;
}

#[test]
async fn remote_addr_is_known_behind_the_tls_acceptor() {
    let pki = Pki::new();
    let files = Files::new(&pki);
    let tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&tls)).await;
    let mut client = health_client(
        server.addr,
        &pki.ca_pem,
        Some((&pki.client_pem, &pki.client_key_pem)),
    )
    .await
    .unwrap();
    check(&mut client).await.unwrap();
    let recorded = server.remote_addrs.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    let addr = recorded[0].expect("a remote address");
    assert_ne!(addr.port(), 0);
    server.stop().await;
}

#[test]
async fn a_silent_client_does_not_block_other_handshakes() {
    let pki = Pki::new();
    let files = Files::new(&pki);
    let tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&tls)).await;

    let _silent = TcpStream::connect(server.addr).await.unwrap();
    let mut client = tokio::time::timeout(
        Duration::from_secs(2),
        health_client(
            server.addr,
            &pki.ca_pem,
            Some((&pki.client_pem, &pki.client_key_pem)),
        ),
    )
    .await
    .expect("the handshake must not wait for the silent client")
    .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), check(&mut client))
            .await
            .unwrap()
            .unwrap(),
        ServingStatus::Serving
    );
    server.stop().await;
}

#[test]
async fn the_reloader_timer_picks_up_new_files() {
    let pki = Pki::new();
    let files = Files::new(&pki);
    let tls = tls_for(&files, Duration::from_millis(50));
    let server = TestServer::start(Some(&tls)).await;
    let client_id = Some((pki.client_pem.as_str(), pki.client_key_pem.as_str()));
    let before = handshake(server.addr, client_id, &pki.ca_pem)
        .await
        .unwrap();

    let renewed = signed_by(&pki.ca, "localhost", ExtendedKeyUsagePurpose::ServerAuth);
    files.write("server.crt", &renewed.0);
    files.write("server.key", &renewed.1);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let now = handshake(server.addr, client_id, &pki.ca_pem)
            .await
            .unwrap();
        if sha256(&now[0]) != sha256(&before[0]) {
            assert_eq!(sha256(&now[0]), pem_sha256(&renewed.0));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the new certificate was never served"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    server.stop().await;
}

#[test]
async fn a_plain_server_still_works_through_the_same_incoming() {
    let server = TestServer::start(None).await;
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{}", server.addr))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut client = HealthClient::new(channel);
    assert_eq!(check(&mut client).await.unwrap(), ServingStatus::Serving);
    server.stop().await;
}

fn client_config(
    files: &Files,
    max_connection_age: Duration,
) -> golem_service_base::grpc::client::GrpcClientConfig {
    use golem_service_base::grpc::client::{
        EnabledGrpcClientTlsConfig, GrpcClientConfig, GrpcClientTlsConfig,
    };
    let file = |name: &str| Some(files.path(name));
    GrpcClientConfig {
        connection_pool_size: std::num::NonZeroUsize::new(1).unwrap(),
        max_connection_age,
        retries_on_unavailable: golem_common::model::RetryConfig {
            max_attempts: 1,
            ..Default::default()
        },
        tls: GrpcClientTlsConfig::Enabled(EnabledGrpcClientTlsConfig {
            client_cert: None,
            client_cert_file: file("client.crt"),
            client_key: None,
            client_key_file: file("client.key"),
            server_ca_cert: None,
            server_ca_cert_file: file("client-ca.crt"),
            server_domain_name: Some("localhost".to_string()),
        }),
        ..Default::default()
    }
}

async fn client_check(
    client: &golem_service_base::grpc::client::GrpcClient<
        HealthClient<
            tonic_tracing_opentelemetry::middleware::client::OtelGrpcService<
                tonic::transport::Channel,
            >,
        >,
    >,
) -> Result<ServingStatus, tonic::Status> {
    client
        .call("check", |c| {
            Box::pin(async move {
                Ok(c.check(HealthCheckRequest { service: "".into() })
                    .await?
                    .into_inner()
                    .status())
            })
        })
        .await
}

#[test]
async fn a_client_presents_the_certificate_current_at_each_new_connection() {
    use golem_service_base::grpc::client::GrpcClient;
    let pki = Pki::new();
    let files = Files::new(&pki);
    let server_tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&server_tls)).await;

    files.write("client.crt", &pki.client_pem);
    files.write("client.key", &pki.client_key_pem);
    files.write("client-ca.crt", &pki.ca_pem);
    let client = GrpcClient::new(
        "t",
        |channel, _| HealthClient::new(channel),
        format!("https://localhost:{}", server.addr.port())
            .parse()
            .unwrap(),
        client_config(&files, Duration::from_millis(200)),
    );

    assert_eq!(client_check(&client).await.unwrap(), ServingStatus::Serving);
    assert_eq!(
        *server.client_certs.lock().unwrap(),
        vec![pem_sha256(&pki.client_pem)]
    );

    // Renew: a new client certificate from the same CA.
    let renewed = signed_by(&pki.ca, "client", ExtendedKeyUsagePurpose::ClientAuth);
    files.write("client.crt", &renewed.0);
    files.write("client.key", &renewed.1);
    let material = client.tls_material().expect("TLS is enabled");
    assert!(matches!(
        material.reload_if_changed(),
        ReloadOutcome::Reloaded
    ));

    // Wait for the connection to pass its maximum age, then call again: the next connection
    // carries the renewed certificate.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(client_check(&client).await.unwrap(), ServingStatus::Serving);
    }
    let seen = server.client_certs.lock().unwrap().clone();
    assert!(seen.len() >= 2, "{seen:?}");
    assert_eq!(seen.last().unwrap(), &pem_sha256(&renewed.0));
    server.stop().await;
}

#[test]
async fn a_client_with_a_bad_tls_file_fails_fast_with_the_setting_name() {
    use golem_service_base::grpc::client::GrpcClient;
    let pki = Pki::new();
    let files = Files::new(&pki);
    files.write("client.crt", &pki.client_pem);
    files.write("client-ca.crt", &pki.ca_pem);
    // client.key is never written
    let client = GrpcClient::new(
        "t",
        |channel, _| HealthClient::new(channel),
        "https://localhost:1".parse().unwrap(),
        client_config(&files, Duration::from_secs(60)),
    );
    assert!(client.tls_material().is_none());
    let error = client_check(&client).await.unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert!(error.message().contains("client_key_file"), "{error:?}");
}
