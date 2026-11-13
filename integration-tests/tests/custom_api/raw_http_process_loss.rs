use crate::http_test_context::{HttpTestContext, make_test_context};
use golem_client::api::RegistryServiceClient;
use golem_common::base_model::ScanCursor;
use golem_common::base_model::agent::{AgentMode, AgentTypeName};
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::{AgentFilter, FilterComparator};
use golem_test_framework::components::worker_service::WorkerService;
use golem_test_framework::components::worker_service::restartable::RestartableWorkerService;
use golem_test_framework::config::{
    EnvBasedTestDependencies, TestDependencies, WorkerExecutorClusterControl,
    WorkerExecutorClusterControlStub,
};
use golem_test_framework::dsl::TestDsl;
use reqwest::{Body, Method, Response, StatusCode};
use std::time::Duration;
use test_r::{inherit_test_dep, test, test_dep, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

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

#[test]
#[timeout("3m")]
async fn lifecycle_serving_process_loss(context: &RawHttpProcessLossContext) -> anyhow::Result<()> {
    let deps = &context.http.user.deps;
    let service = RestartableWorkerService::start(deps).await?;
    let base_url = reqwest::Url::parse(&format!(
        "http://127.0.0.1:{}",
        service.custom_request_port()
    ))?;
    let (_request_body, mut response) = open_streaming_echo(context, base_url).await?;
    let before = agent_count(context).await?;
    anyhow::ensure!(before > 0, "active ephemeral was absent from enumeration");

    service.kill().await;
    let terminal = tokio::time::timeout(Duration::from_secs(10), response.chunk()).await;
    service.restart().await;

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
    Ok(())
}
