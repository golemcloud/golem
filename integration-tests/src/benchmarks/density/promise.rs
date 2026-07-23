// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

//! Promise-completion density benchmark (golemcloud/golem#3525).

use super::prep::PrepManifest;
use super::{PromiseRuntime, PromiseTopology, PromiseWaiterPresence};
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

pub const PROMISE_PAYLOAD_BOOLEAN: usize = 1;
pub const PROMISE_PAYLOAD_ONE_INTEGER: usize = std::mem::size_of::<u64>();
pub const PROMISE_PAYLOAD_TWO_INTEGERS: usize = 2 * std::mem::size_of::<u64>();
pub const PROMISE_PAYLOAD_TINY: usize = 256;
pub const PROMISE_PAYLOAD_SMALL: usize = 65_536;
pub const PROMISE_PAYLOAD_MEDIUM: usize = 16_777_216;
pub const PROMISE_PAYLOAD_HUGE: usize = 32_505_856;

const PROMISE_AGENT_TYPE: &str = "PromiseAgent";
const DEFAULT_RATE_RAMP: &[u32] = &[1, 2, 4, 8, 16, 32, 64, 128, 256];
const DEFAULT_RATE_PERIOD: Duration = Duration::from_secs(60);
const WAITER_READY_TIMEOUT: Duration = Duration::from_secs(60);
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
    pub runtime: PromiseRuntime,
}

impl CellConfig {
    pub fn cell_name(&self) -> String {
        format!(
            "promise-{}-{}-{}-{}",
            payload_label(self.payload_size),
            self.waiter_presence,
            self.runtime,
            self.topology
        )
    }
}

struct PromiseWork {
    agent: AgentId,
    parsed_agent: ParsedAgentId,
    promise: PromiseId,
    wait: bool,
    runtime: PromiseRuntime,
}

struct StageTimings {
    get_promise: Duration,
    await_promise: Option<Duration>,
    suspended_wait: Option<Duration>,
}

struct RestageTimings {
    idle_wait: Duration,
    stage: StageTimings,
}

pub async fn run_cell(
    config: &CellConfig,
    rate_ramp: Option<&[u32]>,
    manifest: &PrepManifest,
    deps: &BenchmarkTestDependencies,
) -> anyhow::Result<BenchmarkResult> {
    validate_payload_size(config.payload_size)?;
    let rates = validated_rates(rate_ramp.unwrap_or(DEFAULT_RATE_RAMP))?;
    let component = resolve_component(manifest, deps, config.runtime).await?;
    let user = manifest.user_context(deps);
    let payload = vec![0; config.payload_size];
    let mut outcome = Outcome::default();
    println!(
        "Promise-density [{}]: preparing {PROMISE_POOL_SIZE} workers",
        config.cell_name()
    );
    let (ready_sender, mut ready_work) = create_work_pool(&user, &component, config).await?;

    for &rate in rates {
        println!(
            "Promise-density [{}]: completing at {rate}/s",
            config.cell_name()
        );
        let period = complete_at_rate(
            &user,
            &component,
            &ready_sender,
            &mut ready_work,
            payload.clone(),
            rate,
        )
        .await?;
        outcome.record(rate, period);
    }

    drop(ready_sender);
    drop(ready_work);
    cleanup_pool(&user, &component, config).await?;
    Ok(outcome.into_benchmark_result(config))
}

