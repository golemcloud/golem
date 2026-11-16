use crate::http_test_context::{HttpTestContext, make_test_context};
use golem_client::api::RegistryServiceClient;
use golem_common::base_model::ScanCursor;
use golem_common::base_model::agent::{AgentMode, AgentTypeName};
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::{AgentFilter, FilterComparator};
use golem_test_framework::components::worker_service::WorkerService;
use golem_test_framework::components::worker_service::spawned::SpawnedWorkerService;
use golem_test_framework::config::{
    EnvBasedTestDependencies, TestDependencies, WorkerExecutorClusterControl,
    WorkerExecutorClusterControlStub,
};
use golem_test_framework::dsl::TestDsl;
use reqwest::{Body, Method, Response, StatusCode};
use std::path::PathBuf;
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::Level;

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(WorkerExecutorClusterControlStub);

struct RawHttpProcessLossContext {
    http: HttpTestContext,
    component_id: ComponentId,
}

#[test_dep(scope = PerWorker)]
async fn raw_http_process_loss_context(
    deps: &EnvBasedTestDependencies,
) -> RawHttpProcessLossContext {
    let http = make_test_context(
        deps,
        vec![(
            AgentTypeName("RawHttpRouter".to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )],
        "golem_it_agent_rpc_rust_release",
        "golem-it:agent-rpc-rust",
    )
    .await
    .unwrap();
    let registry = deps.registry_service().client(&http.user.token).await;
    let components = registry
        .list_environment_components(&http.env_id.0)
        .await
        .unwrap();
    let component_id = components
        .values
        .into_iter()
        .find(|component| component.component_name.0 == "golem-it:agent-rpc-rust")
        .expect("raw HTTP router component was not found")
        .id;

    RawHttpProcessLossContext { http, component_id }
}

async fn open_streaming_echo(
    context: &RawHttpProcessLossContext,
    base_url: reqwest::Url,
) -> anyhow::Result<(mpsc::Sender<Result<Vec<u8>, std::io::Error>>, Response)> {
    let (tx, rx) = mpsc::channel(2);
    tx.send(Ok(b"first-chunk".to_vec())).await?;
    let mut response = tokio::time::timeout(
        Duration::from_secs(10),
        context
            .http
            .client
            .request(Method::POST, base_url.join("/raw/echo")?)
            .body(Body::wrap_stream(ReceiverStream::new(rx)))
            .send(),
    )
    .await??;
    anyhow::ensure!(response.status() == StatusCode::OK);
    let echoed = tokio::time::timeout(Duration::from_secs(10), response.chunk()).await??;
    anyhow::ensure!(echoed.as_deref() == Some(b"first-chunk".as_slice()));
    Ok((tx, response))
}

async fn agent_count(context: &RawHttpProcessLossContext) -> anyhow::Result<usize> {
    let (_, agents) = context
        .http
        .user
        .get_workers_metadata(
            &context.component_id,
            Some(AgentFilter::new_mode(
                FilterComparator::Equal,
                AgentMode::Ephemeral,
            )),
            ScanCursor::default(),
            100,
            true,
        )
        .await?;
    Ok(agents.len())
}

#[test]
#[timeout("2m")]
async fn lifecycle_executor_loss_after_head(
    context: &RawHttpProcessLossContext,
    cluster: &WorkerExecutorClusterControlStub,
) -> anyhow::Result<()> {
    let (_request_body, mut response) =
        open_streaming_echo(context, context.http.base_url.clone()).await?;
    let before = agent_count(context).await?;
    anyhow::ensure!(before > 0, "active ephemeral was absent from enumeration");

    cluster
        .kill_all_and_wait(60_000)
        .await
        .map_err(anyhow::Error::msg)?;
    let result = async {
        let terminal = tokio::time::timeout(Duration::from_secs(15), response.chunk()).await;
        Ok::<_, anyhow::Error>((terminal, before))
    }
    .await;
    cluster.restart_all().await;

    let (terminal, before) = result?;
    let terminal =
        terminal.map_err(|_| anyhow::anyhow!("response did not abort after executor loss"))?;
    anyhow::ensure!(
        terminal.is_err(),
        "executor loss returned successful EOF: {terminal:?}"
    );
    anyhow::ensure!(
        agent_count(context).await? == before,
        "request was dispatched to a replacement agent"
    );
    Ok(())
}

async fn free_ports() -> anyhow::Result<[u16; 4]> {
    loop {
        let http = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let grpc = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let custom = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let custom_port = custom.local_addr()?.port();
        let Some(mcp_port) = custom_port.checked_add(2) else {
            continue;
        };
        let Ok(mcp) = tokio::net::TcpListener::bind(("127.0.0.1", mcp_port)).await else {
            continue;
        };
        let ports = [
            http.local_addr()?.port(),
            grpc.local_addr()?.port(),
            custom_port,
            mcp.local_addr()?.port(),
        ];
        drop((http, grpc, custom, mcp));
        return Ok(ports);
    }
}

fn worker_service_binary() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"))
        .join("debug/golem-worker-service")
}

