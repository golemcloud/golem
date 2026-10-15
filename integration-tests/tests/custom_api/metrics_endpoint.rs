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

//! The `/metrics` endpoint of the standalone worker-service process.
//!
//! The application metrics of the worker-service register in the process-wide default registry.
//! The endpoint must export that registry, together with the Tokio runtime metrics, after the
//! service has served REST API, inbound and outbound gRPC, HTTP session and durable stream traffic.

use crate::custom_api::http_test_context::{HttpTestContext, make_test_context};
use golem_api_grpc::proto::golem::worker::v1::CompletePromiseRequest;
use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::oplog::OplogIndex;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

inherit_test_dep!(EnvBasedTestDependencies);

async fn context(
    deps: &EnvBasedTestDependencies,
    agent_type: &str,
    component_name: &str,
    package_name: &str,
) -> anyhow::Result<HttpTestContext> {
    make_test_context(
        deps,
        vec![(
            AgentTypeName(agent_type.to_string()),
            HttpApiDeploymentAgentOptions::default(),
        )],
        component_name,
        package_name,
    )
    .await
}

/// Appends to a durable input stream, closes it and reads the echoed output stream. The agent
/// reads its input from the worker-service over gRPC.
async fn stream_traffic(streams: &HttpTestContext) -> anyhow::Result<()> {
    let session = Uuid::new_v4().to_string();
    let path = |slot: &str| {
        format!("/durable-stream-agents/ds3-{session}/echo/invocations/{session}/streams/{slot}")
    };

    let appended = streams
        .client
        .post(streams.base_url.join(&path("input"))?)
        .header(CONTENT_TYPE, "application/json")
        .json(&serde_json::json!(["a", "b"]))
        .header("stream-closed", "true")
        .send()
        .await?;
    assert_eq!(appended.status(), StatusCode::NO_CONTENT);

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let response = streams
                .client
                .head(streams.base_url.join(&path("output"))?)
                .send()
                .await?;
            if response
                .headers()
                .get("stream-closed")
                .is_some_and(|value| value == "true")
            {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    Ok(())
}

/// Starts a raw HTTP request, waits for its response head and drops the connection, which cancels
/// the HTTP session behind it.
async fn cancelled_session_traffic(router: &HttpTestContext) -> anyhow::Result<()> {
    let mut socket =
        tokio::net::TcpStream::connect(("127.0.0.1", router.base_url.port().unwrap())).await?;
    socket
        .write_all(
            format!(
                "POST /raw/slow HTTP/1.1\r\nHost: {}\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n",
                router.host_header.to_str()?
            )
            .as_bytes(),
        )
        .await?;
    let mut head = Vec::new();
    let mut byte = [0];
    tokio::time::timeout(Duration::from_secs(20), async {
        while !head.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await?;
            head.push(byte[0]);
        }
        anyhow::Ok(())
    })
    .await??;
    assert!(head.starts_with(b"HTTP/1.1 200 "));
    drop(socket);
    Ok(())
}

/// Sends a request without an agent to the gRPC API of the worker-service. The service records it as
/// a failed `complete_promise` request.
async fn grpc_traffic(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
    let worker_service = deps.worker_service();
    let mut client = WorkerServiceClient::connect(format!(
        "http://{}:{}",
        worker_service.grpc_host(),
        worker_service.gprc_port()
    ))
    .await?;
    let response = client
        .complete_promise(CompletePromiseRequest::default())
        .await?
        .into_inner();
    assert!(
        matches!(
            response.result,
            Some(
                golem_api_grpc::proto::golem::worker::v1::complete_promise_response::Result::Error(
                    _
                )
            )
        ),
        "a request without an agent must be refused"
    );
    Ok(())
}

async fn scrape(deps: &EnvBasedTestDependencies) -> anyhow::Result<String> {
    let worker_service = deps.worker_service();
    let response = reqwest::get(format!(
        "http://{}:{}/metrics",
        worker_service.http_host(),
        worker_service.http_port()
    ))
    .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(response.text().await?)
}

/// The sample lines of `family`: the exact name, or its histogram `_count` series, followed by
/// labels or a value.
fn samples<'a>(text: &'a str, family: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter(|line| {
            line.strip_prefix(family)
                .is_some_and(|rest| rest.starts_with('{') || rest.starts_with(' '))
        })
        .collect()
}

fn has_sample(text: &str, family: &str, labels: &[&str]) -> bool {
    samples(text, family)
        .iter()
        .any(|line| labels.iter().all(|label| line.contains(label)))
}

/// Each expectation is a family and labels that one of its samples must carry.
const EXPECTED: &[(&str, &[&str])] = &[
    ("version_info", &[]),
    ("api_success_seconds_count", &["api_type=\"http\""]),
    (
        "api_failure_seconds_count",
        &["api=\"complete_promise\"", "api_type=\"grpc\""],
    ),
    ("internal_grpc_success_seconds_count", &[]),
    ("durable_stream_append_total", &["outcome=\"accepted\""]),
    ("http_session_terminal_total", &["cause=\"cancelled\""]),
    ("http_session_explicit_cancellations_total", &[]),
    ("openapi_cache_total", &[]),
];

#[test]
#[timeout("180s")]
async fn standalone_worker_service_exports_application_metrics(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let streams = context(
        deps,
        "DurableStreamAgent",
        "golem_it_agent_sdk_rust_release",
        "golem-it:agent-sdk-rust",
    )
    .await?;
    let router = context(
        deps,
        "RawHttpRouter",
        "golem_it_agent_rpc_rust_release",
        "golem-it:agent-rpc-rust",
    )
    .await?;

    stream_traffic(&streams).await?;
    cancelled_session_traffic(&router).await?;
    grpc_traffic(deps).await?;

    let openapi = streams
        .client
        .get(streams.base_url.join("/openapi.json")?)
        .send()
        .await?;
    assert_eq!(openapi.status(), StatusCode::OK);

    let agent_id = streams
        .user
        .get_workers_metadata(&streams.component_id, None, Default::default(), 1, true)
        .await?
        .1
        .into_iter()
        .next()
        .expect("the stream traffic created an agent")
        .agent_id;
    streams
        .user
        .get_oplog(&agent_id, OplogIndex::INITIAL)
        .await?;

    // The session cancellation is recorded after the connection closes, so the endpoint is polled.
    let text = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let text = scrape(deps).await?;
            if EXPECTED
                .iter()
                .all(|(family, labels)| has_sample(&text, family, labels))
            {
                return anyhow::Ok(text);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;

    let text = match text {
        Ok(text) => text?,
        Err(_) => {
            let text = scrape(deps).await?;
            let missing: Vec<_> = EXPECTED
                .iter()
                .filter(|(family, labels)| !has_sample(&text, family, labels))
                .collect();
            panic!("the worker-service /metrics endpoint lacks {missing:?}:\n{text}");
        }
    };

    assert!(
        text.lines().any(|line| line.starts_with("tokio_")),
        "the runtime metrics must still be exported:\n{text}"
    );
    Ok(())
}
