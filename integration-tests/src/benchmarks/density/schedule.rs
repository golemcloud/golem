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

//! Schedule-density benchmark configuration (golemcloud/golem#3524).
//!
//! Operators observe scheduler metrics manually in Grafana or Prometheus. In-driver executor
//! metric scraping is unreliable for agent-density benchmarks.

use super::agent::ExecutorProbe;
use super::prep::PrepManifest;
use super::{ScheduleTargetPattern, ScheduleTargetResidency};
use futures::stream::{StreamExt, TryStreamExt};
use golem_common::agent_id;
use golem_common::base_model::agent::ParsedAgentId;
use golem_common::data_value;
use golem_common::model::component::ComponentDto;
use golem_test_framework::benchmark::{
    BenchmarkRecorder, BenchmarkResult, BenchmarkRunResult, ResultKey, RunConfig,
};
use golem_test_framework::config::BenchmarkTestDependencies;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::dsl::TestDsl;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::info;

pub const SCHEDULE_TARGET_AGENT_TYPE: &str = "ScheduleCounter";
pub const SCHEDULE_EMITTER_AGENT_TYPE: &str = "ScheduleEmitter";
pub const REALISTIC_TARGET_COUNT: u32 = 1_000;

const DEFAULT_RATE_RAMP: &[u32] = &[1, 2, 4, 8, 16, 32, 64];
const SCHEDULE_LEAD: Duration = Duration::from_secs(2);
const DELIVERY_GRACE: Duration = Duration::from_secs(2);
pub const DEFAULT_RATE_PERIOD: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct CellConfig {
    pub residency: ScheduleTargetResidency,
    pub context_spans: u32,
    pub target_pattern: ScheduleTargetPattern,
    pub rate_period: Duration,
}

impl CellConfig {
    pub fn cell_name(&self) -> String {
        format!(
            "sched-{}-d{}-{}",
            self.residency, self.context_spans, self.target_pattern
        )
    }
}

pub async fn run_cell(
    config: &CellConfig,
    rate_ramp: Option<&[u32]>,
    manifest: &PrepManifest,
    deps: &BenchmarkTestDependencies,
    probe: &ExecutorProbe,
) -> anyhow::Result<BenchmarkResult> {
    if config.rate_period.is_zero() {
        anyhow::bail!("schedule rate period must be positive");
    }
    let component = resolve_uniform_component(manifest, deps).await?;
    let user = manifest.user_context(deps);
    let rates = validated_rates(rate_ramp.unwrap_or(DEFAULT_RATE_RAMP))?;
    let target_count = target_count(config.target_pattern, *rates.last().unwrap());
    let target_names = target_names(config, target_count);
    let targets = target_ids(&target_names);
    warm_targets(&user, &component, &targets).await?;
    let emitters = emitter_ids(config, *rates.last().unwrap());
    warm_emitters(&user, &component, &emitters).await?;
    let mut outcome = ScheduleOutcome::default();

    match config.residency {
        ScheduleTargetResidency::Warm => {
            for &rate in rates {
                let period = run_rate_period(
                    &user,
                    &component,
                    &emitters,
                    &target_names,
                    config.context_spans,
                    rate,
                    config.rate_period,
                )
                .await?;
                outcome.record_period(rate, config.rate_period, period);
            }
        }
        ScheduleTargetResidency::Cold => {
            let rate = *rates.last().unwrap();
            let batch = schedule_batch(
                &user,
                &component,
                &emitters,
                &target_names,
                config.context_spans,
                rate,
            )
            .await?;
            outcome.record(rate, batch.scheduled.into(), batch.registration_latency);
            probe.restart_executor().await?;
            tokio::time::sleep(SCHEDULE_LEAD + DELIVERY_GRACE).await;
            let recovery_start = Instant::now();
            user.invoke_and_await_agent(&component, &targets[0], "poll", data_value!())
                .await?;
            outcome.recovery_latency = Some(recovery_start.elapsed());
        }
    }

    Ok(outcome.into_benchmark_result(config, target_count))
}

async fn resolve_uniform_component(
    manifest: &PrepManifest,
    deps: &BenchmarkTestDependencies,
) -> anyhow::Result<ComponentDto> {
    let component_id = manifest
        .uniform_component_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("manifest has no schedule component id"))?;
    manifest
        .user_context(deps)
        .get_latest_component_revision(component_id)
        .await
}

fn validated_rates(rates: &[u32]) -> anyhow::Result<&[u32]> {
    if rates.is_empty() || rates.contains(&0) || rates.windows(2).any(|rates| rates[0] >= rates[1])
    {
        anyhow::bail!("schedule rate ramp must contain strictly increasing positive rates");
    }
    Ok(rates)
}

