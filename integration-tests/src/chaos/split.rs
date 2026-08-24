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

//! Aiming a kill at one executor while still driving the agents it does not own.
//!
//! [`crate::chaos::pinned`] answers a different question. There, an operation
//! that was not on the dead pod says nothing, so the driver keeps only the
//! agents its chosen executor owns and discards the rest. Here every agent is
//! driven and the ones elsewhere are the run's own control group: on a
//! two-executor cluster roughly half the population is never touched, and
//! reporting one percentile across both would let a recovery that took its full
//! budget hide behind the half that was never disturbed.
//!
//! Both scenarios that work this way — S10's schedule targets and S11's promise
//! waiters — need exactly the same three things, which is why they live here
//! rather than being written twice: pick the executor owning the largest share,
//! refuse to proceed if that share is too small to mean anything, and re-check
//! the division immediately before the fault is injected.

use crate::chaos::pinned::{owners_by_pod_in, pod_ip_of, routing_agent_id_in};
use crate::chaos::workload::WorkloadContext;
use anyhow::Context;
use chrono::{DateTime, Utc};
use golem_common::model::component::ComponentDto;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::info;

/// The smallest share of agents one executor must own for the run to mean
/// anything, as a divisor of the population.
///
/// A two-executor cluster splits a hashed population roughly evenly, so a
/// quarter is a floor rather than an expectation. Below it the "affected" group
/// is too small for its percentile to say anything, and a run that reported one
/// anyway would be worse than one that refused.
const MIN_SHARE_DIVISOR: usize = 4;

/// What the split is about, for the messages a reader eventually sees.
///
/// Carried rather than hard-coded because the failure modes here are reported to
/// an operator mid-maintenance-window, and "the most-loaded executor owns only 6
/// of 100 agents" is a worse thing to read at 3am than the same sentence naming
/// promise waiters.
#[derive(Debug, Clone, Copy)]
pub struct Subject<'a> {
    /// Scenario code, used only to prefix log lines.
    pub scenario: &'a str,
    /// The component the agents live in. Ownership is per agent id and an agent
    /// id contains its component, so this is not cosmetic.
    pub component: &'a ComponentDto,
    /// Agent type, e.g. `ScheduleCounter`.
    pub agent_type: &'a str,
    /// Plural noun for messages, e.g. `schedule targets`.
    pub noun: &'a str,
}

/// The fault window, as the workflow reported it.
#[derive(Debug, Clone, Copy)]
pub struct FaultWindow {
    pub injected_at: DateTime<Utc>,
    /// Absent for a run that never saw the fault clear.
    pub recovered_at: Option<DateTime<Utc>>,
}

/// Which side of the fault an event fell on.
///
/// Shared rather than written per scenario because the classification is the
/// same question every time — an event before the kill, while the executor was
/// gone, or after it came back — and because the three names end up in archived
/// results that a reader compares across scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Window {
    BeforeFault,
    DuringFault,
    AfterFault,
    /// The run never learned when the fault was injected, so nothing can be
    /// placed relative to it.
    Unknown,
}

impl Window {
    pub fn as_str(self) -> &'static str {
        match self {
            Window::BeforeFault => "before-fault",
            Window::DuringFault => "during-fault",
            Window::AfterFault => "after-fault",
            Window::Unknown => "unknown",
        }
    }

    /// Where `at` falls relative to the fault.
    pub fn of(at: DateTime<Utc>, fault: Option<FaultWindow>) -> Self {
        match fault {
            None => Window::Unknown,
            Some(window) if at < window.injected_at => Window::BeforeFault,
            Some(FaultWindow {
                recovered_at: Some(recovered),
                ..
            }) if at >= recovered => Window::AfterFault,
            Some(_) => Window::DuringFault,
        }
    }
}

impl std::fmt::Display for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The executor the fault will be aimed at, and how the agents divide around it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PodSplit {
    /// The executor endpoint as the shard-manager names it, e.g.
    /// `10.0.14.207:9000`.
    pub pod_address: String,
    /// Host part of the address, which is what a Kubernetes `status.podIP`
    /// field selector matches.
    pub pod_ip: String,
    /// Agents this executor owns: the population that has to survive recovery.
    pub on_pod: Vec<String>,
    /// Agents owned by any other executor: the run's own control group.
    pub elsewhere: Vec<String>,
    /// How the agents spread across executors, so a run that refused to proceed
    /// says whether the cluster was lopsided or the pool too small.
    pub targets_per_pod: BTreeMap<String, usize>,
    /// Shard count the routing table reported. Ownership is a hash modulo this,
    /// so a selection cannot be re-derived later without it.
    pub number_of_shards: usize,
}

impl PodSplit {
    /// Which group an agent belongs to, or `None` for one the selection never
    /// saw.
    pub fn group_of(&self, agent: &str) -> Option<Group> {
        if self.on_pod.iter().any(|a| a == agent) {
            Some(Group::OnPod)
        } else if self.elsewhere.iter().any(|a| a == agent) {
            Some(Group::Elsewhere)
        } else {
            None
        }
    }
}

/// Which side of the kill an agent was on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// Owned by the executor the fault was aimed at.
    OnPod,
    /// Owned by some other executor: the control group.
    Elsewhere,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Group::OnPod => "on-pod",
            Group::Elsewhere => "elsewhere",
        }
    }
}

