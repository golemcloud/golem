use crate::custom_api::http_test_context::{HttpTestContext, make_test_context};
use golem_common::base_model::agent::{AgentMode, AgentTypeName};
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::{AgentFilter, FilterComparator};
use golem_test_framework::config::EnvBasedTestDependencies;
use reqwest::{Body, Method, StatusCode};
use test_r::{define_matrix_dimension, inherit_test_dep, test, test_dep, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(
    #[tagged_as("postgres")]
    EnvBasedTestDependencies
);
inherit_test_dep!(
    #[tagged_as("sqlite")]
    EnvBasedTestDependencies
);

#[test_dep(scope = PerWorker, tagged_as = "postgres")]
async fn raw_router_context_postgres(
    #[tagged_as("postgres")] deps: &EnvBasedTestDependencies,
) -> HttpTestContext {
    raw_router_context(deps).await
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite")]
async fn raw_router_context_sqlite(
    #[tagged_as("sqlite")] deps: &EnvBasedTestDependencies,
) -> HttpTestContext {
    raw_router_context(deps).await
}

define_matrix_dimension!(db: HttpTestContext -> "postgres", "sqlite");

#[test_dep(scope = PerWorker)]
async fn raw_router_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    make_test_context(
        deps,
        vec![(
            AgentTypeName("RawHttpRouter".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )],
        "golem_it_agent_rpc_rust_release",
        "golem-it:agent-rpc-rust",
    )
    .await
    .unwrap()
}

#[test]
#[timeout("60s")]
async fn raw_router_streams_duplex_and_preserves_request(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(2);
    tx.send(Ok(b"first-asymmetric".to_vec())).await?;
    let request = raw_router_context
        .client
        .request(
            Method::PATCH,
            raw_router_context.base_url.join("/raw/echo-path?q=a%2Fb")?,
        )
        .body(Body::wrap_stream(ReceiverStream::new(rx)));
    let response_task = tokio::spawn(async move { request.send().await });
    let mut response =
        tokio::time::timeout(std::time::Duration::from_secs(10), response_task).await???;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-echo-method"], "PATCH");
    assert_eq!(response.headers()["x-echo-path"], "/raw/echo-path");
    assert_eq!(response.headers()["x-echo-query"], "q=a%2Fb");
    let cookies: Vec<_> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.as_bytes().to_vec())
        .collect();
    assert_eq!(
        cookies,
        vec![b"first=one".to_vec(), b"second=\x80".to_vec()]
    );
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), response.chunk())
        .await??
        .unwrap();
    assert_eq!(first, b"first-asymmetric".as_slice());
    tx.send(Ok(b"-second".to_vec())).await?;
    drop(tx);
    assert_eq!(response.bytes().await?, b"-second".as_slice());
    Ok(())
}

#[test]
#[timeout("60s")]
async fn raw_router_handles_large_incremental_duplex(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let first = vec![b'a'; 256 * 1024];
    let second = vec![b'b'; 256 * 1024];
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    tx.send(Ok(first.clone())).await?;
    let request = raw_router_context
        .client
        .post(raw_router_context.base_url.join("/raw/large")?)
        .body(Body::wrap_stream(ReceiverStream::new(rx)));
    let mut response =
        tokio::time::timeout(std::time::Duration::from_secs(30), request.send()).await??;
    let received_first = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut received = Vec::with_capacity(first.len());
        while received.len() < first.len() {
            let chunk = response
                .chunk()
                .await?
                .ok_or_else(|| anyhow::anyhow!("response ended before the first payload"))?;
            received.extend_from_slice(&chunk);
        }
        anyhow::ensure!(
            received.len() == first.len(),
            "response crossed the request payload boundary"
        );
        anyhow::Ok(received)
    })
    .await??;
    assert_eq!(received_first, first);
    tx.send(Ok(second.clone())).await?;
    drop(tx);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(30), response.bytes()).await??,
        second
    );
    Ok(())
}

#[test]
#[timeout("60s")]
async fn raw_router_returns_before_request_body_finishes(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    tx.send(Ok(b"still-open".to_vec())).await?;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        raw_router_context
            .client
            .post(raw_router_context.base_url.join("/raw/early-response")?)
            .body(Body::wrap_stream(ReceiverStream::new(rx)))
            .send(),
    )
    .await??;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.bytes().await?, b"accepted".as_slice());
    drop(tx);
    Ok(())
}

