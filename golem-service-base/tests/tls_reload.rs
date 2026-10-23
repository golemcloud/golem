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
    /// Changes the status the health service reports, which reaches every open `watch` stream.
    health: tonic_health::server::HealthReporter,
}

impl TestServer {
    async fn start(tls: Option<&Arc<GrpcServerTls>>) -> Self {
        install_ring();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let remote_addrs = Arc::new(Mutex::new(Vec::new()));
        let recorded = remote_addrs.clone();
        let (reporter, health) = tonic_health::server::health_reporter();
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
            health: reporter,
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
async fn a_garbage_handshake_leaves_the_server_serving() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let pki = Pki::new();
    let files = Files::new(&pki);
    let tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&tls)).await;

    let mut raw = TcpStream::connect(server.addr).await.unwrap();
    raw.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    let mut sink = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), raw.read_to_end(&mut sink)).await;
    drop(raw);

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

    let mut client = health_client(
        server.addr,
        &pki.ca_pem,
        Some((&pki.client_pem, &pki.client_key_pem)),
    )
    .await
    .unwrap();
    assert_eq!(check(&mut client).await.unwrap(), ServingStatus::Serving);
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
async fn a_client_first_used_after_a_renewal_presents_the_renewed_certificate() {
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
        client_config(&files, Duration::from_secs(60)),
    );

    // The certificate is renewed while the client has not connected yet: its poller has not
    // started, so nothing has looked at the files since the client was built.
    let renewed = signed_by(&pki.ca, "client", ExtendedKeyUsagePurpose::ClientAuth);
    files.write("client.crt", &renewed.0);
    files.write("client.key", &renewed.1);

    assert_eq!(client_check(&client).await.unwrap(), ServingStatus::Serving);
    assert_eq!(
        *server.client_certs.lock().unwrap(),
        vec![pem_sha256(&renewed.0)],
        "the first connection must present the renewed certificate"
    );
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

#[test]
async fn a_client_recovers_when_its_tls_files_appear_after_start() {
    use golem_service_base::grpc::client::GrpcClient;
    let pki = Pki::new();
    let files = Files::new(&pki);
    let server_tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&server_tls)).await;

    files.write("client.crt", &pki.client_pem);
    files.write("client-ca.crt", &pki.ca_pem);
    // client.key does not exist yet
    let client = GrpcClient::new(
        "t",
        |channel, _| HealthClient::new(channel),
        format!("https://localhost:{}", server.addr.port())
            .parse()
            .unwrap(),
        client_config(&files, Duration::from_secs(60)),
    );
    let error = client_check(&client).await.unwrap_err();
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert!(error.message().contains("client_key_file"), "{error:?}");
    assert!(client.tls_material().is_none());

    files.write("client.key", &pki.client_key_pem);
    client.retry_tls_load_now().await;
    assert_eq!(client_check(&client).await.unwrap(), ServingStatus::Serving);
    assert!(client.tls_material().is_some());
    assert_eq!(
        *server.client_certs.lock().unwrap(),
        vec![pem_sha256(&pki.client_pem)]
    );
    server.stop().await;
}

/// A TCP proxy in front of a server that can cut every connection open through it, as a network
/// fault or a restarted load balancer would. Connections made after a cut are proxied as before.
struct Proxy {
    addr: SocketAddr,
    connections: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Proxy {
    async fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(Mutex::new(Vec::new()));
        let open = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                let connection = tokio::spawn(async move {
                    if let Ok(mut outbound) = TcpStream::connect(upstream).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
                open.lock().unwrap().push(connection);
            }
        });
        Self {
            addr,
            connections,
            task,
        }
    }

    /// Closes both sides of every connection open through the proxy now.
    fn cut(&self) {
        for connection in self.connections.lock().unwrap().drain(..) {
            connection.abort();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
        self.cut();
    }
}

type HealthOverGrpcClient = HealthClient<
    tonic_tracing_opentelemetry::middleware::client::OtelGrpcService<tonic::transport::Channel>,
>;

async fn multi_target_check(
    client: &golem_service_base::grpc::client::MultiTargetGrpcClient<HealthOverGrpcClient>,
    target: &http::Uri,
) -> Result<ServingStatus, tonic::Status> {
    client
        .call("check", target.clone(), |c| {
            Box::pin(async move {
                Ok(c.check(HealthCheckRequest { service: "".into() })
                    .await?
                    .into_inner()
                    .status())
            })
        })
        .await
}

