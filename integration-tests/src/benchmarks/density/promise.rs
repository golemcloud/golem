// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

//! Promise-completion density benchmark (golemcloud/golem#3525).

use super::prep::PrepManifest;
use super::{PromiseTopology, PromiseWaiterPresence};
use futures::stream::{self, StreamExt, TryStreamExt};
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::base_model::{AgentId, PromiseId};
use golem_common::model::AgentStatus;
use golem_common::model::component::ComponentDto;
use golem_common::{agent_id, data_value};
use golem_test_framework::benchmark::{
    BenchmarkRecorder, BenchmarkResult, BenchmarkRunResult, ResultKey, RunConfig,
};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::FromValue;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

pub const PROMISE_PAYLOAD_TINY: usize = 256;
pub const PROMISE_PAYLOAD_SMALL: usize = 65_536;
pub const PROMISE_PAYLOAD_MEDIUM: usize = 16_777_216;
pub const PROMISE_PAYLOAD_HUGE: usize = 32_505_856;

const PROMISE_AGENT_TYPE: &str = "PromiseAgent";
const DEFAULT_RATE_RAMP: &[u32] = &[1, 2, 4, 8, 16, 32, 64, 128, 256];
const DEFAULT_RATE_PERIOD: Duration = Duration::from_secs(60);
const WAITER_READY_TIMEOUT: Duration = Duration::from_secs(60);
const POOL_STARVATION_THRESHOLD: Duration = Duration::from_millis(10);
// Keep the pool small enough that benchmark scaffolding does not become an
// agent-density test; pool starvation reports lifecycle capacity separately.
const PROMISE_POOL_SIZE: usize = 256;
const READY_WORK_CAPACITY: usize = PROMISE_POOL_SIZE;
// Creating agents/promises traverses cold worker activation and must not flood
// the gateway.
const SETUP_CONCURRENCY: usize = 16;
const COMPLETION_CONCURRENCY: usize = 100;

#[derive(Debug, Clone, Copy)]
pub struct CellConfig {
    pub payload_size: usize,
    pub waiter_presence: PromiseWaiterPresence,
    pub topology: PromiseTopology,
}

impl CellConfig {
    pub fn cell_name(&self) -> String {
        format!(
            "promise-{}-{}-{}",
            payload_label(self.payload_size),
            self.waiter_presence,
            self.topology
        )
    }
}

struct PromiseWork {
    agent: AgentId,
    parsed_agent: ParsedAgentId,
    promise: PromiseId,
    wait: bool,
}

pub async fn run_cell(
    config: &CellConfig,
    rate_ramp: Option<&[u32]>,
    manifest: &PrepManifest,
    deps: &BenchmarkTestDependencies,
) -> anyhow::Result<BenchmarkResult> {
    validate_payload_size(config.payload_size)?;
    let rates = validated_rates(rate_ramp.unwrap_or(DEFAULT_RATE_RAMP))?;
    let component = resolve_component(manifest, deps).await?;
    let user = manifest.user_context(deps);
    let payload = vec![0; config.payload_size];
    let mut outcome = Outcome::default();

    for &rate in rates {
        println!(
            "Promise-density [{}]: preparing {PROMISE_POOL_SIZE} workers for {rate}/s",
            config.cell_name()
        );
        let (ready_sender, ready_work) = create_work_pool(&user, &component, config).await?;
        println!(
            "Promise-density [{}]: completing at {rate}/s",
            config.cell_name()
        );
        let period = complete_at_rate(
            &user,
            &component,
            ready_sender,
            ready_work,
            payload.clone(),
            rate,
        )
        .await?;
        outcome.record(rate, period);
    }

    cleanup_pool(&user, &component, config).await?;
    Ok(outcome.into_benchmark_result(config))
}

async fn resolve_component(
    manifest: &PrepManifest,
    deps: &BenchmarkTestDependencies,
) -> anyhow::Result<ComponentDto> {
    let component_id = manifest
        .uniform_component_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("manifest has no promise component id"))?;
    manifest
        .user_context(deps)
        .get_latest_component_revision(component_id)
        .await
}