#[test]
async fn raw_router_http_body_rules(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    for (method, path, status, expected, expected_content_length) in [
        (Method::GET, "/raw/empty", 200, b"".as_slice(), None),
        (
            Method::GET,
            "/raw/known-one",
            200,
            b"K".as_slice(),
            Some("1"),
        ),
        (Method::HEAD, "/raw/head", 200, b"".as_slice(), None),
        (Method::GET, "/raw/204", 204, b"".as_slice(), None),
        (Method::GET, "/raw/205", 205, b"".as_slice(), Some("0")),
        (Method::GET, "/raw/304", 304, b"".as_slice(), Some("17")),
        (Method::GET, "/raw/cl-zero", 200, b"".as_slice(), Some("0")),
    ] {
        let response = raw_router_context
            .client
            .request(method, raw_router_context.base_url.join(path)?)
            .send()
            .await?;
        assert_eq!(response.status().as_u16(), status, "{path}");
        assert_eq!(
            response
                .headers()
                .get("content-length")
                .map(|value| value.to_str())
                .transpose()?,
            expected_content_length,
            "{path}"
        );
        assert_eq!(response.bytes().await?.as_ref(), expected, "{path}");
    }
    Ok(())
}

#[test]
async fn raw_router_rejects_invalid_response_head(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    for path in ["/raw/invalid-status", "/raw/invalid-header"] {
        let response = raw_router_context
            .client
            .get(raw_router_context.base_url.join(path)?)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{path}");
    }
    Ok(())
}

#[test]
#[timeout("60s")]
async fn raw_router_reports_response_body_failures(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    for path in ["/raw/short-cl", "/raw/overlong-cl"] {
        let response = raw_router_context
            .client
            .get(raw_router_context.base_url.join(path)?)
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(
            response.bytes().await.is_err(),
            "{path} body unexpectedly succeeded"
        );
    }

    let (release_failure, wait_for_response) = tokio::sync::oneshot::channel::<()>();
    let body = reqwest::Body::wrap_stream(futures::stream::once(async move {
        wait_for_response.await.unwrap();
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"fail-now"))
    }));
    let mut response = raw_router_context
        .client
        .post(raw_router_context.base_url.join("/raw/result-late-fail")?)
        .body(body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("transfer-encoding")
            .map(|v| v.as_bytes()),
        Some(b"chunked".as_slice()),
        "{:?} {:?}",
        response.version(),
        response.headers()
    );
    assert_eq!(response.chunk().await?.unwrap(), b"before-error".as_slice());
    release_failure.send(()).unwrap();
    let terminal = response.chunk().await;
    assert!(terminal.is_err(), "late failure returned {terminal:?}");

    let response = raw_router_context
        .client
        .post(
            raw_router_context
                .base_url
                .join("/raw/one-byte-late-fail")?,
        )
        .body("fail-now")
        .send()
        .await;
    // The final byte is withheld, so Hyper may abort before flushing the head.
    // In neither case may the client receive the promised byte and successful EOF.
    if let Ok(response) = response {
        if response.status() == StatusCode::OK {
            assert!(response.bytes().await.is_err());
        } else {
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        }
    }
    Ok(())
}

#[test]
#[timeout("30s")]
async fn raw_router_cl_zero_late_failure_is_rejected_before_head(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = raw_router_context
        .client
        .post(raw_router_context.base_url.join("/raw/cl-zero-late-fail")?)
        .body("release-failure")
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    Ok(())
}