/// tonic re-dials a lost connection inside the cached `Channel`, with the TLS configuration the
/// `Channel` was built with, and a multi-target client never replaces its connections by age. So
/// a reconnect after a renewal must not go through the channel cached before it.
#[test]
async fn a_reconnect_inside_a_cached_multi_target_channel_presents_the_renewed_certificate() {
    use golem_service_base::grpc::client::MultiTargetGrpcClient;
    let pki = Pki::new();
    let files = Files::new(&pki);
    let server_tls = tls_for(&files, NEVER);
    let server = TestServer::start(Some(&server_tls)).await;
    let proxy = Proxy::start(server.addr).await;

    files.write("client.crt", &pki.client_pem);
    files.write("client.key", &pki.client_key_pem);
    files.write("client-ca.crt", &pki.ca_pem);
    let client = MultiTargetGrpcClient::new(
        "t",
        |channel, _| HealthClient::new(channel),
        client_config(&files, Duration::ZERO),
    );
    let target: http::Uri = format!("https://127.0.0.1:{}", proxy.addr.port())
        .parse()
        .unwrap();

    assert_eq!(
        multi_target_check(&client, &target).await.unwrap(),
        ServingStatus::Serving
    );
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

    // Cut the TCP connection under the cached channel, and give the client the moment it needs to
    // see it closed, so that the next call finds a channel that re-dials rather than one whose
    // request fails on a dead connection.
    proxy.cut();
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        multi_target_check(&client, &target).await.unwrap(),
        ServingStatus::Serving
    );
    assert_eq!(
        *server.client_certs.lock().unwrap(),
        vec![pem_sha256(&pki.client_pem), pem_sha256(&renewed.0)],
        "the connection made after the reload must present the renewed certificate"
    );
    server.stop().await;
}

/// A reload moves new calls to a new connection and leaves a call already in progress on the
/// connection it started on. The fixed-target pool here keeps a connection for an hour, so only
/// the reload can move the new call.
#[test]
async fn an_open_stream_keeps_its_connection_while_new_calls_present_the_renewed_certificate() {
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
        client_config(&files, Duration::from_secs(3600)),
    );

    let mut watch = client
        .call_without_retry("watch", |c| {
            Box::pin(async move {
                Ok(c.watch(HealthCheckRequest { service: "".into() })
                    .await?
                    .into_inner())
            })
        })
        .await
        .unwrap();
    assert_eq!(
        watch.message().await.unwrap().unwrap().status(),
        ServingStatus::Serving
    );
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

    assert_eq!(client_check(&client).await.unwrap(), ServingStatus::Serving);
    assert_eq!(
        *server.client_certs.lock().unwrap(),
        vec![pem_sha256(&pki.client_pem), pem_sha256(&renewed.0)],
        "a call after the reload must open a connection with the renewed certificate"
    );

    // The stream opened before the reload still receives on its own connection.
    server
        .health
        .set_service_status("", tonic_health::ServingStatus::NotServing)
        .await;
    let update = tokio::time::timeout(Duration::from_secs(5), watch.message())
        .await
        .expect("the stream opened before the reload stopped receiving")
        .unwrap()
        .unwrap();
    assert_eq!(update.status(), ServingStatus::NotServing);
    assert_eq!(
        server.client_certs.lock().unwrap().len(),
        2,
        "the stream opened before the reload was moved to another connection"
    );
    drop(watch);
    server.stop().await;
}

/// A file whose read blocks until the test lets it through: the path becomes a symbolic link to a
/// FIFO, and opening a FIFO for reading waits for a writer. A thread lets the read through once
/// the test has seen its runtime run another task, or after 3 s, so that a runtime blocked by the
/// read fails the test instead of hanging it.
#[cfg(unix)]
struct SlowRead {
    progressed: Arc<std::sync::atomic::AtomicBool>,
    releaser: std::thread::JoinHandle<(bool, bool)>,
}

#[cfg(unix)]
impl SlowRead {
    /// From the release on, `path` is an ordinary file that holds `content`, and the read blocked on
    /// the FIFO gets the same `content`.
    fn start(path: &std::path::Path, content: &[u8]) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let beside = |suffix: &str| {
            let mut name = path.file_name().unwrap().to_os_string();
            name.push(suffix);
            path.with_file_name(name)
        };
        let fifo = beside(".fifo");
        let staged = beside(".staged");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success(), "mkfifo failed");
        let _ = std::fs::remove_file(path);
        std::os::unix::fs::symlink(&fifo, path).unwrap();
        let progressed = Arc::new(AtomicBool::new(false));
        let releaser = std::thread::spawn({
            let progressed = progressed.clone();
            let path = path.to_path_buf();
            let content = content.to_vec();
            move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                while !progressed.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let in_time = progressed.load(Ordering::SeqCst);
                std::fs::write(&staged, &content).unwrap();
                std::fs::rename(&staged, &path).unwrap();
                // Opening the FIFO for writing waits for the read that is blocked on it.
                let (written, was_written) = std::sync::mpsc::channel();
                let writer = std::thread::spawn({
                    let fifo = fifo.clone();
                    move || {
                        let _ = std::fs::write(&fifo, &content);
                        let _ = written.send(());
                    }
                });
                let was_read = was_written.recv_timeout(Duration::from_secs(5)).is_ok();
                if !was_read {
                    // Nothing reads the FIFO: open it here, so that the writer ends.
                    let _ = std::fs::File::open(&fifo);
                }
                writer.join().unwrap();
                (in_time, was_read)
            }
        });
        Self {
            progressed,
            releaser,
        }
    }

    /// Records that the runtime ran another task while the read was blocked.
    fn progressed(&self) {
        self.progressed
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn finish(self) {
        let (in_time, was_read) = self.releaser.join().unwrap();
        assert!(was_read, "nothing read the slow file");
        assert!(
            in_time,
            "the runtime ran no other task while a TLS file read was blocked"
        );
    }
}

