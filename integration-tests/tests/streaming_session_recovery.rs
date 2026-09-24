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

test_r::enable!();

#[test_r::sequential]
mod tests {
    use anyhow::ensure;
    use golem_api_grpc::proto::golem::worker::ResumeOperation;
    use golem_common::model::oplog::{OplogIndex, PublicAgentInvocation, PublicOplogEntry};
    use golem_common::model::{AgentId, PromiseId};
    use golem_common::schema::SchemaValue;
    use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
    use golem_common::{agent_id, data_value};
    use golem_test_framework::config::{
        DbType, EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    };
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};
    use integration_tests::benchmarks::streaming_recovery::{Topology, leaves, prefix};
    use integration_tests::invocation_session::InvocationSession;
    use std::sync::Once;
    use std::time::Duration;
    use test_r::{test, timeout};

    const PHASE: Duration = Duration::from_secs(120);
    static TRACING: Once = Once::new();

    async fn dependencies() -> EnvBasedTestDependencies {
        TRACING.call_once(|| {
            init_tracing_with_default_debug_env_filter(
                &TracingConfig::test_pretty_without_time("streaming-session-recovery")
                    .with_env_overrides(),
            )
        });
        // These tests own the processes, outside the shared API suite: killing a cluster must
        // not interrupt unrelated tests. All processes are managed by the test framework.
        EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
            worker_executor_cluster_size: 1,
            db_type: DbType::Sqlite,
            ..EnvBasedTestDependenciesConfig::new()
        })
        .await
        .expect("streaming recovery dependencies")
    }

    async fn stop(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
        let cluster = deps.worker_executor_cluster();
        cluster
            .kill_all_and_wait(tokio::time::Instant::now() + PHASE)
            .await?;
        ensure!(cluster.all_reaped(), "executor exit was not confirmed");
        Ok(())
    }

    async fn restart(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
        tokio::time::timeout(PHASE, async {
            deps.worker_executor_cluster().restart_all().await;
            loop {
                let table = deps.shard_manager().get_routing_table().await?;
                let executors = deps.worker_executor_cluster().to_vec();
                if table.all().len() == executors.len()
                    && executors.iter().all(|executor| {
                        table
                            .all()
                            .iter()
                            .any(|pod| pod.port == executor.grpc_port())
                    })
                {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await??;
        Ok(())
    }

    async fn scenario(topology: Topology, second_restart: bool) -> anyhow::Result<()> {
        let deps = dependencies().await;
        let user = deps.user().await?;
        let (_, environment) = user.app_and_env().await?;
        let component = user
            .component(&environment.id, "golem_it_agent_rpc_rust_release")
            .name("golem-it:agent-rpc-rust")
            .unique()
            .store()
            .await?;
        let method = match topology {
            Topology::Flat => "benchmark_gated_output",
            Topology::Siblings => "benchmark_gated_siblings",
            Topology::Nested => "benchmark_gated_nested_siblings",
        };
        let mut sessions = Vec::new();
        let mut agents = Vec::new();
        let mut gates = Vec::new();
        let mut expectations = Vec::new();
        let mut stream_ids = Vec::new();
        for branch in 0..if topology == Topology::Flat { 2 } else { 1 } {
            let agent = agent_id!("StreamingRpcTarget", uuid::Uuid::new_v4().to_string());
            let left = user
                .invoke_and_await_agent(&component, &agent, "create_output_gate", data_value!())
                .await?
                .into_typed::<PromiseId>()?;
            let (input, expected) = if topology == Topology::Flat {
                let length = 16 + branch * 8;
                let domain = 701 + branch * 10_000;
                (
                    data_value!(length, domain, left.clone()),
                    vec![(domain, length)],
                )
            } else {
                let right = user
                    .invoke_and_await_agent(&component, &agent, "create_output_gate", data_value!())
                    .await?
                    .into_typed::<PromiseId>()?;
                gates.push(right.clone());
                (
                    data_value!(8u32, left.clone(), right),
                    vec![(1000, 8), (100_000, 11)],
                )
            };
            gates.push(left);
            let mut session =
                InvocationSession::start(&deps, &component, &agent, method, input, PHASE).await?;
            stream_ids.push(prefix(&mut session, topology, &expected).await?);
            expectations.push(expected);
            agents.push(agent);
            sessions.push(session);
        }
        // Both flat attachments (or all sibling branches) are active and gated at SIGKILL.
        stop(&deps).await?;
        let checkpoints: Vec<_> = sessions
            .into_iter()
            .map(InvocationSession::disconnect)
            .collect();
        restart(&deps).await?;
        let resumed = futures::future::try_join_all(checkpoints.into_iter().map(|checkpoint| {
            InvocationSession::resume(&deps, checkpoint, ResumeOperation::Takeover, PHASE)
        }))
        .await?;
        // Every Takeover has been accepted and its epoch checked before either gate is released.
        for gate in gates.iter().rev() {
            user.complete_promise(gate, Vec::new()).await?;
        }
        let reports =
            futures::future::try_join_all(resumed.into_iter().map(InvocationSession::finish))
                .await?;
        let mut histories = Vec::new();
        for (index, report) in reports.iter().enumerate() {
            assert_eq!(leaves(report, topology)?, stream_ids[index]);
            assert_eq!(
                report.outputs.len(),
                if topology == Topology::Nested {
                    4
                } else {
                    expectations[index].len()
                }
            );
            for (id, (domain, length)) in stream_ids[index].iter().zip(&expectations[index]) {
                let output = &report.outputs[id];
                let values = output
                    .values()?
                    .into_iter()
                    .map(|value| SchemaValue::try_from(value).map_err(anyhow::Error::msg))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                assert_eq!(
                    values,
                    (0..*length)
                        .map(|i| SchemaValue::U32(domain + i * 3))
                        .collect::<Vec<_>>()
                );
                assert_eq!(
                    output
                        .items
                        .iter()
                        .filter(|item| item.epoch == report.acceptance.as_ref().unwrap().epoch)
                        .count(),
                    (length - (length / 4).clamp(1, 8)) as usize
                );
                let events = &report.attempts[1];
                assert!(events.sent <= events.accepted.unwrap());
                assert!(events.accepted.unwrap() <= events.first_items[id]);
                assert!(events.first_items[id] <= events.completed.unwrap());
            }
            let worker =
                AgentId::from_agent_id(component.id, &agents[index]).map_err(anyhow::Error::msg)?;
            let history = user.get_oplog(&worker, OplogIndex::INITIAL).await?;
            let key = &report
                .acceptance
                .as_ref()
                .unwrap()
                .idempotency_key
                .as_ref()
                .unwrap()
                .value;
            assert_eq!(history.iter().filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::AgentInvocationStarted(start)
                    if matches!(&start.invocation, PublicAgentInvocation::AgentMethodInvocation(invocation)
                        if &invocation.idempotency_key.value == key)
            )).count(), 1);
            assert_eq!(history.iter().filter(|entry| matches!(&entry.entry,
                PublicOplogEntry::AgentInvocationFinished(finish) if finish.method_name.as_deref() == Some(method)
            )).count(), 1);
            histories.push((worker, history));
        }
        if second_restart {
            stop(&deps).await?;
            restart(&deps).await?;
            // One default reconciliation interval plus slack, before any new guest demand.
            tokio::time::sleep(Duration::from_secs(35)).await;
            for ((worker, before), agent) in histories.iter().zip(&agents) {
                assert_eq!(
                    user.invoke_and_await_agent(&component, agent, "ping", data_value!())
                        .await?
                        .into_typed::<u64>()?,
                    42
                );
                let after = user.get_oplog(worker, OplogIndex::INITIAL).await?;
                let session_records =
                    |history: &[golem_common::model::oplog::PublicOplogEntryWithIndex]| {
                        history
                            .iter()
                            .filter(|entry| {
                                matches!(entry.entry, PublicOplogEntry::StreamSession(_))
                            })
                            .map(|entry| entry.entry.clone())
                            .collect::<Vec<_>>()
                    };
                assert_eq!(
                    session_records(before),
                    session_records(&after),
                    "finalized sessions acquired new lifecycle records after a second restart"
                );
                assert_eq!(after.iter().filter(|entry| matches!(&entry.entry,
                    PublicOplogEntry::AgentInvocationFinished(finish) if finish.method_name.as_deref() == Some(method)
                )).count(), 1);
            }
        }
        Ok(())
    }

    #[test]
    #[timeout("8 minutes")]
    async fn durable_streaming_output_recovers_after_hard_crash() -> anyhow::Result<()> {
        scenario(Topology::Flat, false).await
    }

    #[test]
    #[timeout("8 minutes")]
    async fn durable_streaming_output_stays_retired_after_second_restart() -> anyhow::Result<()> {
        scenario(Topology::Flat, true).await
    }

    #[test]
    #[timeout("8 minutes")]
    async fn sibling_output_recovers_mid_production() -> anyhow::Result<()> {
        scenario(Topology::Siblings, false).await
    }

    #[test]
    #[timeout("8 minutes")]
    async fn nested_sibling_output_recovers_mid_production() -> anyhow::Result<()> {
        scenario(Topology::Nested, false).await
    }
}