#[test]
#[timeout("30s")]
async fn raw_router_uses_official_transport_framing(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    use golem_test_framework::config::TestDependencies;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let deps = &raw_router_context.user.deps;
    let host = raw_router_context.host_header.to_str()?;
    let gateway_port = raw_router_context.base_url.port().unwrap();
    for (port, fields, body, status) in [
        (
            gateway_port,
            "Content-Length: 0\r\nContent-Length: 0\r\nConnection: close\r\n",
            "",
            200,
        ),
        (gateway_port, "Content-Length: 0, 0\r\n", "", 400),
        (
            gateway_port,
            "Content-Length: 0\r\nContent-Length: 1\r\n",
            "",
            400,
        ),
        (
            gateway_port,
            "Content-Length: 0\r\nTransfer-Encoding: chunked\r\n",
            "0\r\n\r\n",
            200,
        ),
        (
            gateway_port,
            "Transfer-Encoding: chunked\r\nContent-Length: 0\r\n",
            "0\r\n\r\n",
            200,
        ),
        (
            gateway_port,
            "Transfer-Encoding: gzip, chunked\r\nConnection: close\r\n",
            "0\r\n\r\n",
            501,
        ),
        (gateway_port, "Transfer-Encoding: gzip\r\n", "", 400),
    ] {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        // Hyper closes TE+CL and invalid-framing connections without an explicit close header.
        socket
            .write_all(
                format!("POST /raw/empty HTTP/1.1\r\nHost: {host}\r\n{fields}\r\n{body}")
                    .as_bytes(),
            )
            .await?;
        let mut wire = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.read_to_end(&mut wire),
        )
        .await??;
        assert!(
            wire.starts_with(format!("HTTP/1.1 {status} ").as_bytes()),
            "{fields}: {wire:?}"
        );
    }

    let mut socket =
        tokio::net::TcpStream::connect(("127.0.0.1", deps.worker_service().http_port())).await?;
    socket.write_all(b"GET /healthcheck HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
    let mut wire = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        socket.read_to_end(&mut wire),
    )
    .await??;
    assert!(
        !wire.starts_with(b"HTTP/1.1 400 "),
        "management listener unexpectedly enabled strict framing"
    );
    assert!(wire.starts_with(b"HTTP/1.1 "));
    Ok(())
}

#[test]
#[timeout("30s")]
async fn raw_router_h2c_preserves_304_representation_length(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let host = raw_router_context.host_header.to_str()?;
    let port = raw_router_context.base_url.port().unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .resolve(host, std::net::SocketAddr::from(([127, 0, 0, 1], port)))
        .build()?;
    let mut url = raw_router_context.base_url.join("/raw/304")?;
    url.set_host(Some(host))?;
    url.set_port(None)
        .map_err(|_| anyhow::anyhow!("invalid test URL"))?;
    let response = client.get(url).send().await?;
    assert_eq!(response.version(), reqwest::Version::HTTP_2);
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(response.headers()["content-length"], "17");
    assert!(response.bytes().await?.is_empty());
    Ok(())
}

#[test]
#[timeout("60s")]
async fn raw_router_accepts_large_producer_chunk(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let response = raw_router_context
        .client
        .get(raw_router_context.base_url.join("/raw/large-chunk")?)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await?.as_ref(), vec![0xab; 512 * 1024]);
    Ok(())
}

