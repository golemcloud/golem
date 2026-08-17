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

use golem_api_grpc::proto::golem::worker::v1::worker_service_client::WorkerServiceClient;
use golem_api_grpc::proto::golem::worker::{
    InvocationFrame, InvocationStart, invocation_frame, invocation_result,
    invocation_session_finished,
};
use golem_client::model::ComponentDto;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::{AgentId, IdempotencyKey, RoutingTable};
use golem_common::schema::{SchemaValue, TypedSchemaValue};
use golem_common::{agent_id, data_value};
use golem_service_base::model::auth::AuthCtx;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

inherit_test_dep!(EnvBasedTestDependencies);

fn cross_executor_agent_name(
    component: &ComponentDto,
    routing_table: &RoutingTable,
) -> anyhow::Result<String> {
    for index in 0..10_000 {
        let name = format!("generated-streaming-rpc-cross-{index}");
        let caller = agent_id!("StreamingRpcCaller", name.clone());
        let target = agent_id!("StreamingRpcTarget", name.clone());
        let caller = AgentId::from_agent_id(component.id, &caller)
            .map_err(|error| anyhow::anyhow!("invalid caller agent id: {error}"))?;
        let target = AgentId::from_agent_id(component.id, &target)
            .map_err(|error| anyhow::anyhow!("invalid target agent id: {error}"))?;
        let caller_pod = routing_table
            .lookup(&caller)
            .ok_or_else(|| anyhow::anyhow!("caller agent has no executor assignment"))?;
        let target_pod = routing_table
            .lookup(&target)
            .ok_or_else(|| anyhow::anyhow!("target agent has no executor assignment"))?;
        if caller_pod != target_pod {
            return Ok(name);
        }
    }

    anyhow::bail!("could not find caller and target agent IDs assigned to different executors")
}

async fn invoke_agent_session(
    deps: &EnvBasedTestDependencies,
    component: &ComponentDto,
    agent_id: &ParsedAgentId,
    method_name: &str,
    params: TypedSchemaValue,
) -> anyhow::Result<Result<SchemaValue, String>> {
    let agent_id = AgentId::from_agent_id(component.id, agent_id)
        .map_err(|error| anyhow::anyhow!("invalid agent id: {error}"))?;
    let (_, input) = params.into_parts();
    let input = input.try_into().map_err(anyhow::Error::msg)?;
    let (frames, receiver) = mpsc::channel(8);
    frames
        .send(InvocationFrame {
            frame: Some(invocation_frame::Frame::Start(InvocationStart {
                agent_id: Some(agent_id.into()),
                method_name: Some(method_name.to_string()),
                input: Some(input),
                idempotency_key: Some(IdempotencyKey::fresh().into()),
                context: None,
                auth_ctx: Some(AuthCtx::System.into()),
                principal: None,
                environment_id: None,
                config: Vec::new(),
                component_owner_account_id: None,
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                schedule_at: None,
                freshness_disposition:
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                        as i32,
            })),
        })
        .await?;

    let worker_service = deps.worker_service();
    let mut client = WorkerServiceClient::connect(format!(
        "http://{}:{}",
        worker_service.grpc_host(),
        worker_service.gprc_port()
    ))
    .await?;
    let mut inbound = client
        .invoke_agent_session(ReceiverStream::new(receiver))
        .await?
        .into_inner();
    let mut result = None;
    while let Some(frame) = inbound.message().await? {
        match frame.frame {
            Some(invocation_frame::Frame::Result(value)) => {
                if result.is_some() {
                    anyhow::bail!("invocation session returned more than one result");
                }
                result = match value.result {
                    Some(invocation_result::Result::MethodResult(value)) => {
                        Some(value.try_into().map_err(anyhow::Error::msg)?)
                    }
                    Some(invocation_result::Result::NoResult(_)) | None => {
                        anyhow::bail!("invocation session returned no method result")
                    }
                };
            }
            Some(invocation_frame::Frame::Finished(finished)) => match finished.outcome {
                Some(invocation_session_finished::Outcome::Success(_)) => {
                    return result.map(Ok).ok_or_else(|| {
                        anyhow::anyhow!("invocation session ended without a result")
                    });
                }
                Some(invocation_session_finished::Outcome::Failure(failure)) => {
                    return Ok(Err(format!("{failure:?}")));
                }
                Some(invocation_session_finished::Outcome::ProtocolFailure(failure)) => {
                    return Ok(Err(failure.details));
                }
                None => anyhow::bail!("invocation session completion has no outcome"),
            },
            Some(other) => {
                anyhow::bail!("unexpected outer invocation session frame: {other:?}")
            }
            None => anyhow::bail!("empty outer invocation session frame"),
        }
    }
    anyhow::bail!("invocation session response ended before completion")
}

fn assert_streaming_report(value: SchemaValue) {
    assert_eq!(
        value,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(1),
                        SchemaValue::U32(2),
                        SchemaValue::U32(3),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(4),
                        SchemaValue::U32(5),
                        SchemaValue::U32(6),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::U32(70),
                        SchemaValue::U32(80),
                        SchemaValue::U32(90),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::String("left".to_string()),
                        SchemaValue::String("right".to_string()),
                    ],
                },
                SchemaValue::List {
                    elements: vec![SchemaValue::U32(10), SchemaValue::U32(11)],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::String("first".to_string()),
                        SchemaValue::String("second".to_string()),
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::List {
                            elements: vec![SchemaValue::U32(1), SchemaValue::U32(2)],
                        },
                        SchemaValue::List {
                            elements: vec![
                                SchemaValue::U32(3),
                                SchemaValue::U32(4),
                                SchemaValue::U32(5),
                            ],
                        },
                    ],
                },
                SchemaValue::List {
                    elements: vec![
                        SchemaValue::String("a".to_string()),
                        SchemaValue::String("b".to_string()),
                    ],
                },
                SchemaValue::List {
                    elements: (0..64).map(SchemaValue::U32).collect(),
                },
                SchemaValue::U64(42),
            ],
        }
    );
}

#[test]
#[timeout("4 minutes")]
#[tracing::instrument]
async fn generated_rust_client_streaming_rpc_cross_executor(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let user = deps.user().await?;
    let (_, environment) = user.app_and_env().await?;
    let component = user
        .component(&environment.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;
    let routing_table = deps.shard_manager().get_routing_table().await?;
    let name = cross_executor_agent_name(&component, &routing_table)?;
    let caller_agent_id = agent_id!("StreamingRpcCaller", name);

    let result = invoke_agent_session(deps, &component, &caller_agent_id, "run", data_value!())
        .await?
        .map_err(anyhow::Error::msg)?;
    assert_streaming_report(result);

    let producer_error = invoke_agent_session(
        deps,
        &component,
        &caller_agent_id,
        "call_producer_error",
        data_value!(),
    )
    .await?
    .expect_err("producer stream error must fail the invocation session");
    assert!(
        producer_error.contains("value-node index out of range: 0"),
        "unexpected producer error: {producer_error}"
    );

    let first = user
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "call_stream_free",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    let second = user
        .invoke_and_await_agent(
            &component,
            &caller_agent_id,
            "call_stream_free",
            data_value!(),
        )
        .await?
        .into_typed::<u64>()?;
    assert_eq!((first, second), (1, 2));
    Ok(())
}