fn target_count(pattern: ScheduleTargetPattern, rate: u32) -> u32 {
    match pattern {
        ScheduleTargetPattern::Spread => rate,
        ScheduleTargetPattern::Realistic => REALISTIC_TARGET_COUNT,
    }
}

fn target_names(config: &CellConfig, count: u32) -> Vec<String> {
    (0..count)
        .map(|index| format!("{}-target-{index}", config.cell_name()))
        .collect()
}

fn target_ids(names: &[String]) -> Vec<ParsedAgentId> {
    names
        .iter()
        .map(|name| agent_id!(SCHEDULE_TARGET_AGENT_TYPE, name.clone()))
        .collect()
}

fn emitter_ids(config: &CellConfig, count: u32) -> Vec<ParsedAgentId> {
    (0..count)
        .map(|index| {
            agent_id!(
                SCHEDULE_EMITTER_AGENT_TYPE,
                format!("{}-emitter-{index}", config.cell_name())
            )
        })
        .collect()
}

async fn warm_targets(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    targets: &[ParsedAgentId],
) -> anyhow::Result<()> {
    for target in targets {
        user.invoke_and_await_agent(component, target, "poll", data_value!())
            .await?;
    }
    Ok(())
}

async fn warm_emitters(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    emitters: &[ParsedAgentId],
) -> anyhow::Result<()> {
    for emitter in emitters {
        user.invoke_and_await_agent(component, emitter, "warm", data_value!())
            .await?;
    }
    Ok(())
}

struct Batch {
    scheduled: u32,
    registration_latency: Duration,
}

async fn schedule_batch(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    emitters: &[ParsedAgentId],
    targets: &[String],
    context_spans: u32,
    rate: u32,
) -> anyhow::Result<Batch> {
    let started = Instant::now();
    let due_start = SystemTime::now() + SCHEDULE_LEAD;
    let registrations = (0..rate).map(|index| {
        let target_name = targets[index as usize % targets.len()].clone();
        let emitter = &emitters[index as usize % emitters.len()];
        let registration_at = started + Duration::from_secs_f64(index as f64 / rate as f64);
        let due_at = due_start + Duration::from_secs_f64(index as f64 / rate as f64);
        async move {
            tokio::time::sleep_until(registration_at.into()).await;
            let (seconds, nanoseconds) = scheduled_at(due_at);
            user.invoke_and_await_agent(
                component,
                emitter,
                "schedule_poll_at",
                data_value!(target_name, seconds, nanoseconds, context_spans),
            )
            .await
        }
    });
    futures::future::try_join_all(registrations).await?;
    Ok(Batch {
        scheduled: rate,
        registration_latency: started.elapsed(),
    })
}

async fn run_rate_period(
    user: &TestUserContext<BenchmarkTestDependencies>,
    component: &ComponentDto,
    emitters: &[ParsedAgentId],
    targets: &[String],
    context_spans: u32,
    rate: u32,
    rate_period: Duration,
) -> anyhow::Result<PeriodOutcome> {
    let started = Instant::now();
    let due_start = SystemTime::now() + SCHEDULE_LEAD;
    let scheduled = expected_actions(rate, rate_period);
    futures::stream::iter(0..scheduled)
        .map(|index| {
            let target_name = targets[index as usize % targets.len()].clone();
            let emitter = &emitters[index as usize % emitters.len()];
            let registration_at = started + Duration::from_secs_f64(index as f64 / rate as f64);
            let due_at = due_start + Duration::from_secs_f64(index as f64 / rate as f64);
            async move {
                tokio::time::sleep_until(registration_at.into()).await;
                let (seconds, nanoseconds) = scheduled_at(due_at);
                user.invoke_and_await_agent(
                    component,
                    emitter,
                    "schedule_poll_at",
                    data_value!(target_name, seconds, nanoseconds, context_spans),
                )
                .await
            }
        })
        .buffer_unordered(emitters.len())
        .try_collect::<Vec<_>>()
        .await?;
    Ok(PeriodOutcome {
        scheduled,
        registration_latency: started.elapsed(),
    })
}

fn expected_actions(rate: u32, rate_period: Duration) -> u64 {
    rate as u64 * rate_period.as_secs()
}

fn scheduled_at(deadline: SystemTime) -> (u64, u32) {
    let since_epoch = deadline
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch");
    (since_epoch.as_secs(), since_epoch.subsec_nanos())
}