#[test]
#[timeout("60s")]
async fn raw_router_disconnect_stops_private_execution(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let raw_router_context = &self::raw_router_context(&raw_router_context.user.deps).await;
    use golem_common::model::ScanCursor;
    use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
    use golem_test_framework::dsl::TestDsl;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for path in ["/raw/wait-head", "/raw/echo", "/raw/slow"] {
        let (_, before) = raw_router_context
            .user
            .get_workers_metadata(
                &raw_router_context.component_id,
                Some(AgentFilter::new_mode(
                    FilterComparator::Equal,
                    AgentMode::Ephemeral,
                )),
                ScanCursor::default(),
                10_000,
                true,
            )
            .await?;
        let mut socket = tokio::net::TcpStream::connect((
            "127.0.0.1",
            raw_router_context.base_url.port().unwrap(),
        ))
        .await?;
        socket.write_all(format!("POST {path} HTTP/1.1\r\nHost: {}\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n", raw_router_context.host_header.to_str()?).as_bytes()).await?;
        let agent = tokio::time::timeout(Duration::from_secs(10), async {
            'discover: loop {
                let (_, agents) = raw_router_context
                    .user
                    .get_workers_metadata(
                        &raw_router_context.component_id,
                        Some(AgentFilter::new_mode(
                            FilterComparator::Equal,
                            AgentMode::Ephemeral,
                        )),
                        ScanCursor::default(),
                        10_000,
                        false,
                    )
                    .await?;
                for agent_id in agents
                    .into_iter()
                    .map(|agent| agent.agent_id)
                    .filter(|agent_id| !before.iter().any(|old| old.agent_id == *agent_id))
                {
                    let oplog = raw_router_context
                        .user
                        .get_oplog(&agent_id, OplogIndex::INITIAL)
                        .await?;
                    if oplog.iter().any(|entry| {
                        matches!(&entry.entry,
                        PublicOplogEntry::AgentInvocationStarted(started)
                            if matches!(&started.invocation,
                                PublicAgentInvocation::AgentMethodInvocation(method)
                                    if method.method_name == "route"))
                    }) {
                        break 'discover anyhow::Ok(agent_id);
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await??;
        if path != "/raw/wait-head" {
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await?;
                head.push(byte[0]);
            }
            assert!(head.starts_with(b"HTTP/1.1 200 "));
        }
        drop(socket);
        wait_for_stopped(raw_router_context, &agent).await?;
    }
    Ok(())
}

#[test]
#[timeout("60s")]
async fn raw_router_h2_reset_stops_private_execution(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let raw_router_context = &self::raw_router_context(&raw_router_context.user.deps).await;
    use golem_common::model::ScanCursor;
    use golem_test_framework::dsl::TestDsl;
    use std::time::Duration;

    let (_, before) = raw_router_context
        .user
        .get_workers_metadata(
            &raw_router_context.component_id,
            Some(AgentFilter::new_mode(
                FilterComparator::Equal,
                AgentMode::Ephemeral,
            )),
            ScanCursor::default(),
            10_000,
            true,
        )
        .await?;
    let host = raw_router_context.host_header.to_str()?;
    let port = raw_router_context.base_url.port().unwrap();
    let socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let (mut client, connection) = h2::client::handshake(socket).await?;
    let connection = tokio::spawn(connection);
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("http://{host}/raw/slow"))
        .body(())?;
    let (response, mut request_body) = client.send_request(request, false)?;
    request_body.send_data(bytes::Bytes::from_static(b"x"), true)?;
    let response = response.await?;
    assert_eq!(response.status(), StatusCode::OK);

    let agent = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (_, agents) = raw_router_context
                .user
                .get_workers_metadata(
                    &raw_router_context.component_id,
                    Some(AgentFilter::new_mode(
                        FilterComparator::Equal,
                        AgentMode::Ephemeral,
                    )),
                    ScanCursor::default(),
                    10_000,
                    false,
                )
                .await?;
            if let Some(agent_id) = agents
                .into_iter()
                .map(|agent| agent.agent_id)
                .find(|agent_id| !before.iter().any(|old| old.agent_id == *agent_id))
            {
                // The response head proves this isolated request's method has started.
                // Do not chase a growing output oplog before sending the reset.
                break anyhow::Ok(agent_id);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("HTTP/2 agent discovery timed out"))??;
    request_body.send_reset(h2::Reason::CANCEL);
    drop(response);
    let stopped = wait_for_stopped(raw_router_context, &agent).await;
    assert!(
        !connection.is_finished(),
        "reset must not close the HTTP/2 connection"
    );
    connection.abort();
    stopped?;
    Ok(())
}

async fn wait_for_stopped(
    context: &HttpTestContext,
    agent: &golem_common::model::AgentId,
) -> anyhow::Result<()> {
    use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};
    use golem_test_framework::dsl::TestDsl;
    use std::time::Duration;

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let oplog = context.user.get_oplog(agent, OplogIndex::INITIAL).await?;
            if oplog
                .iter()
                .rev()
                .take_while(|entry| {
                    !matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_))
                })
                .any(|entry| {
                    matches!(
                        entry.entry,
                        PublicOplogEntry::Interrupted(_)
                            | PublicOplogEntry::AgentInvocationFinished(_)
                    )
                })
            {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    Ok(())
}

#[test]
#[timeout("30s")]
async fn raw_router_cors_preflight_does_not_invoke(
    #[dimension(db)] raw_router_context: &HttpTestContext,
) -> anyhow::Result<()> {
    let raw_router_context = &self::raw_router_context(&raw_router_context.user.deps).await;
    use golem_common::model::ScanCursor;
    use golem_test_framework::dsl::TestDsl;
    let count = || async {
        raw_router_context
            .user
            .get_workers_metadata(
                &raw_router_context.component_id,
                Some(AgentFilter::new_mode(
                    FilterComparator::Equal,
                    AgentMode::Ephemeral,
                )),
                ScanCursor::default(),
                100,
                true,
            )
            .await
            .map(|(_, agents)| agents.len())
    };
    let before = count().await?;
    let response = raw_router_context
        .client
        .request(
            Method::OPTIONS,
            raw_router_context.base_url.join("/raw/echo")?,
        )
        .header("origin", "https://allowed.test")
        .header("access-control-request-method", "POST")
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://allowed.test"
    );
    assert_eq!(count().await?, before);
    Ok(())
}