/// Chooses the executor to aim at: the one owning the largest share of agents.
///
/// Fails rather than proceeding unaimed. Chaos Mesh's `mode: one` would pick a
/// pod at random, and a run that killed an executor owning six agents out of a
/// hundred would still produce a confident-looking report about recovery.
pub async fn select(
    subject: Subject<'_>,
    deps: &BenchmarkTestDependencies,
    agents: &[String],
) -> anyhow::Result<PodSplit> {
    let table = deps
        .shard_manager()
        .get_routing_table()
        .await
        .with_context(|| {
            format!(
                "reading the routing table to aim the {} fault",
                subject.noun
            )
        })?;

    let by_pod = owners_by_pod_in(subject.component, &table, subject.agent_type, agents);
    let targets_per_pod: BTreeMap<String, usize> = by_pod
        .iter()
        .map(|(pod, xs)| (pod.clone(), xs.len()))
        .collect();

    let (pod_address, on_pod) = by_pod
        .iter()
        .max_by_key(|(_, agents)| agents.len())
        .map(|(pod, agents)| (pod.clone(), agents.clone()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "routing table assigned none of the {} {} to any executor",
                agents.len(),
                subject.noun
            )
        })?;

    let floor = (agents.len() / MIN_SHARE_DIVISOR).max(1);
    if on_pod.len() < floor {
        anyhow::bail!(
            "the most-loaded executor owns only {} of {} {}, below the {floor} needed for its \
             share to be worth measuring: {targets_per_pod:?}",
            on_pod.len(),
            agents.len(),
            subject.noun
        );
    }

    let elsewhere: Vec<String> = agents
        .iter()
        .filter(|t| !on_pod.contains(t))
        .cloned()
        .collect();

    info!(
        "{}: aiming at executor {pod_address}, which owns {} of {} {} ({} elsewhere, across {} \
         executors)",
        subject.scenario,
        on_pod.len(),
        agents.len(),
        subject.noun,
        elsewhere.len(),
        targets_per_pod.len()
    );

    Ok(PodSplit {
        pod_ip: pod_ip_of(&pod_address),
        pod_address,
        on_pod,
        elsewhere,
        targets_per_pod,
        number_of_shards: table.number_of_shards.value,
    })
}

/// Re-checks, against a freshly read routing table, that the agents are still
/// divided the way the selection says.
///
/// Called immediately before the readiness signal, for the same reason
/// [`crate::chaos::pinned`] does it: a rebalance between selection and injection
/// would leave the run reporting a control group that was actually the affected
/// one.
pub async fn verify_ownership(
    subject: Subject<'_>,
    deps: &BenchmarkTestDependencies,
    split: &PodSplit,
) -> anyhow::Result<()> {
    let table = deps
        .shard_manager()
        .get_routing_table()
        .await
        .with_context(|| {
            format!(
                "re-reading the routing table to verify {} ownership",
                subject.noun
            )
        })?;

    let mut drifted = Vec::new();
    for agent in &split.on_pod {
        let owner = table
            .lookup(&routing_agent_id_in(
                subject.component,
                subject.agent_type,
                agent,
            ))
            .map(|pod| pod.to_string());
        if owner.as_deref() != Some(split.pod_address.as_str()) {
            drifted.push(format!(
                "{agent} now owned by {}",
                owner.unwrap_or_else(|| "nobody".to_string())
            ));
        }
    }

    if !drifted.is_empty() {
        anyhow::bail!(
            "{} of {} {} are no longer owned by {}: {}",
            drifted.len(),
            split.on_pod.len(),
            subject.noun,
            split.pod_address,
            drifted.join(", ")
        );
    }

    info!(
        "{}: verified all {} {} are still owned by {}",
        subject.scenario,
        split.on_pod.len(),
        subject.noun,
        split.pod_address
    );
    Ok(())
}

/// The counters component's schedule targets, as S10 aims at them.
pub fn schedule_subject<'a>(ctx: &'a WorkloadContext) -> Subject<'a> {
    Subject {
        scenario: "S10",
        component: &ctx.counters,
        agent_type: crate::chaos::workload::SCHEDULE_COUNTER_AGENT,
        noun: "schedule targets",
    }
}

/// The counters component's durable agents, as S3 aims at them.
///
/// The same agent type the mixed workload's durable stream drives, because it
/// is the same population: S3's emitters exist to pace it per agent, not to
/// invent a new kind of agent.
pub fn counter_subject<'a>(ctx: &'a WorkloadContext) -> Subject<'a> {
    Subject {
        scenario: "S3",
        component: &ctx.counters,
        agent_type: crate::chaos::workload::COUNTER_AGENT,
        noun: "counter agents",
    }
}

/// The promise component's waiters, as S11 aims at them.
pub fn waiter_subject<'a>(ctx: &'a WorkloadContext) -> Subject<'a> {
    Subject {
        scenario: "S11",
        component: &ctx.promise,
        agent_type: crate::chaos::waiters::PROMISE_WAITER_AGENT,
        noun: "promise waiters",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn split() -> PodSplit {
        PodSplit {
            pod_address: "10.0.1.1:9000".to_string(),
            pod_ip: "10.0.1.1".to_string(),
            on_pod: vec!["a".to_string(), "b".to_string()],
            elsewhere: vec!["c".to_string()],
            targets_per_pod: BTreeMap::new(),
            number_of_shards: 1024,
        }
    }

    #[test]
    fn a_split_places_each_agent_in_exactly_one_group() {
        let split = split();
        assert_eq!(split.group_of("a"), Some(Group::OnPod));
        assert_eq!(split.group_of("c"), Some(Group::Elsewhere));
    }

    /// An agent the selection never saw is not silently counted as a control:
    /// the caller has to decide what an unknown agent means, because in every
    /// scenario here it means the population drifted.
    #[test]
    fn an_agent_outside_the_selection_belongs_to_no_group() {
        assert_eq!(split().group_of("z"), None);
    }
}