async fn create_work_pool(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    config: &CellConfig,
) -> anyhow::Result<(
    mpsc::Sender<anyhow::Result<PromiseWork>>,
    mpsc::Receiver<anyhow::Result<PromiseWork>>,
)> {
    let (ready_sender, ready_receiver) = mpsc::channel(READY_WORK_CAPACITY);
    stream::iter(0..PROMISE_POOL_SIZE)
        .map(|index| {
            let user = user.clone();
            let component = component.clone();
            let ready_sender = ready_sender.clone();
            async move {
                let name = format!("{}-{index}", config.cell_name());
                let parsed_agent = agent_id!(PROMISE_AGENT_TYPE, name);
                let agent = user
                    .start_agent(&component.id, parsed_agent.clone())
                    .await?;
                let work = stage_work(
                    &user,
                    &component,
                    agent,
                    parsed_agent,
                    should_wait(config.waiter_presence, index),
                )
                .await?;
                ready_sender
                    .send(Ok(work))
                    .await
                    .map_err(|_| anyhow::anyhow!("promise work receiver closed"))
            }
        })
        .buffer_unordered(SETUP_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
    Ok((ready_sender, ready_receiver))
}

async fn cleanup_pool(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    config: &CellConfig,
) -> anyhow::Result<()> {
    let workers = (0..PROMISE_POOL_SIZE)
        .filter_map(|index| {
            let agent = agent_id!(
                PROMISE_AGENT_TYPE,
                format!("{}-{index}", config.cell_name())
            );
            AgentId::from_agent_id(component.id, &agent).ok()
        })
        .collect::<Vec<_>>();
    crate::benchmarks::delete_workers(user, &workers).await;
    Ok(())
}

async fn stage_work(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    agent: AgentId,
    parsed_agent: ParsedAgentId,
    wait: bool,
) -> anyhow::Result<PromiseWork> {
    let result = user
        .invoke_and_await_agent(component, &parsed_agent, "getPromise", data_value!())
        .await?;
    let promise_value = result
        .into_return_value_and_type()
        .ok_or_else(|| anyhow::anyhow!("getPromise returned no promise id"))?;
    let promise = PromiseId::from_value(promise_value.value.clone())
        .map_err(|error| anyhow::anyhow!("invalid promise id: {error}"))?;
    if wait {
        user.invoke_agent(
            component,
            &parsed_agent,
            "awaitPromise",
            data_value!(promise_value.clone()),
        )
        .await?;
        user.wait_for_status(&agent, AgentStatus::Suspended, WAITER_READY_TIMEOUT)
            .await?;
    }
    Ok(PromiseWork {
        agent,
        parsed_agent,
        promise,
        wait,
    })
}

async fn complete_at_rate(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    ready_sender: mpsc::Sender<anyhow::Result<PromiseWork>>,
    mut ready_work: mpsc::Receiver<anyhow::Result<PromiseWork>>,
    payload: Vec<u8>,
    rate: u32,
) -> anyhow::Result<Period> {
    let started = Instant::now();
    let count = rate as usize * DEFAULT_RATE_PERIOD.as_secs() as usize;
    let completion_limit = Arc::new(Semaphore::new(COMPLETION_CONCURRENCY));
    let staging_limit = Arc::new(Semaphore::new(SETUP_CONCURRENCY));
    let mut completions = JoinSet::new();
    let mut minimum_ready_depth = PROMISE_POOL_SIZE;
    let mut starvation_count = 0;
    let mut maximum_ready_wait = Duration::ZERO;
    for index in 0..count {
        tokio::time::sleep_until(
            (started + Duration::from_secs_f64(index as f64 / rate as f64)).into(),
        )
        .await;
        minimum_ready_depth = minimum_ready_depth.min(ready_work.len());
        let ready_wait_started = Instant::now();
        let work = ready_work
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("promise work producer stopped"))??;
        let ready_wait = ready_wait_started.elapsed();
        maximum_ready_wait = maximum_ready_wait.max(ready_wait);
        if ready_wait > POOL_STARVATION_THRESHOLD {
            starvation_count += 1;
        }
        let user = user.clone();
        let component = component.clone();
        let payload = payload.clone();
        let ready_sender = ready_sender.clone();
        let completion_limit = completion_limit.clone();
        let staging_limit = staging_limit.clone();
        completions.spawn(async move {
            let latency = {
                let _permit = completion_limit.acquire_owned().await?;
                let completion_started = Instant::now();
                user.complete_promise(&work.promise, payload).await?;
                completion_started.elapsed()
            };
            user.wait_for_status(&work.agent, AgentStatus::Idle, WAITER_READY_TIMEOUT)
                .await?;
            let _permit = staging_limit.acquire_owned().await?;
            let restaged =
                stage_work(&user, &component, work.agent, work.parsed_agent, work.wait).await?;
            ready_sender
                .send(Ok(restaged))
                .await
                .map_err(|_| anyhow::anyhow!("promise work receiver closed"))?;
            Ok::<_, anyhow::Error>(latency)
        });
    }
    let mut completion_latencies = Vec::with_capacity(count);
    while let Some(completion) = completions.join_next().await {
        completion_latencies.push(completion??);
    }
    println!(
        "Promise-density: offered={rate}/s pool-min-ready={minimum_ready_depth} \
         pool-starvations={starvation_count} pool-max-wait-ms={}",
        maximum_ready_wait.as_millis()
    );

    // Every restaged worker has one ready promise when the measured window ends.
    // Complete these unmeasured promises before advancing so the next rate starts
    // from an empty promise registry rather than accumulating one pool per step.
    drop(ready_sender);
    let mut cleanup_work = Vec::new();
    while let Ok(work) = ready_work.try_recv() {
        cleanup_work.push(work?);
    }
    stream::iter(cleanup_work)
        .map(|work| {
            let user = user.clone();
            async move {
                user.complete_promise(&work.promise, Vec::new()).await?;
                user.wait_for_status(&work.agent, AgentStatus::Idle, WAITER_READY_TIMEOUT)
                    .await
            }
        })
        .buffer_unordered(SETUP_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
    Ok(Period {
        completed: completion_latencies.len() as u64,
        elapsed: started.elapsed(),
        completion_latencies,
    })
}

fn should_wait(presence: PromiseWaiterPresence, index: usize) -> bool {
    match presence {
        PromiseWaiterPresence::Cold => false,
        PromiseWaiterPresence::Warm => true,
        PromiseWaiterPresence::Mixed => index.is_multiple_of(2),
    }
}

fn validate_payload_size(payload_size: usize) -> anyhow::Result<()> {
    if payload_size == 0 || payload_size > PROMISE_PAYLOAD_HUGE {
        anyhow::bail!("promise payload size must be between 1 and {PROMISE_PAYLOAD_HUGE} bytes");
    }
    Ok(())
}

fn validated_rates(rates: &[u32]) -> anyhow::Result<&[u32]> {
    if rates.is_empty() || rates.contains(&0) || rates.windows(2).any(|rates| rates[0] >= rates[1])
    {
        anyhow::bail!("promise rate ramp must contain strictly increasing positive rates");
    }
    Ok(rates)
}

fn payload_label(size: usize) -> &'static str {
    match size {
        PROMISE_PAYLOAD_TINY => "tiny",
        PROMISE_PAYLOAD_SMALL => "small",
        PROMISE_PAYLOAD_MEDIUM => "medium",
        PROMISE_PAYLOAD_HUGE => "huge",
        _ => "custom",
    }
}

