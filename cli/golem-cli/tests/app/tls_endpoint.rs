// The platform TLS verifier honours SSL_CERT_FILE on Linux.
#![cfg(target_os = "linux")]

use crate::app::{TestContext, cmd, flag};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::{test, timeout};
use tokio::net::{TcpListener, TcpStream};

const CA_PEM: &[u8] =
    include_bytes!("../../../../golem-registry-service/src/services/mcp_import/tls/ca.pem");
const SERVER_CERT_PEM: &[u8] = include_bytes!(
    "../../../../golem-registry-service/src/services/mcp_import/tls/server-cert.pem"
);
const SERVER_KEY_PEM: &[u8] =
    include_bytes!("../../../../golem-registry-service/src/services/mcp_import/tls/server-key.pem");

/// Terminates TLS on `https://127.0.0.1:<port>` and forwards the plain bytes to the router port,
/// including WebSocket upgrades. Returns the proxy URL and the count of completed handshakes.
async fn start_tls_proxy(router_port: u16) -> (String, Arc<AtomicUsize>) {
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        CertificateDer::pem_slice_iter(SERVER_CERT_PEM)
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        PrivateKeyDer::from_pem_slice(SERVER_KEY_PEM).unwrap(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let handshakes = Arc::new(AtomicUsize::new(0));
    let counter = handshakes.clone();
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let Ok(mut client) = acceptor.accept(socket).await else {
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let mut upstream = TcpStream::connect(("127.0.0.1", router_port))
                    .await
                    .unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            });
        }
    });
    (url, handshakes)
}

/// Writes the system CA bundle and the fixture CA to one file for `SSL_CERT_FILE`.
fn trust_file_with_fixture_ca() -> tempfile::NamedTempFile {
    let trust = tempfile::NamedTempFile::new().unwrap();
    let system_ca_path = std::env::var_os("SSL_CERT_FILE").unwrap_or_else(|| {
        [
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/certs/ca-bundle.crt",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .expect("a system CA bundle or SSL_CERT_FILE is required")
        .into()
    });
    let system_ca = std::fs::read(system_ca_path).unwrap();
    std::fs::write(trust.path(), [system_ca.as_slice(), CA_PEM].concat()).unwrap();
    trust
}

#[test]
#[timeout("15 minutes")]
async fn agent_invoke_works_against_tls_endpoint_with_certificate_checks() {
    let trust = trust_file_with_fixture_ca();
    let mut ctx = TestContext::new();
    let app_name = "tls-endpoint";
    let agent_name = "CounterAgent(\"tls\")";

    ctx.add_env_var("SSL_CERT_FILE", trust.path().to_str().unwrap());
    ctx.start_server().await;
    let (proxy_url, handshakes) = start_tls_proxy(ctx.router_port()).await;
    ctx.add_env_var("GOLEM_BUILTIN_LOCAL_URL", proxy_url);

    let output = ctx
        .cli([flag::YES, cmd::NEW, app_name, flag::TEMPLATE, "rust"])
        .await;
    assert!(output.success_or_dump());
    ctx.cd(app_name);
    let output = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(output.success_or_dump());

    // `ctx.cli` runs the full `golem` binary.
    let output = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent_name, "increment"])
        .await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains_ordered(["Invocation result in Rust syntax:", "1"]));

    ctx.golem_cli_path = PathBuf::from(env!("CARGO_BIN_EXE_golem-cli"));
    let output = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent_name, "increment"])
        .await;
    assert!(output.success_or_dump());
    assert!(output.stdout_contains_ordered(["Invocation result in Rust syntax:", "2"]));

    assert!(handshakes.load(Ordering::SeqCst) > 0);
}