async fn spawn_worker_service(
    deps: &EnvBasedTestDependencies,
    ports: [u16; 4],
    extra_env_vars: &[(String, String)],
) -> SpawnedWorkerService {
    SpawnedWorkerService::new(
        &worker_service_binary(),
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../golem-worker-service"),
        ports[0],
        ports[1],
        ports[2],
        ports[3],
        &deps.shard_manager(),
        &deps.rdb(),
        &deps.redis(),
        Level::DEBUG,
        Level::INFO,
        Level::ERROR,
        &deps.registry_service(),
        true,
        false,
        extra_env_vars,
    )
    .await
}

#[test]
#[timeout("2m")]
async fn request_body_idle_timeout_is_catchable_without_interrupting_the_owner(
    context: &RawHttpProcessLossContext,
) -> anyhow::Result<()> {
    use golem_common::model::AgentId;
    use golem_common::model::oplog::{OplogIndex, PublicOplogEntry};

    let ports = free_ports().await?;
    let _service = spawn_worker_service(
        &context.http.user.deps,
        ports,
        &[(
            "GOLEM__HTTP_SESSION__REQUEST_BODY_IDLE_TIMEOUT".into(),
            "2s".into(),
        )],
    )
    .await;
    let url = reqwest::Url::parse(&format!("http://127.0.0.1:{}/raw/catch-input", ports[2]))?;
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(2);
    tx.send(Ok(b"before-timeout".to_vec())).await?;
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        context
            .http
            .client
            .post(url)
            .body(Body::wrap_stream(ReceiverStream::new(rx)))
            .send(),
    )
    .await??;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let owner = AgentId {
        component_id: context.component_id,
        agent_id: response.headers()["x-echo-agent"].to_str()?.to_string(),
    };
    assert_eq!(
        response.bytes().await?,
        b"before-timeout|HTTP request body idle timeout|handled".as_slice()
    );
    drop(tx);
    let oplog = context
        .http
        .user
        .get_oplog(&owner, OplogIndex::INITIAL)
        .await?;
    assert!(
        oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_)))
    );
    assert!(
        !oplog
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Interrupted(_))),
        "request-body failure must not interrupt its owner"
    );
    Ok(())
}

#[test]
#[timeout("2m")]
async fn steady_request_body_preserves_bytes_over_many_idle_intervals(
    context: &RawHttpProcessLossContext,
) -> anyhow::Result<()> {
    let ports = free_ports().await?;
    let _service = spawn_worker_service(
        &context.http.user.deps,
        ports,
        &[(
            "GOLEM__HTTP_SESSION__REQUEST_BODY_IDLE_TIMEOUT".into(),
            "2s".into(),
        )],
    )
    .await;
    let url = reqwest::Url::parse(&format!("http://127.0.0.1:{}/raw/echo", ports[2]))?;
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    tx.send(Ok(b"first".to_vec())).await?;
    let mut response = tokio::time::timeout(
        Duration::from_secs(20),
        context
            .http
            .client
            .post(url)
            .body(Body::wrap_stream(ReceiverStream::new(rx)))
            .send(),
    )
    .await??;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.chunk().await?.unwrap(), b"first".as_slice());
    let mut expected = Vec::new();
    for i in 0..24 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let chunk = format!("-{i:02}-payload").into_bytes();
        expected.extend_from_slice(&chunk);
        tx.send(Ok(chunk)).await?;
    }
    drop(tx);
    assert_eq!(response.bytes().await?, expected);
    Ok(())
}