/// One thread for the whole runtime, so that a read on it would stop every task.
#[cfg(unix)]
fn single_threaded_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(unix)]
#[test]
fn a_server_poller_reads_a_slow_file_without_blocking_the_runtime() {
    let pki = Pki::new();
    let files = Files::new(&pki);
    let renewed = signed_by(&pki.ca, "localhost", ExtendedKeyUsagePurpose::ServerAuth);
    let read = single_threaded_runtime().block_on(async {
        install_ring();
        let tls =
            GrpcServerTls::from_sources("slow server", files.sources(), Duration::from_millis(20))
                .unwrap();
        files.write("server.key", &renewed.1);
        let read = SlowRead::start(&files.path("server.crt"), renewed.0.as_bytes());
        tokio::time::sleep(Duration::from_millis(200)).await;
        read.progressed();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tls.material().current().certificate.sha256 != pem_sha256(&renewed.0) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the renewed certificate was never loaded"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        read
    });
    read.finish();
}

#[cfg(unix)]
#[test]
fn a_client_reads_a_slow_file_at_its_first_connection_without_blocking_the_runtime() {
    use golem_service_base::grpc::client::GrpcClient;
    let pki = Pki::new();
    let files = Files::new(&pki);
    files.write("client.crt", &pki.client_pem);
    files.write("client.key", &pki.client_key_pem);
    files.write("client-ca.crt", &pki.ca_pem);
    let renewed = signed_by(&pki.ca, "client", ExtendedKeyUsagePurpose::ClientAuth);
    let (seen, read) = single_threaded_runtime().block_on(async {
        let server_tls = tls_for(&files, NEVER);
        let server = TestServer::start(Some(&server_tls)).await;
        let client = GrpcClient::new(
            "t",
            |channel, _| HealthClient::new(channel),
            format!("https://localhost:{}", server.addr.port())
                .parse()
                .unwrap(),
            client_config(&files, Duration::from_secs(60)),
        );
        // Renewed after the client was built and before its first connection, which looks at
        // the files once: that look is the read that blocks.
        files.write("client.key", &renewed.1);
        let read = SlowRead::start(&files.path("client.crt"), renewed.0.as_bytes());
        let call = tokio::spawn(async move { client_check(&client).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        read.progressed();
        let status = tokio::time::timeout(Duration::from_secs(10), call)
            .await
            .expect("the call did not end")
            .unwrap();
        assert_eq!(status.unwrap(), ServingStatus::Serving);
        let seen = server.client_certs.lock().unwrap().clone();
        server.stop().await;
        (seen, read)
    });
    read.finish();
    assert_eq!(
        seen,
        vec![pem_sha256(&renewed.0)],
        "the first connection must present the renewed certificate"
    );
}

#[cfg(unix)]
#[test]
fn a_client_retries_its_tls_load_from_a_slow_file_without_blocking_the_runtime() {
    use golem_service_base::grpc::client::GrpcClient;
    let pki = Pki::new();
    let files = Files::new(&pki);
    files.write("client.crt", &pki.client_pem);
    files.write("client-ca.crt", &pki.ca_pem);
    // client.key does not exist yet, so the client starts without TLS material.
    let (seen, read) = single_threaded_runtime().block_on(async {
        let server_tls = tls_for(&files, NEVER);
        let server = TestServer::start(Some(&server_tls)).await;
        let client = GrpcClient::new(
            "t",
            |channel, _| HealthClient::new(channel),
            format!("https://localhost:{}", server.addr.port())
                .parse()
                .unwrap(),
            client_config(&files, Duration::from_secs(60)),
        );
        assert!(client.tls_material().is_none());
        let read = SlowRead::start(&files.path("client.key"), pki.client_key_pem.as_bytes());
        client.retry_tls_load_now().await;
        let call = tokio::spawn(async move { client_check(&client).await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        read.progressed();
        let status = tokio::time::timeout(Duration::from_secs(10), call)
            .await
            .expect("the call did not end")
            .unwrap();
        assert_eq!(status.unwrap(), ServingStatus::Serving);
        let seen = server.client_certs.lock().unwrap().clone();
        server.stop().await;
        (seen, read)
    });
    read.finish();
    assert_eq!(seen, vec![pem_sha256(&pki.client_pem)]);
}
