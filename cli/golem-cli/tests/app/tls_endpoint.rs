// The platform TLS verifier honours SSL_CERT_FILE on Linux.
#![cfg(target_os = "linux")]

use crate::app::agents::{streaming_agent, streaming_invocation_context};
use crate::app::{TestContext, cmd};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

const CA_PEM: &[u8] =
    include_bytes!("../../../../golem-registry-service/src/services/mcp_import/tls/ca.pem");
const SERVER_CERT_PEM: &[u8] = include_bytes!(
    "../../../../golem-registry-service/src/services/mcp_import/tls/server-cert.pem"
);
const SERVER_KEY_PEM: &[u8] =
    include_bytes!("../../../../golem-registry-service/src/services/mcp_import/tls/server-key.pem");

const INVOCATION_SESSION_PATH: &str = "/v1/agents/invoke-agent-session";

/// Terminates TLS on `https://127.0.0.1:<port>` and forwards the plain bytes to the router port,
/// including WebSocket upgrades. Returns the proxy URL and the path of the first request on each
/// connection.
async fn start_tls_proxy(router_port: u16) -> (String, Arc<Mutex<Vec<String>>>) {
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
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorded = paths.clone();
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let Ok(mut client) = acceptor.accept(socket).await else {
                    return;
                };
                let mut head = Vec::new();
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut chunk = [0u8; 4096];
                    match client.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => head.extend_from_slice(&chunk[..read]),
                    }
                }
                if let Some(path) = String::from_utf8_lossy(&head).split_whitespace().nth(1) {
                    recorded.lock().unwrap().push(path.to_string());
                }
                let mut upstream = TcpStream::connect(("127.0.0.1", router_port))
                    .await
                    .unwrap();
                upstream.write_all(&head).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            });
        }
    });
    (url, paths)
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

/// Runs a streaming method, which needs the invocation session WebSocket, and checks the values
/// that the output stream delivers.
async fn assert_streaming_invoke(ctx: &TestContext) {
    let agent = streaming_agent(&Uuid::new_v4().to_string());
    let output = ctx
        .cli_with_input(
            [cmd::AGENT, cmd::INVOKE, &agent, "produce", "[4, 5, 6]"],
            b"",
        )
        .await;
    assert!(output.success(), "{}", output.stderr_text());
    assert!(output.stdout_text().contains("4\n5\n6\n"));
}

/// Runs `agent stream`, which needs the agent stream WebSocket, and waits for a log line that an
/// invocation of the agent writes while the stream is open.
async fn assert_agent_stream(ctx: &TestContext) {
    let agent = format!(r#"RpcGlobalState("{}")"#, Uuid::new_v4());
    let marker = format!("tls-{}", Uuid::new_v4());
    let created = ctx
        .cli([cmd::AGENT, cmd::INVOKE, &agent, "inc_global_by", "1"])
        .await;
    assert!(created.success_or_dump());

    let expected = format!("Got {marker}");
    let stream = ctx.cli_process_after_line([cmd::AGENT, "stream", &agent], &expected);
    let invocations = async {
        let argument = format!("\"{marker}\"");
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let logged = ctx
                .cli([cmd::AGENT, cmd::INVOKE, &agent, "bug_golem1265", &argument])
                .await;
            assert!(logged.success_or_dump());
        }
    };
    tokio::select! {
        (mut child, _) = stream => {
            child.kill().await.unwrap();
        }
        _ = invocations => panic!("agent stream did not show {expected}"),
    }
}

#[test]
#[timeout("20 minutes")]
async fn agent_invoke_and_stream_work_against_tls_endpoint_with_certificate_checks() {
    let trust = trust_file_with_fixture_ca();
    let mut ctx = streaming_invocation_context().await;
    let (proxy_url, paths) = start_tls_proxy(ctx.router_port()).await;
    ctx.add_env_var("SSL_CERT_FILE", trust.path().to_str().unwrap());
    ctx.add_env_var("GOLEM_BUILTIN_LOCAL_URL", proxy_url);

    // `ctx.cli` runs the full `golem` binary.
    assert_streaming_invoke(&ctx).await;
    assert_agent_stream(&ctx).await;
    assert_websockets_used_tls(&paths);

    ctx.golem_cli_path = PathBuf::from(env!("CARGO_BIN_EXE_golem-cli"));
    assert_streaming_invoke(&ctx).await;
    assert_agent_stream(&ctx).await;
    assert_websockets_used_tls(&paths);
}

/// Checks that the proxy forwarded both WebSocket connections, then clears the recorded paths.
fn assert_websockets_used_tls(paths: &Mutex<Vec<String>>) {
    let mut paths = paths.lock().unwrap();
    assert!(
        paths.iter().any(|path| path == INVOCATION_SESSION_PATH),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|path| path.ends_with("/connect")),
        "{paths:?}"
    );
    paths.clear();
}