#[derive(Default)]
struct Outcome {
    completed: u64,
    max_rate: u32,
    periods: Vec<(u32, Period)>,
}

struct Period {
    completed: u64,
    elapsed: Duration,
    completion_latencies: Vec<Duration>,
}

impl Outcome {
    fn record(&mut self, rate: u32, period: Period) {
        self.completed += period.completed;
        self.max_rate = self.max_rate.max(rate);
        self.periods.push((rate, period));
    }

    fn into_benchmark_result(self, config: &CellConfig) -> BenchmarkResult {
        let recorder = BenchmarkRecorder::new();
        recorder.count(&ResultKey::primary("promise-completions"), self.completed);
        recorder.count(
            &ResultKey::primary("promise-payload-size-bytes"),
            config.payload_size as u64,
        );
        recorder.count(
            &ResultKey::primary("max-offered-promise-completion-rate-per-sec"),
            self.max_rate as u64,
        );
        for (rate, period) in self.periods {
            recorder.count(
                &ResultKey::primary(format!("promise-completions-at-offered-{rate}-per-sec")),
                period.completed,
            );
            recorder.count(
                &ResultKey::primary(format!(
                    "promise-completion-period-duration-ms-at-offered-{rate}-per-sec"
                )),
                period.elapsed.as_millis() as u64,
            );
            recorder.count(
                &ResultKey::primary(format!(
                    "achieved-promise-completion-rate-at-offered-{rate}-per-sec"
                )),
                (period.completed as f64 / period.elapsed.as_secs_f64()).round() as u64,
            );
            for latency in period.completion_latencies {
                recorder.duration(&ResultKey::primary("promise-completion-latency"), latency);
                recorder.duration(
                    &ResultKey::primary(format!(
                        "promise-completion-latency-at-offered-{rate}-per-sec"
                    )),
                    latency,
                );
            }
        }
        let run_config = RunConfig {
            cluster_size: 0,
            size: self.max_rate as usize,
            length: config.payload_size,
            disable_compilation_cache: false,
        };
        let mut run_result = BenchmarkRunResult::new(run_config.clone());
        run_result.add(recorder);
        BenchmarkResult {
            name: format!("density-{}", config.cell_name()),
            description: format!(
                "Promise-density cell: payload={} bytes, waiter-presence={}, topology={}",
                config.payload_size, config.waiter_presence, config.topology
            ),
            runs: vec![run_config],
            results: vec![run_result],
            run_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn accepts_documented_payload_sizes() {
        for size in [
            PROMISE_PAYLOAD_TINY,
            PROMISE_PAYLOAD_SMALL,
            PROMISE_PAYLOAD_MEDIUM,
            PROMISE_PAYLOAD_HUGE,
        ] {
            assert!(validate_payload_size(size).is_ok());
        }
    }

    #[test]
    fn mixed_waiters_are_evenly_split() {
        assert!(should_wait(PromiseWaiterPresence::Mixed, 0));
        assert!(!should_wait(PromiseWaiterPresence::Mixed, 1));
    }
}