async fn explicit_cancellations(metrics_port: u16) -> anyhow::Result<u64> {
    let metrics = reqwest::get(format!("http://127.0.0.1:{metrics_port}/metrics"))
        .await?
        .text()
        .await?;
    Ok(metrics
        .lines()
        .find_map(|line| line.strip_prefix("http_session_explicit_cancellations_total "))
        .map(|value| value.trim().parse::<f64>())
        .transpose()?
        .unwrap_or_default() as u64)
}

#[test]
#[timeout("2m")]
async fn unread_incomplete_body_does_not_wait_for_acceptance_through_an_outage(
    context: &RawHttpProcessLossContext,
    cluster: &WorkerExecutorClusterControlStub,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let ports = free_ports().await?;
    let _service = spawn_worker_service(
        &context.http.user.deps,
        ports,
        &[(
            "GOLEM__HTTP_SESSION__REQUEST_BODY_IDLE_TIMEOUT".into(),
            "2s".into(),
        )],
    )
    .await;
    cluster
        .kill_all_and_wait(60_000)
        .await
        .map_err(anyhow::Error::msg)?;
    let result = async {
        let request = format!(
            "POST /raw/echo HTTP/1.1\r\nHost: {}\r\nContent-Length: 100\r\n\r\nx",
            context.http.host_header.to_str()?
        );
        let mut connected = TcpStream::connect(("127.0.0.1", ports[2])).await?;
        connected.write_all(request.as_bytes()).await?;
        let mut disconnected = TcpStream::connect(("127.0.0.1", ports[2])).await?;
        disconnected.write_all(request.as_bytes()).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
        drop(disconnected);

        let mut head = Vec::new();
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut buffer = [0u8; 1024];
            while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = connected.read(&mut buffer).await?;
                anyhow::ensure!(read > 0, "connection closed before a response head");
                head.extend_from_slice(&buffer[..read]);
            }
            Ok(())
        })
        .await
        .map_err(|_| anyhow::anyhow!("an unread body waited for acceptance without a bound"))??;
        anyhow::ensure!(
            head.starts_with(b"HTTP/1.1 504"),
            "unexpected response: {}",
            String::from_utf8_lossy(&head)
        );

        // The disconnected client sees no response, so the ended sessions are counted instead.
        tokio::time::timeout(Duration::from_secs(20), async {
            while explicit_cancellations(ports[0]).await? < 2 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .map_err(|_| anyhow::anyhow!("a disconnected request kept its session"))??;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cluster.restart_all().await;
    result
}

#[test]
#[timeout("3m")]
async fn lifecycle_serving_process_loss(context: &RawHttpProcessLossContext) -> anyhow::Result<()> {
    let deps = &context.http.user.deps;
    let ports = free_ports().await?;
    let service = spawn_worker_service(deps, ports, &[]).await;
    let base_url = reqwest::Url::parse(&format!("http://127.0.0.1:{}", ports[2]))?;
    let (_request_body, mut response) = open_streaming_echo(context, base_url).await?;
    let before = agent_count(context).await?;
    anyhow::ensure!(before > 0, "active ephemeral was absent from enumeration");

    service.kill().await;
    let terminal = tokio::time::timeout(Duration::from_secs(10), response.chunk()).await;
    let replacement = spawn_worker_service(deps, ports, &[]).await;

    let terminal = terminal
        .map_err(|_| anyhow::anyhow!("response did not abort after serving process loss"))?;
    anyhow::ensure!(
        terminal.is_err(),
        "serving process loss returned successful EOF: {terminal:?}"
    );
    anyhow::ensure!(
        agent_count(context).await? == before,
        "request was dispatched to a replacement agent"
    );
    drop(replacement);
    Ok(())
}