#[derive(Default)]
struct PeriodOutcome {
    scheduled: u64,
    registration_latency: Duration,
}

struct RatePeriod {
    rate: u32,
    scheduled: u64,
    expected: u64,
    registration_latency: Duration,
}

#[derive(Default)]
struct ScheduleOutcome {
    scheduled: u64,
    expected_scheduled: u64,
    max_rate: u32,
    registration_latencies: Vec<Duration>,
    rate_periods: Vec<RatePeriod>,
    recovery_latency: Option<Duration>,
}

impl ScheduleOutcome {
    fn record(&mut self, rate: u32, scheduled: u64, registration_latency: Duration) {
        self.scheduled += scheduled;
        self.max_rate = self.max_rate.max(rate);
        self.registration_latencies.push(registration_latency);
    }

    fn record_period(&mut self, rate: u32, rate_period: Duration, period: PeriodOutcome) {
        let expected = expected_actions(rate, rate_period);
        self.record(rate, period.scheduled, period.registration_latency);
        self.expected_scheduled += expected;
        self.rate_periods.push(RatePeriod {
            rate,
            scheduled: period.scheduled,
            expected,
            registration_latency: period.registration_latency,
        });
    }

    fn into_benchmark_result(self, config: &CellConfig, targets: u32) -> BenchmarkResult {
        let recorder = BenchmarkRecorder::new();
        recorder.count(&ResultKey::primary("scheduled-actions"), self.scheduled);
        if self.expected_scheduled > 0 {
            recorder.count(
                &ResultKey::primary("expected-scheduled-actions"),
                self.expected_scheduled,
            );
        }
        recorder.count(
            &ResultKey::primary("max-schedule-rate-per-sec"),
            self.max_rate as u64,
        );
        recorder.count(&ResultKey::primary("schedule-target-count"), targets as u64);
        recorder.count(
            &ResultKey::primary("schedule-context-spans"),
            config.context_spans as u64,
        );
        for latency in self.registration_latencies {
            recorder.duration(
                &ResultKey::primary("schedule-registration-period-latency"),
                latency,
            );
        }
        for period in self.rate_periods {
            recorder.count(
                &ResultKey::primary(format!(
                    "expected-scheduled-actions-at-{}-per-sec",
                    period.rate
                )),
                period.expected,
            );
            recorder.count(
                &ResultKey::primary(format!("scheduled-actions-at-{}-per-sec", period.rate)),
                period.scheduled,
            );
            recorder.duration(
                &ResultKey::primary(format!(
                    "schedule-registration-period-latency-at-{}-per-sec",
                    period.rate
                )),
                period.registration_latency,
            );
        }
        if let Some(latency) = self.recovery_latency {
            recorder.duration(&ResultKey::primary("cold-recovery-latency"), latency);
        }
        let run_config = RunConfig {
            cluster_size: 0,
            size: self.max_rate as usize,
            length: 0,
            disable_compilation_cache: false,
        };
        let mut run_result = BenchmarkRunResult::new(run_config.clone());
        run_result.add(recorder);
        if self.expected_scheduled > 0 {
            info!(
                "Density-schedule[{}]: scheduled {} of {} expected actions at max {} per second across {} targets",
                config.cell_name(),
                self.scheduled,
                self.expected_scheduled,
                self.max_rate,
                targets
            );
        } else {
            info!(
                "Density-schedule[{}]: scheduled {} actions at max {} per second across {} targets",
                config.cell_name(),
                self.scheduled,
                self.max_rate,
                targets
            );
        }
        BenchmarkResult {
            name: format!("density-schedule-{}", config.cell_name()),
            description: format!(
                "Schedule-density cell: residency={}, context-spans={}, target-pattern={}",
                config.residency, config.context_spans, config.target_pattern
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
    fn rate_validation_rejects_non_geometric_ordering_errors() {
        assert!(validated_rates(&[]).is_err());
        assert!(validated_rates(&[1, 1]).is_err());
        assert_eq!(validated_rates(&[1, 2, 4]).unwrap(), &[1, 2, 4]);
    }

    #[test]
    fn expected_actions_match_the_default_rate_ramp() {
        assert_eq!(expected_actions(64, DEFAULT_RATE_PERIOD), 3_840);
        assert_eq!(expected_actions(64, Duration::from_secs(30 * 60)), 115_200);
        assert_eq!(
            DEFAULT_RATE_RAMP
                .iter()
                .map(|rate| expected_actions(*rate, DEFAULT_RATE_PERIOD))
                .sum::<u64>(),
            7_620
        );
    }
}
