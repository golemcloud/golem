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

//! The PR subset of the streaming fault matrix, and the self-checks of its injectors.
//!
//! Each row runs one fault against one path for each fixture in
//! [`integration_tests::fault_matrix::FIXTURES`] that supports the path, asserts the path
//! guarantee with the harness assertions, and asserts that the injector had its effect.
//! The suite owns its processes: the rows kill executors and worker services.

#[path = "custom_api/http_test_context.rs"]
mod http_test_context;

test_r::enable!();

#[test_r::sequential]
mod rows {
    use crate::http_test_context::{HttpTestContext, make_test_context};
    use anyhow::{Context, ensure};
    use golem_api_grpc::proto::golem::worker::{
        ResumeOperation, invocation_response, invocation_session_completion,
        invocation_session_result, stream_terminal,
    };
    use golem_client::model::ComponentDto;
    use golem_common::base_model::agent::AgentTypeName;
    use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
    use golem_common::model::agent::ParsedAgentKey;
    use golem_common::model::{AgentId, PromiseId, RoutingTable};
    use golem_common::schema::SchemaValue;
    use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
    use golem_common::{agent_id, data_value};
    use golem_schema::proto::golem::schema::{SchemaValue as ProtoValue, schema_value};
    use golem_test_framework::components::worker_service::WorkerService;
    use golem_test_framework::components::worker_service::relayed::RelayedWorkerService;
    use golem_test_framework::components::worker_service::restartable::RestartableWorkerService;
    use golem_test_framework::config::dsl_impl::TestUserContext;
    use golem_test_framework::config::{
        DbType, EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    };
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};
    use integration_tests::fault_matrix::{
        Delivery, FIXTURE_AGENT_TYPE, FIXTURES, Fixture, InjectorEffect, Terminal,
        assert_fail_clean, assert_resumed,
    };
    use integration_tests::invocation_session::{
        InvocationSession, OutputObservation, SessionCheckpoint, StreamId,
    };
    use reqwest::StatusCode;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    use test_r::{test, test_dep, timeout};

    const PHASE: Duration = Duration::from_secs(120);
    const COUNT: u32 = 12;
    const PAUSE_AFTER: u32 = 4;

    static RUN: AtomicU32 = AtomicU32::new(1);

    fn next_run() -> u32 {
        RUN.fetch_add(1, Ordering::Relaxed)
    }

    pub struct Matrix {
        deps: EnvBasedTestDependencies,
    }

    impl std::fmt::Debug for Matrix {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Matrix")
        }
    }

    #[test_dep(scope = PerWorker)]
    async fn matrix() -> Matrix {
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("fault-matrix").with_env_overrides(),
        );
        // Two executors, so a streaming RPC row can kill the callee's executor alone.
        let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
            worker_executor_cluster_size: 2,
            db_type: DbType::Sqlite,
            ..EnvBasedTestDependenciesConfig::new()
        })
        .await
        .expect("fault matrix dependencies");
        Matrix { deps }
    }

    // ---- Injectors -----------------------------------------------------------------------

    fn executor_pids(deps: &EnvBasedTestDependencies) -> Vec<Option<u32>> {
        deps.worker_executor_cluster()
            .to_vec()
            .iter()
            .map(|executor| executor.pid())
            .collect()
    }

    async fn wait_for_routing(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
        tokio::time::timeout(PHASE, async {
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
        .await?
    }

    /// Executor kill and restart: every executor is killed, reaped and started again.
    async fn kill_executors(deps: &EnvBasedTestDependencies) -> anyhow::Result<Vec<Option<u32>>> {
        let before = executor_pids(deps);
        let cluster = deps.worker_executor_cluster();
        cluster
            .kill_all_and_wait(tokio::time::Instant::now() + PHASE)
            .await?;
        ensure!(cluster.all_reaped(), "executor exit was not confirmed");
        Ok(before)
    }

    async fn restart_executors(
        deps: &EnvBasedTestDependencies,
        before: Vec<Option<u32>>,
    ) -> anyhow::Result<InjectorEffect> {
        deps.worker_executor_cluster().restart_all().await;
        wait_for_routing(deps).await?;
        Ok(InjectorEffect::ExecutorRestarted {
            before,
            after: executor_pids(deps),
        })
    }

    // ---- Fixture access ------------------------------------------------------------------

    struct Row {
        sdk: &'static str,
        user: TestUserContext<EnvBasedTestDependencies>,
        component: ComponentDto,
    }

    async fn row(deps: &EnvBasedTestDependencies, fixture: Fixture) -> anyhow::Result<Row> {
        let user = deps.user().await?;
        let (_, environment) = user.app_and_env().await?;
        let component = user
            .component(&environment.id, fixture.wasm)
            .name(fixture.component)
            .unique()
            .store()
            .await?;
        Ok(Row {
            sdk: fixture.sdk,
            user,
            component,
        })
    }

    fn agent(name: &str) -> ParsedAgentKey {
        agent_id!(FIXTURE_AGENT_TYPE, name.to_string())
    }

    fn fresh_agent() -> ParsedAgentKey {
        agent(&uuid::Uuid::new_v4().to_string())
    }

    async fn gate(row: &Row, agent: &ParsedAgentKey) -> anyhow::Result<PromiseId> {
        row.user
            .invoke_and_await_agent(&row.component, agent, "create_gate", data_value!())
            .await?
            .into_typed::<PromiseId>()
            .with_context(|| format!("{}: create_gate", row.sdk))
    }

    fn root(report: &SessionCheckpoint) -> anyhow::Result<StreamId> {
        let Some(invocation_session_result::Result::MethodResult(value)) = report
            .result
            .as_ref()
            .and_then(|result| result.result.as_ref())
        else {
            anyhow::bail!("expected a method result, got {:?}", report.result);
        };
        let Some(schema_value::Value::StreamReference(reference)) = &value.value else {
            anyhow::bail!("expected a stream reference: {value:?}");
        };
        let id = report
            .mappings
            .get(&reference.stream_id)
            .and_then(|mapping| mapping.handle.as_ref())
            .and_then(|handle| handle.stream_id)
            .context("the result stream has no mapping")?;
        Ok((id.high_bits, id.low_bits))
    }

    fn u32_value(value: ProtoValue) -> anyhow::Result<u32> {
        match SchemaValue::try_from(value).map_err(anyhow::Error::msg)? {
            SchemaValue::U32(value) => Ok(value),
            other => anyhow::bail!("expected a u32 record, got {other:?}"),
        }
    }

    fn delivery(output: &OutputObservation) -> anyhow::Result<Delivery> {
        let records = output
            .values()?
            .into_iter()
            .map(u32_value)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let terminal = match output
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.response.as_ref())
        {
            None => Terminal::Open,
            Some(invocation_response::Response::StreamEnd(end)) => match end
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.terminal.as_ref())
            {
                Some(stream_terminal::Terminal::Finished(_)) => Terminal::End,
                Some(stream_terminal::Terminal::Failed(error)) => {
                    Terminal::Failed(format!("{:?}", error.error))
                }
                Some(stream_terminal::Terminal::ConsumerCancelled(cancelled)) => {
                    Terminal::Cancelled(cancelled.details.clone().unwrap_or_default())
                }
                None => anyhow::bail!("output stream end has no outcome"),
            },
            Some(other) => anyhow::bail!("unexpected output terminal {other:?}"),
        };
        Ok(Delivery::new(records, terminal))
    }

    fn records(value: SchemaValue) -> anyhow::Result<Vec<u32>> {
        let SchemaValue::List { elements: values } = value else {
            anyhow::bail!("expected a list of records, got {value:?}");
        };
        values
            .into_iter()
            .map(|value| match value {
                SchemaValue::U32(value) => Ok(value),
                other => anyhow::bail!("expected a u32 record, got {other:?}"),
            })
            .collect()
    }

    /// Starts `produce` over an invocation session and reads until the producer waits at its
    /// gate after `PAUSE_AFTER` records.
    async fn start_gated(
        deps: &EnvBasedTestDependencies,
        row: &Row,
        agent: &ParsedAgentKey,
        run: u32,
    ) -> anyhow::Result<(InvocationSession, PromiseId, StreamId)> {
        let gate = gate(row, agent).await?;
        let mut session = InvocationSession::start(
            deps,
            &row.component,
            agent,
            "produce",
            data_value!(run, COUNT, PAUSE_AFTER, gate.clone()),
            PHASE,
        )
        .await?;
        loop {
            let report = session.checkpoint();
            if report.result.is_some() {
                let id = root(report)?;
                if report
                    .outputs
                    .get(&id)
                    .is_some_and(|output| output.items.len() == PAUSE_AFTER as usize)
                {
                    return Ok((session, gate, id));
                }
            }
            session.receive().await?;
        }
    }

    // ---- Injector self-checks ------------------------------------------------------------

    #[test]
    #[timeout("6 minutes")]
    async fn worker_service_restart_keeps_ports_changes_process_and_spares_the_shared_service(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        let fixture = FIXTURES[0];
        let service = Arc::new(RestartableWorkerService::start(deps).await?);
        let view = deps.with_worker_service(service.clone());
        let private = row(&view, fixture).await?;
        let shared = row(deps, fixture).await?;
        let agent = fresh_agent();
        let ports = (
            service.http_port(),
            service.gprc_port(),
            service.custom_request_port(),
        );
        gate(&private, &agent).await?;
        let before = service
            .pid()
            .context("the private service is not running")?;

        service.kill().await;
        ensure!(
            service.pid().is_none(),
            "the killed service still has a process"
        );
        // The shared worker service of the other tests keeps serving while this one is down.
        gate(&shared, &agent).await?;
        service.restart().await;

        let after = service
            .pid()
            .context("the restarted service is not running")?;
        InjectorEffect::WorkerServiceRestarted { before, after }.ensure_happened()?;
        ensure!(
            ports
                == (
                    service.http_port(),
                    service.gprc_port(),
                    service.custom_request_port()
                ),
            "the restarted service moved to other ports"
        );
        // The client kept its address and reaches the new process.
        gate(&private, &agent).await?;
        gate(&shared, &agent).await?;
        Ok(())
    }

    #[test]
    #[timeout("4 minutes")]
    async fn relay_between_client_and_worker_service_resets_and_reconnects(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        let relayed = Arc::new(RelayedWorkerService::start(deps.worker_service()).await?);
        let view = deps.with_worker_service(relayed.clone());
        let row = row(&view, FIXTURES[0]).await?;
        let agent = fresh_agent();
        let run = next_run();
        let (session, _gate, _) = start_gated(&view, &row, &agent, run).await?;
        let accepted = relayed.accepted_count();
        ensure!(accepted > 0, "the client did not connect through the relay");
        let resets = relayed.drop_connections().await;
        InjectorEffect::ClientDropped { resets }.ensure_happened()?;
        ensure!(relayed.reset_count() == resets);
        // The open session observes the drop as an error, not as a hang or a clean end.
        let mut session = session;
        let observed = tokio::time::timeout(Duration::from_secs(20), session.receive()).await;
        ensure!(
            matches!(observed, Ok(Err(_))),
            "the dropped session did not fail promptly: {observed:?}"
        );
        // New connections succeed after the drop. The gated agent is still busy, so the check
        // invokes another agent.
        gate(&row, &fresh_agent()).await?;
        ensure!(relayed.accepted_count() > accepted);
        Ok(())
    }

    // ---- No-fault runs of each fixture -----------------------------------------------------

    #[test]
    #[timeout("8 minutes")]
    async fn each_fixture_follows_the_contract_without_faults(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        for &fixture in FIXTURES {
            let row = row(deps, fixture).await?;
            let sdk = fixture.sdk;

            // write
            let run = next_run();
            let agent = fresh_agent();
            let gate = gate(&row, &agent).await?;
            let report = InvocationSession::start(
                deps,
                &row.component,
                &agent,
                "produce",
                data_value!(run, COUNT, COUNT, gate),
                PHASE,
            )
            .await?
            .finish()
            .await?;
            let root_id = root(&report).with_context(|| format!("{sdk}: write"))?;
            assert_resumed(run, COUNT, &delivery(&report.outputs[&root_id])?)
                .with_context(|| format!("{sdk}: write"))?;

            // error, on its own agent: an SDK that traps on a failed stream leaves the agent
            // failed
            let run = next_run();
            let report = InvocationSession::start(
                deps,
                &row.component,
                &fresh_agent(),
                "produce_then_fail",
                data_value!(run, PAUSE_AFTER),
                PHASE,
            )
            .await?
            .complete()
            .await?;
            // A producer failure may also fail the invocation, so the session can end without a
            // method result and without an output terminal; the invocation failure is then the
            // terminal.
            let mut failed = match report.outputs.iter().collect::<Vec<_>>()[..] {
                [(_, output)] => delivery(output)?,
                [] => Delivery::new(Vec::new(), Terminal::Open),
                _ => anyhow::bail!(
                    "{sdk}: error: expected one output, got {:?}",
                    report.outputs
                ),
            };
            if let (
                Terminal::Open,
                Some(invocation_session_completion::Outcome::Failure(failure)),
            ) = (
                &failed.terminal,
                report.completion.as_ref().and_then(|c| c.outcome.as_ref()),
            ) {
                failed.terminal = Terminal::Failed(failure.message.clone());
            }
            let delivered =
                assert_fail_clean(run, &failed).with_context(|| format!("{sdk}: error"))?;
            ensure!(
                delivered <= PAUSE_AFTER as usize,
                "{sdk}: error delivered more than the prefix: {failed:?}"
            );

            // cancel
            let run = next_run();
            let (mut session, gate, id) = start_gated(deps, &row, &agent, run).await?;
            session.cancel_output(id).await?;
            row.user.complete_promise(&gate, Vec::new()).await?;
            let report = session.complete().await?;
            let cancelled = delivery(&report.outputs[&id])?;
            ensure!(
                matches!(cancelled.terminal, Terminal::Cancelled(_)),
                "{sdk}: a cancelled output ended with {:?}",
                cancelled.terminal
            );
            assert_fail_clean(run, &cancelled).with_context(|| format!("{sdk}: cancel"))?;

            // read, and drop, through a second fixture agent
            for keep in [COUNT, PAUSE_AFTER] {
                let run = next_run();
                let target = uuid::Uuid::new_v4().to_string();
                let read = records(
                    row.user
                        .invoke_and_await_agent(
                            &row.component,
                            &agent,
                            "pipe",
                            data_value!(target, run, COUNT, keep),
                        )
                        .await?
                        .into_return_value()
                        .context("pipe returned no value")?,
                )?;
                assert_resumed(run, keep, &Delivery::new(read, Terminal::End))
                    .with_context(|| format!("{sdk}: read and drop with keep {keep}"))?;
            }
        }
        Ok(())
    }

    // ---- PR subset: invocation session output ----------------------------------------------

    #[test]
    #[timeout("8 minutes")]
    async fn invocation_session_output_survives_executor_kill(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        for &fixture in FIXTURES {
            let row = row(deps, fixture).await?;
            let agent = fresh_agent();
            let run = next_run();
            let (session, gate, id) = start_gated(deps, &row, &agent, run).await?;
            let before = kill_executors(deps).await?;
            let checkpoint = session.disconnect();
            let effect = restart_executors(deps, before).await?;
            let (resumed, _) =
                InvocationSession::reattach(deps, checkpoint, ResumeOperation::Takeover, PHASE)
                    .await?;
            row.user.complete_promise(&gate, Vec::new()).await?;
            let report = resumed.finish().await?;
            effect.ensure_happened()?;
            assert_resumed(run, COUNT, &delivery(&report.outputs[&id])?)
                .with_context(|| format!("{}: executor kill", fixture.sdk))?;
        }
        Ok(())
    }

    #[test]
    #[timeout("8 minutes")]
    async fn invocation_session_output_survives_worker_service_restart(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        let service = Arc::new(RestartableWorkerService::start(deps).await?);
        let view = deps.with_worker_service(service.clone());
        for &fixture in FIXTURES {
            let row = row(&view, fixture).await?;
            let agent = fresh_agent();
            let run = next_run();
            let (session, gate, id) = start_gated(&view, &row, &agent, run).await?;
            let before = service.pid().context("the service is not running")?;
            service.restart().await;
            let after = service.pid().context("the service did not restart")?;
            let checkpoint = session.disconnect();
            let (resumed, _) =
                InvocationSession::reattach(&view, checkpoint, ResumeOperation::Resume, PHASE)
                    .await?;
            row.user.complete_promise(&gate, Vec::new()).await?;
            let report = resumed.finish().await?;
            InjectorEffect::WorkerServiceRestarted { before, after }.ensure_happened()?;
            assert_resumed(run, COUNT, &delivery(&report.outputs[&id])?)
                .with_context(|| format!("{}: worker-service restart", fixture.sdk))?;
        }
        Ok(())
    }

    #[test]
    #[timeout("8 minutes")]
    async fn invocation_session_output_survives_modelled_browser_reload(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        let relayed = Arc::new(RelayedWorkerService::start(deps.worker_service()).await?);
        let view = deps.with_worker_service(relayed.clone());
        for &fixture in FIXTURES {
            let row = row(&view, fixture).await?;
            let agent = fresh_agent();
            let run = next_run();
            let (session, gate, id) = start_gated(&view, &row, &agent, run).await?;
            let epoch_before = session.checkpoint().acceptance.as_ref().unwrap().epoch;
            // The reload: the client loses its connection, then attaches with its saved session.
            let resets = relayed.drop_connections().await;
            let saved = session.disconnect();
            let (resumed, _) =
                InvocationSession::reattach(&view, saved, ResumeOperation::Takeover, PHASE).await?;
            let epoch_after = resumed.checkpoint().acceptance.as_ref().unwrap().epoch;
            row.user.complete_promise(&gate, Vec::new()).await?;
            let report = resumed.finish().await?;
            InjectorEffect::ClientDropped { resets }.ensure_happened()?;
            InjectorEffect::BrowserReloaded {
                resets,
                epoch_before,
                epoch_after,
            }
            .ensure_happened()?;
            assert_resumed(run, COUNT, &delivery(&report.outputs[&id])?)
                .with_context(|| format!("{}: modelled browser reload", fixture.sdk))?;
        }
        Ok(())
    }

    #[test]
    #[timeout("8 minutes")]
    async fn invocation_session_output_survives_deploy_mid_stream(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        for &fixture in FIXTURES {
            let row = row(deps, fixture).await?;
            let agent = fresh_agent();
            let agent_id =
                AgentId::from_agent_id(row.component.id, &agent).map_err(anyhow::Error::msg)?;
            let run = next_run();
            let (mut session, gate, id) = start_gated(deps, &row, &agent, run).await?;
            let before = row
                .user
                .get_worker_metadata(&agent_id)
                .await?
                .component_revision;
            // The new revision runs the same code; only its environment differs.
            let updated = row
                .user
                .update_component_with_env(
                    &row.component.id,
                    FIXTURE_AGENT_TYPE,
                    fixture.wasm,
                    &[("FAULT_MATRIX_DEPLOY".to_string(), run.to_string())],
                )
                .await?;
            row.user
                .auto_update_worker(&agent_id, updated.revision, false)
                .await?;
            row.user.complete_promise(&gate, Vec::new()).await?;
            // The update is requested while the session is open and the producer is gated.
            ensure!(
                session.checkpoint().completion.is_none(),
                "the invocation completed before the deploy"
            );
            session.receive().await?;
            let report = session.finish().await?;
            let after = tokio::time::timeout(PHASE, async {
                loop {
                    let revision = row
                        .user
                        .get_worker_metadata(&agent_id)
                        .await?
                        .component_revision;
                    if revision == updated.revision {
                        return Ok::<_, anyhow::Error>(revision);
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await??;
            InjectorEffect::Deployed {
                before: before.get(),
                after: after.get(),
            }
            .ensure_happened()?;
            assert_resumed(run, COUNT, &delivery(&report.outputs[&id])?)
                .with_context(|| format!("{}: deploy mid-stream", fixture.sdk))?;
        }
        Ok(())
    }

    // ---- PR subset: streaming RPC ----------------------------------------------------------

    fn executor_index(
        deps: &EnvBasedTestDependencies,
        table: &RoutingTable,
        agent: &AgentId,
    ) -> anyhow::Result<usize> {
        let pod = table.lookup(agent).context("the agent has no executor")?;
        deps.worker_executor_cluster()
            .to_vec()
            .iter()
            .position(|executor| executor.grpc_port() == pod.port)
            .context("the agent's executor is not in the cluster")
    }

    #[test]
    #[timeout("8 minutes")]
    async fn streaming_rpc_output_survives_callee_executor_kill(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        for &fixture in FIXTURES {
            let row = row(deps, fixture).await?;
            let table = deps.shard_manager().get_routing_table().await?;
            let (caller, target, callee_index, caller_index) = (0..10_000)
                .find_map(|index| {
                    let name = format!("rpc-{}-{index}", uuid::Uuid::new_v4());
                    let caller = agent(&format!("{name}-caller"));
                    let target = format!("{name}-target");
                    let caller_id = AgentId::from_agent_id(row.component.id, &caller).ok()?;
                    let target_id =
                        AgentId::from_agent_id(row.component.id, &agent(&target)).ok()?;
                    let caller_index = executor_index(deps, &table, &caller_id).ok()?;
                    let callee_index = executor_index(deps, &table, &target_id).ok()?;
                    (caller_index != callee_index).then_some((
                        caller,
                        target,
                        callee_index,
                        caller_index,
                    ))
                })
                .context("no caller and callee on different executors")?;
            let run = next_run();
            let gate = gate(&row, &agent(&target)).await?;
            let call = {
                let user = row.user.clone();
                let component = row.component.clone();
                let caller = caller.clone();
                let input = data_value!(target.clone(), run, COUNT, PAUSE_AFTER, gate.clone());
                tokio::spawn(async move {
                    user.invoke_and_await_agent(&component, &caller, "read_remote", input)
                        .await
                })
            };
            // The callee's producer cannot finish before the gate opens, so the stream is open
            // when its executor dies. The pause lets the caller read the first records.
            tokio::time::sleep(Duration::from_secs(3)).await;
            let pids = executor_pids(deps);
            let cluster = deps.worker_executor_cluster();
            cluster.stop(callee_index).await;
            cluster.start(callee_index).await;
            wait_for_routing(deps).await?;
            let after = executor_pids(deps);
            InjectorEffect::ExecutorRestarted {
                before: vec![pids[callee_index]],
                after: vec![after[callee_index]],
            }
            .ensure_happened()?;
            ensure!(
                pids[caller_index] == after[caller_index],
                "the caller's executor restarted too"
            );
            row.user.complete_promise(&gate, Vec::new()).await?;
            let read = records(
                call.await??
                    .into_return_value()
                    .context("read_remote returned no value")?,
            )?;
            assert_resumed(run, COUNT, &Delivery::new(read, Terminal::End))
                .with_context(|| format!("{}: callee executor kill", fixture.sdk))?;
        }
        Ok(())
    }

    // ---- PR subset: Durable Streams output read --------------------------------------------

    struct DurableRead {
        path: String,
        offset: String,
        records: Vec<u32>,
        closed: bool,
    }

    async fn open_durable_stream(
        context: &HttpTestContext,
        run: u32,
        delay_ms: u64,
    ) -> anyhow::Result<DurableRead> {
        let session = uuid::Uuid::new_v4();
        let path = format!(
            "/fault-matrix/{session}/numbered/{run}/{COUNT}/invocations/{session}/streams/$result?delay_ms={delay_ms}"
        );
        let created = context
            .client
            .put(context.base_url.join(&path)?)
            .send()
            .await?;
        ensure!(
            created.status() == StatusCode::CREATED,
            "session create returned {}",
            created.status()
        );
        Ok(DurableRead {
            path,
            offset: "-1".to_string(),
            records: Vec::new(),
            closed: false,
        })
    }

    impl DurableRead {
        /// One read from the current offset. `long_poll` waits for data at the tail.
        async fn read(&mut self, context: &HttpTestContext, long_poll: bool) -> anyhow::Result<()> {
            let live = if long_poll { "&live=long-poll" } else { "" };
            let response = context
                .client
                .get(
                    context
                        .base_url
                        .join(&format!("{}&offset={}{live}", self.path, self.offset))?,
                )
                .send()
                .await?;
            let status = response.status();
            ensure!(
                status == StatusCode::OK || status == StatusCode::NO_CONTENT,
                "read returned {status}"
            );
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            };
            let next = header("stream-next-offset").context("read omitted its next offset")?;
            let closed = header("stream-closed").as_deref() == Some("true");
            if status == StatusCode::OK {
                let items: Vec<u32> = response.json().await?;
                self.records.extend(items);
            }
            self.offset = next;
            self.closed = closed;
            Ok(())
        }

        async fn read_until(
            &mut self,
            context: &HttpTestContext,
            done: impl Fn(&Self) -> bool,
        ) -> anyhow::Result<()> {
            tokio::time::timeout(PHASE, async {
                while !done(self) {
                    self.read(context, false).await?;
                    if !done(self) {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
                Ok::<_, anyhow::Error>(())
            })
            .await?
        }

        fn delivery(&self) -> Delivery {
            Delivery::new(
                self.records.clone(),
                if self.closed {
                    Terminal::End
                } else {
                    Terminal::Open
                },
            )
        }
    }

    async fn durable_context(
        deps: &EnvBasedTestDependencies,
        fixture: Fixture,
    ) -> anyhow::Result<HttpTestContext> {
        make_test_context(
            deps,
            vec![(
                AgentTypeName(FIXTURE_AGENT_TYPE.to_string()),
                HttpApiDeploymentAgentOptions::default(),
            )],
            fixture.wasm,
            fixture.component,
        )
        .await
    }

    fn durable_fixtures() -> impl Iterator<Item = Fixture> {
        FIXTURES
            .iter()
            .copied()
            .filter(|fixture| fixture.durable_streams)
    }

    #[test]
    #[timeout("8 minutes")]
    async fn durable_streams_read_survives_executor_kill(matrix: &Matrix) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        for fixture in durable_fixtures() {
            let context = durable_context(deps, fixture).await?;
            let run = next_run();
            let mut read = open_durable_stream(&context, run, 300).await?;
            read.read_until(&context, |read| read.records.len() >= PAUSE_AFTER as usize)
                .await?;
            ensure!(!read.closed, "the stream closed before the fault");
            let before = kill_executors(deps).await?;
            let effect = restart_executors(deps, before).await?;
            read.read_until(&context, |read| read.closed).await?;
            effect.ensure_happened()?;
            assert_resumed(run, COUNT, &read.delivery())
                .with_context(|| format!("{}: Durable Streams executor kill", fixture.sdk))?;
        }
        Ok(())
    }

    #[test]
    #[timeout("8 minutes")]
    async fn durable_streams_read_survives_worker_service_restart(
        matrix: &Matrix,
    ) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        let service = Arc::new(RestartableWorkerService::start(deps).await?);
        let view = deps.with_worker_service(service.clone());
        for fixture in durable_fixtures() {
            let context = durable_context(&view, fixture).await?;
            let run = next_run();
            let mut read = open_durable_stream(&context, run, 300).await?;
            read.read_until(&context, |read| read.records.len() >= PAUSE_AFTER as usize)
                .await?;
            ensure!(!read.closed, "the stream closed before the fault");
            let before = service.pid().context("the service is not running")?;
            service.restart().await;
            let after = service.pid().context("the service did not restart")?;
            // The client resumes from its offset on the restarted gateway.
            read.read_until(&context, |read| read.closed).await?;
            InjectorEffect::WorkerServiceRestarted { before, after }.ensure_happened()?;
            assert_resumed(run, COUNT, &read.delivery()).with_context(|| {
                format!("{}: Durable Streams worker-service restart", fixture.sdk)
            })?;
        }
        Ok(())
    }

    #[test]
    #[timeout("8 minutes")]
    async fn durable_streams_long_poll_survives_client_drop(matrix: &Matrix) -> anyhow::Result<()> {
        let deps = &matrix.deps;
        let relayed = Arc::new(RelayedWorkerService::start(deps.worker_service()).await?);
        let view = deps.with_worker_service(relayed.clone());
        for fixture in durable_fixtures() {
            let context = durable_context(&view, fixture).await?;
            let run = next_run();
            let mut read = open_durable_stream(&context, run, 1_000).await?;
            read.read_until(&context, |read| read.records.len() >= PAUSE_AFTER as usize)
                .await?;
            ensure!(!read.closed, "the stream closed before the fault");
            // A long-poll is open at the tail when the relay drops every connection.
            let url = context.base_url.join(&format!(
                "{}&offset={}&live=long-poll",
                read.path, read.offset
            ))?;
            let pending = tokio::spawn(context.client.get(url).send());
            tokio::time::sleep(Duration::from_millis(200)).await;
            let resets = relayed.drop_connections().await;
            let dropped = pending.await?;
            InjectorEffect::ClientDropped { resets }.ensure_happened()?;
            if let Ok(response) = dropped {
                // The long-poll answered before the drop reached it; its data is not consumed,
                // so the reconnect below reads it again from the same offset.
                drop(response);
            }
            // Reconnect with the saved offset and long-poll to the end.
            tokio::time::timeout(PHASE, async {
                while !read.closed {
                    read.read(&context, true).await?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await??;
            assert_resumed(run, COUNT, &read.delivery()).with_context(|| {
                format!("{}: Durable Streams long-poll client drop", fixture.sdk)
            })?;
        }
        Ok(())
    }
}
