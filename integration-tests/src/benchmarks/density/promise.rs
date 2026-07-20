// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

//! Promise-completion density benchmark (golemcloud/golem#3525).

use super::prep::PrepManifest;
use super::{PromiseFanIn, PromiseTopology, PromiseWaiterPresence};
use golem_common::base_model::{AgentId, PromiseId};
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::{agent_id, data_value};
use golem_common::model::AgentStatus;
use golem_common::model::component::ComponentDto;
use golem_test_framework::benchmark::{
    BenchmarkRecorder, BenchmarkResult, BenchmarkRunResult, ResultKey, RunConfig,
};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use golem_wasm::FromValue;
use futures::stream::{self, StreamExt, TryStreamExt};
use std::time::{Duration, Instant};

pub const PROMISE_PAYLOAD_TINY: usize = 256;
pub const PROMISE_PAYLOAD_SMALL: usize = 65_536;
pub const PROMISE_PAYLOAD_MEDIUM: usize = 16_777_216;
pub const PROMISE_PAYLOAD_HUGE: usize = 32_505_856;

const PROMISE_AGENT_TYPE: &str = "PromiseAgent";
const DEFAULT_RATE_RAMP: &[u32] = &[1, 2, 4, 8, 16, 32, 64, 128, 256];
const DEFAULT_RATE_PERIOD: Duration = Duration::from_secs(60);
const WAITER_READY_TIMEOUT: Duration = Duration::from_secs(60);
const SETUP_CONCURRENCY: usize = 100;

#[derive(Debug, Clone, Copy)]
pub struct CellConfig {
    pub payload_size: usize,
    pub waiter_presence: PromiseWaiterPresence,
    pub fan_in: PromiseFanIn,
    pub topology: PromiseTopology,
}

impl CellConfig {
    pub fn cell_name(&self) -> String {
        format!(
            "promise-{}-{}-{}-{}",
            payload_label(self.payload_size),
            self.waiter_presence,
            self.fan_in,
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
        let count = rate as usize * DEFAULT_RATE_PERIOD.as_secs() as usize;
        println!("Promise-density [{}]: preparing {count} promises for {rate}/s", config.cell_name());
        let work = create_work(&user, &component, config, count).await?;
        println!("Promise-density [{}]: preparing waiters", config.cell_name());
        prepare_waiters(&user, &component, &work).await?;
        println!("Promise-density [{}]: completing at {rate}/s", config.cell_name());
        let period = complete_at_rate(&user, work, payload.clone(), rate).await?;
        outcome.record(rate, period);
    }

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

async fn create_work(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    config: &CellConfig,
    count: usize,
) -> anyhow::Result<Vec<PromiseWork>> {
    stream::iter(0..count)
        .map(|index| {
            let user = user.clone();
            let component = component.clone();
            async move {
        let name = match config.fan_in {
            PromiseFanIn::OnePerAgent => format!("{}-{index}", config.cell_name()),
            PromiseFanIn::FanIn => format!("{}-fan-in", config.cell_name()),
        };
        let parsed_agent = agent_id!(PROMISE_AGENT_TYPE, name);
        let agent = user.start_agent(&component.id, parsed_agent.clone()).await?;
        let result = user
            .invoke_and_await_agent(&component, &parsed_agent, "getPromise", data_value!())
            .await?;
        let promise_value = result
            .into_return_value_and_type()
            .ok_or_else(|| anyhow::anyhow!("getPromise returned no promise id"))?;
        let promise = PromiseId::from_value(promise_value.value)
            .map_err(|error| anyhow::anyhow!("invalid promise id: {error}"))?;
        Ok(PromiseWork {
            agent,
            parsed_agent,
            promise,
            wait: should_wait(config.waiter_presence, index),
        })
            }
        })
        .buffer_unordered(SETUP_CONCURRENCY)
        .try_collect()
        .await
}

async fn prepare_waiters(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    work: &[PromiseWork],
) -> anyhow::Result<()> {
    stream::iter(work.iter().filter(|item| item.wait))
        .map(|item| {
            let user = user.clone();
            let component = component.clone();
            let parsed_agent = item.parsed_agent.clone();
            let promise = item.promise.clone();
            async move {
                user.invoke_agent(
                    &component,
                    &parsed_agent,
                    "awaitPromise",
                    data_value!(promise),
                )
                .await
            }
        })
        .buffer_unordered(SETUP_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;

    // Completion must observe a registered PromiseHandle for warm/mixed cells.
    // A shared fan-in agent can be suspended once while many queued awaiters exist.
    let mut waited_agents = Vec::new();
    for item in work.iter().filter(|item| item.wait) {
        if !waited_agents.contains(&item.agent) {
            user.wait_for_status(&item.agent, AgentStatus::Suspended, WAITER_READY_TIMEOUT)
                .await?;
            waited_agents.push(item.agent.clone());
        }
    }
    Ok(())
}

async fn complete_at_rate(
    user: &TestUserContext<BenchmarkTestDependencies>,
    work: Vec<PromiseWork>,
    payload: Vec<u8>,
    rate: u32,
) -> anyhow::Result<Period> {
    let started = Instant::now();
    let mut completion_latencies = Vec::with_capacity(work.len());
    for (index, item) in work.into_iter().enumerate() {
        tokio::time::sleep_until(
            (started + Duration::from_secs_f64(index as f64 / rate as f64)).into(),
        )
        .await;
        let completion_started = Instant::now();
        user.complete_promise(&item.promise, payload.clone()).await?;
        completion_latencies.push(completion_started.elapsed());
    }
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
            &ResultKey::primary("max-promise-completion-rate-per-sec"),
            self.max_rate as u64,
        );
        for (rate, period) in self.periods {
            recorder.count(
                &ResultKey::primary(format!("promise-completions-at-{rate}-per-sec")),
                period.completed,
            );
            recorder.duration(
                &ResultKey::primary(format!("promise-completion-period-latency-at-{rate}-per-sec")),
                period.elapsed,
            );
            for latency in period.completion_latencies {
                recorder.duration(&ResultKey::primary("promise-completion-latency"), latency);
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
                "Promise-density cell: payload={} bytes, waiter-presence={}, fan-in={}, topology={}",
                config.payload_size, config.waiter_presence, config.fan_in, config.topology
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
