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

use crate::benchmarks::{
    cleanup_user_state, delete_workers, deployment_base_url, deployment_domain,
};
use crate::mcp_client::McpClient;
use async_trait::async_trait;
use futures::{TryStreamExt, stream};
use futures_concurrency::future::Join;
use golem_client::api::RegistryServiceClient;
use golem_common::agent_id;
use golem_common::model::AgentId;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::ComponentDto;
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::mcp_deployment::{McpDeploymentAgentOptions, McpDeploymentCreation};
use golem_test_framework::benchmark::{
    Benchmark, BenchmarkRecorder, BenchmarkResultValue, ResultKey, RunConfig,
};
use golem_test_framework::config::benchmark::TestMode;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use indoc::indoc;
use serde_json::json;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tracing::{Instrument, Level};

/// The kinds of agent whose `echo` method the sessions call as a tool.
#[derive(Clone, Copy)]
enum AgentKind {
    Durable,
    Ephemeral,
}

impl AgentKind {
    const ALL: [Self; 2] = [Self::Durable, Self::Ephemeral];

    /// The agent type of the Rust benchmark component with this kind.
    fn agent_type(self) -> &'static str {
        match self {
            Self::Durable => "RustBenchmarkAgent",
            Self::Ephemeral => "RustEphemeralBenchmarkAgent",
        }
    }

    /// The prefix of this kind's result names.
    fn label(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::Ephemeral => "ephemeral",
        }
    }
}

/// The longest wait for one MCP request, from sending it to the end of its response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

pub struct McpLatency {
    config: RunConfig,
}

#[async_trait]
impl Benchmark for McpLatency {
    type BenchmarkContext = McpLatencyBenchmark;
    type IterationContext = IterationContext;