async fn resolve_component(
    manifest: &PrepManifest,
    deps: &BenchmarkTestDependencies,
    runtime: PromiseRuntime,
) -> anyhow::Result<ComponentDto> {
    let component_id = match runtime {
        PromiseRuntime::Rust => manifest.uniform_component_id.as_ref(),
        PromiseRuntime::Ts => manifest.promise_ts_component_id.as_ref(),
    }
    .ok_or_else(|| anyhow::anyhow!("manifest has no {runtime} promise component id"))?;
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
                let (work, _) = stage_work(
                    &user,
                    &component,
                    agent,
                    parsed_agent,
                    should_wait(config.waiter_presence, index),
                    config.runtime,
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
    stream::iter(workers)
        .map(|worker| async move {
            let deleted_started = Instant::now();
            loop {
                if user.get_worker_metadata_opt(&worker).await?.is_none() {
                    return Ok(());
                }
                if deleted_started.elapsed() >= WAITER_READY_TIMEOUT {
                    anyhow::bail!("timed out deleting worker {worker}");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .buffer_unordered(SETUP_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
    Ok(())
}

async fn stage_work(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    agent: AgentId,
    parsed_agent: ParsedAgentId,
    wait: bool,
    runtime: PromiseRuntime,
) -> anyhow::Result<(PromiseWork, StageTimings)> {
    let get_promise_started = Instant::now();
    let result = user
        .invoke_and_await_agent(
            component,
            &parsed_agent,
            get_promise_method(runtime),
            data_value!(),
        )
        .await?;
    let get_promise = get_promise_started.elapsed();
    let promise_value = result
        .into_return_value_and_type()
        .ok_or_else(|| anyhow::anyhow!("{} returned no promise id", get_promise_method(runtime)))?;
    let promise = PromiseId::from_value(promise_value.value.clone())
        .map_err(|error| anyhow::anyhow!("invalid promise id: {error}"))?;
    let (await_promise, suspended_wait) = if wait {
        let await_promise_started = Instant::now();
        user.invoke_agent(
            component,
            &parsed_agent,
            await_promise_method(runtime),
            data_value!(promise_value.clone()),
        )
        .await?;
        let await_promise = await_promise_started.elapsed();
        let suspended_wait_started = Instant::now();
        user.wait_for_status(&agent, AgentStatus::Suspended, WAITER_READY_TIMEOUT)
            .await?;
        (Some(await_promise), Some(suspended_wait_started.elapsed()))
    } else {
        (None, None)
    };
    Ok((
        PromiseWork {
            agent,
            parsed_agent,
            promise,
            wait,
            runtime,
        },
        StageTimings {
            get_promise,
            await_promise,
            suspended_wait,
        },
    ))
}

async fn complete_at_rate(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    ready_sender: &mpsc::Sender<anyhow::Result<PromiseWork>>,
    ready_work: &mut mpsc::Receiver<anyhow::Result<PromiseWork>>,
    payload: Vec<u8>,
    rate: u32,
) -> anyhow::Result<Period> {
    let started = Instant::now();
    let deadline = started + DEFAULT_RATE_PERIOD;
    let count = rate as usize * DEFAULT_RATE_PERIOD.as_secs() as usize;
    let completion_limit = Arc::new(Semaphore::new(COMPLETION_CONCURRENCY));
    let staging_limit = Arc::new(Semaphore::new(SETUP_CONCURRENCY));
    let (completion_sender, mut completion_receiver) =
        mpsc::channel::<anyhow::Result<Duration>>(count);
    let (restage_sender, mut restage_receiver) = mpsc::channel(count);
    let mut restages = JoinSet::new();
    let mut minimum_ready_depth = PROMISE_POOL_SIZE;
    let mut starvation_count = 0;
    let mut scheduled_completions = 0;
    for index in 0..count {
        tokio::time::sleep_until(
            (started + Duration::from_secs_f64(index as f64 / rate as f64)).into(),
        )
        .await;
        minimum_ready_depth = minimum_ready_depth.min(ready_work.len());
        let work = match ready_work.try_recv() {
            Ok(work) => work?,
            Err(mpsc::error::TryRecvError::Empty) => {
                starvation_count += 1;
                continue;
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                anyhow::bail!("promise work producer stopped")
            }
        };
        let user = user.clone();
        let component = component.clone();
        let payload = payload.clone();
        let ready_sender = (*ready_sender).clone();
        let completion_sender = completion_sender.clone();
        let restage_sender = restage_sender.clone();
        let completion_limit = completion_limit.clone();
        let staging_limit = staging_limit.clone();
        scheduled_completions += 1;
        restages.spawn(async move {
            let latency = {
                let _permit = completion_limit.acquire_owned().await?;
                let completion_started = Instant::now();
                user.complete_promise(&work.promise, payload).await?;
                completion_started.elapsed()
            };
            completion_sender
                .send(Ok(latency))
                .await
                .map_err(|_| anyhow::anyhow!("completion receiver closed"))?;
            let idle_wait_started = Instant::now();
            user.wait_for_status(&work.agent, AgentStatus::Idle, WAITER_READY_TIMEOUT)
                .await?;
            let idle_wait = idle_wait_started.elapsed();
            let _permit = staging_limit.acquire_owned().await?;
            let (restaged, stage) = stage_work(
                &user,
                &component,
                work.agent,
                work.parsed_agent,
                work.wait,
                work.runtime,
            )
            .await?;
            restage_sender
                .send(RestageTimings { idle_wait, stage })
                .await
                .map_err(|_| anyhow::anyhow!("restage receiver closed"))?;
            ready_sender
                .send(Ok(restaged))
                .await
                .map_err(|_| anyhow::anyhow!("promise work receiver closed"))?;
            Ok::<_, anyhow::Error>(())
        });
    }
    drop(completion_sender);
    drop(restage_sender);

    let mut completion_latencies = Vec::with_capacity(scheduled_completions);
    while completion_latencies.len() < scheduled_completions {
        let completion = completion_receiver
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("completion producer stopped"))?;
        completion_latencies.push(completion?);
    }
    while let Some(restage) = restages.join_next().await {
        match restage {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut restage_timings = Vec::new();
    while let Ok(restage) = restage_receiver.try_recv() {
        restage_timings.push(restage);
    }
    println!(
        "Promise-density: offered={rate}/s completed={} pool-min-ready={minimum_ready_depth} \
         pool-starvations={starvation_count} restaged={}",
        completion_latencies.len(),
        restage_timings.len(),
    );

    Ok(Period {
        completed: completion_latencies.len() as u64,
        elapsed: deadline.duration_since(started),
        completion_latencies,
        restage_timings,
    })
}

fn should_wait(presence: PromiseWaiterPresence, index: usize) -> bool {
    match presence {
        PromiseWaiterPresence::Cold => false,
        PromiseWaiterPresence::Warm => true,
        PromiseWaiterPresence::Mixed => index.is_multiple_of(2),
    }
}

fn get_promise_method(runtime: PromiseRuntime) -> &'static str {
    match runtime {
        PromiseRuntime::Rust => "get_promise",
        PromiseRuntime::Ts => "getPromise",
    }
}

fn await_promise_method(runtime: PromiseRuntime) -> &'static str {
    match runtime {
        PromiseRuntime::Rust => "await_promise",
        PromiseRuntime::Ts => "awaitPromiseVoid",
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
        PROMISE_PAYLOAD_BOOLEAN => "boolean",
        PROMISE_PAYLOAD_ONE_INTEGER => "one-integer",
        PROMISE_PAYLOAD_TWO_INTEGERS => "two-integers",
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
    restage_timings: Vec<RestageTimings>,
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
            for restage in period.restage_timings {
                recorder.duration(
                    &ResultKey::primary(format!("promise-idle-wait-at-offered-{rate}-per-sec")),
                    restage.idle_wait,
                );
                recorder.duration(
                    &ResultKey::primary(format!("promise-get-promise-at-offered-{rate}-per-sec")),
                    restage.stage.get_promise,
                );
                if let Some(await_promise) = restage.stage.await_promise {
                    recorder.duration(
                        &ResultKey::primary(format!(
                            "promise-await-promise-at-offered-{rate}-per-sec"
                        )),
                        await_promise,
                    );
                }
                if let Some(suspended_wait) = restage.stage.suspended_wait {
                    recorder.duration(
                        &ResultKey::primary(format!(
                            "promise-suspended-wait-at-offered-{rate}-per-sec"
                        )),
                        suspended_wait,
                    );
                }
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
            PROMISE_PAYLOAD_BOOLEAN,
            PROMISE_PAYLOAD_ONE_INTEGER,
            PROMISE_PAYLOAD_TWO_INTEGERS,
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