    fn name() -> &'static str {
        "mcp-latency"
    }

    fn description() -> &'static str {
        indoc! {
            "Benchmarks the latency of MCP sessions through the gateway: `initialize`, `tools/list`,
            and `tools/call` of a durable and of an ephemeral Rust agent method. Each session sends
            its requests one after the other; with several worker-service replicas, the gateway
            decides which replica serves each request.
            The `size` parameter is the number of concurrent MCP sessions. The `length` parameter is
            the number of warm `tools/call` requests per session and agent kind, after the first
            call, which is recorded separately as cold.
            "
        }
    }

    async fn create_benchmark_context(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> BenchmarkResultValue<Self::BenchmarkContext> {
        Ok(McpLatencyBenchmark::new(
            mode,
            verbosity,
            cluster_size,
            disable_compilation_cache,
            otlp,
        )
        .await)
    }

    async fn cleanup(benchmark_context: Self::BenchmarkContext) -> BenchmarkResultValue {
        benchmark_context.deps.kill_all().await;
        Ok(())
    }

    async fn create(_mode: &TestMode, config: RunConfig) -> BenchmarkResultValue<Self> {
        Ok(Self { config })
    }

    async fn setup_iteration(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        _recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue<Self::IterationContext> {
        Ok(benchmark_context.setup_iteration(&self.config).await)
    }

    async fn warmup(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        _context: &Self::IterationContext,
    ) -> BenchmarkResultValue {
        if !self.config.disable_compilation_cache {
            let duration = Duration::from_secs(15);
            tokio::time::sleep(duration)
                .instrument(tracing::info_span!(
                    "wait_compilation_cache",
                    duration_secs = duration.as_secs()
                ))
                .await;
        }
        Ok(())
    }

    async fn run(
        &self,
        benchmark_context: &Self::BenchmarkContext,
        context: &Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        (0..context.sessions)
            .map(|index| run_session(&benchmark_context.http, context, index, &recorder))
            .collect::<Vec<_>>()
            .join()
            .await;
        Ok(())
    }

    async fn cleanup_iteration(
        &self,
        _benchmark_context: &Self::BenchmarkContext,
        context: Self::IterationContext,
        recorder: BenchmarkRecorder,
    ) -> BenchmarkResultValue {
        let durable_agents: Vec<AgentId> = (0..context.sessions)
            .filter_map(|index| {
                AgentId::from_agent_id(
                    context.component.id,
                    &agent_id!(AgentKind::Durable.agent_type(), agent_name(index)),
                )
                .ok()
            })
            .collect();
        delete_workers(&context.user, &durable_agents, &recorder).await;
        cleanup_user_state(&context.user, &context.env_id, &recorder).await;
        Ok(())
    }
}

pub struct McpLatencyBenchmark {
    deps: BenchmarkTestDependencies,
    /// One client for every iteration, so that its connection pool stays warm.
    http: reqwest::Client,
}

pub struct IterationContext {
    user: TestUserContext<BenchmarkTestDependencies>,
    component: ComponentDto,
    env_id: EnvironmentId,
    domain: Domain,
    url: Box<str>,
    sessions: usize,
    warm_calls: usize,
}

impl McpLatencyBenchmark {
    async fn new(
        mode: &TestMode,
        verbosity: Level,
        cluster_size: usize,
        disable_compilation_cache: bool,
        otlp: bool,
    ) -> Self {
        Self {
            deps: BenchmarkTestDependencies::new(
                mode,
                verbosity,
                cluster_size,
                disable_compilation_cache,
                otlp,
            )
            .await,
            http: reqwest::Client::builder()
                .pool_max_idle_per_host(1024)
                .tcp_nodelay(true)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("Failed to build the MCP HTTP client"),
        }
    }

    /// An environment with the Rust benchmark agents, served as an MCP deployment on its own
    /// domain.
    async fn setup_iteration(&self, config: &RunConfig) -> IterationContext {
        let user = self.deps.user().await.unwrap();
        let (_, env) = user.app_and_env().await.unwrap();
        let component = user
            .component(&env.id, "benchmark_agent_rust_release")
            .name("benchmark:agent-rust")
            .store()
            .await
            .unwrap();

        let domain = deployment_domain(&self.deps, &env.id);
        let client = user.registry_service_client().await;
        async {
            client
                .create_domain_registration(
                    &env.id.0,
                    &DomainRegistrationCreation {
                        domain: domain.clone(),
                    },
                )
                .await
                .expect("Failed to register the MCP domain");
            client
                .create_mcp_deployment(
                    &env.id.0,
                    &McpDeploymentCreation {
                        domain: domain.clone(),
                        tools: BTreeMap::new(),
                        agents: AgentKind::ALL
                            .iter()
                            .map(|kind| {
                                (
                                    AgentTypeName(kind.agent_type().to_string()),
                                    McpDeploymentAgentOptions::default(),
                                )
                            })
                            .collect(),
                    },
                )
                .await
                .expect("Failed to create the MCP deployment");
            user.deploy_environment(env.id)
                .await
                .expect("Failed to deploy the environment");
        }
        .instrument(tracing::info_span!("create_mcp_deployment"))
        .await;

        let url = format!("{}/mcp", deployment_base_url(&self.deps, &domain));

        IterationContext {
            user,
            component,
            env_id: env.id,
            domain,
            url: url.into_boxed_str(),
            sessions: config.size,
            warm_calls: config.length,
        }
    }
}

fn agent_name(session: usize) -> String {
    format!("mcp-{session}")
}

/// A request that did not succeed, and the result it belongs to.
struct Failure {
    key: ResultKey,
    message: String,
}

/// Runs `work` and records its duration under `key` when it succeeds. A failed request is not
/// retried, because a retry would hide a refused session: its duration goes under
/// `{key}-failed`, out of the primary series, and the caller records the failure under `key`.
async fn measured<T>(
    recorder: &BenchmarkRecorder,
    key: &str,
    work: impl Future<Output = anyhow::Result<T>>,
) -> Result<T, Failure> {
    let start = Instant::now();
    let result = work.await;
    let elapsed = start.elapsed();
    match result {
        Ok(value) => {
            recorder.duration(&key.into(), elapsed);
            Ok(value)
        }
        Err(error) => {
            recorder.duration(&format!("{key}-failed").into(), elapsed);
            Err(Failure {
                key: key.into(),
                message: format!("{error:#}"),
            })
        }
    }
}

/// One MCP session: `initialize`, `notifications/initialized`, `tools/list`, then for each
/// agent kind one cold and `warm_calls` warm `tools/call` requests. The first failed request ends
/// the session and is recorded as a failure of its result.
async fn run_session(
    http: &reqwest::Client,
    context: &IterationContext,
    index: usize,
    recorder: &BenchmarkRecorder,
) {
    if let Err(failure) = session_requests(http, context, index, recorder)
        .instrument(tracing::info_span!("mcp_session", index))
        .await
    {
        recorder.failure(&failure.key, failure.message);
    }
}

async fn session_requests(
    http: &reqwest::Client,
    context: &IterationContext,
    index: usize,
    recorder: &BenchmarkRecorder,
) -> Result<(), Failure> {
    let client = measured(recorder, "initialize", async {
        let client = McpClient::initialize(http.clone(), &context.url, &context.domain.0).await?;
        anyhow::ensure!(
            client.session_id().is_some(),
            "initialize issued no session ID"
        );
        Ok(client)
    })
    .await?;
    measured(
        recorder,
        "initialized-notification",
        client.send_initialized_notification(),
    )
    .await?;
    measured(recorder, "tools-list", client.list_tools()).await?;

    let name = agent_name(index);
    stream::iter(AgentKind::ALL.map(Ok))
        .try_for_each(|kind| {
            let (client, name) = (&client, &name);
            async move {
                measured(
                    recorder,
                    &format!("{}-cold-tools-call", kind.label()),
                    echo(client, kind, name),
                )
                .await?;
                let warm = format!("{}-warm-tools-call", kind.label());
                stream::iter((0..context.warm_calls).map(Ok))
                    .try_for_each(|_| measured(recorder, &warm, echo(client, kind, name)))
                    .await
            }
        })
        .await
}

/// Calls the `echo` method of the agent `name` of `kind` as an MCP tool.
async fn echo(client: &McpClient, kind: AgentKind, name: &str) -> anyhow::Result<()> {
    let result = client
        .call_tool(
            &format!("{}-echo", kind.agent_type()),
            json!({"name": name, "message": "benchmark"}),
        )
        .await?;
    anyhow::ensure!(
        result["isError"] == json!(false),
        "the tool call failed: {result}"
    );
    Ok(())
}
