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

//! Cloud density benchmarks (golemcloud/golem#3516).
//!
//! Density benchmarks measure the per-pod ceiling of a single worker-executor
//! under realistic workload mixes. Unlike cloud-perf (which keeps load below
//! saturation across the whole cluster), density deliberately ramps a single
//! axis up to and past the point where the pod falls over, recording the soft,
//! hard, and catastrophic ceilings.
//!
//! The buildspec drives the cell-by-cell loop (each cell runs in its own
//! freshly-restarted, state-wiped executor) and invokes the benchmark binary
//! once per cell via the `density` subcommand. Each invocation runs one cell,
//! ramps its axis internally feeding the [`ceiling`] state machine, and emits
//! one cell `BenchmarkResult` plus an optional timeseries file.
//!
//! v1 ships the agent-density section ([`agent`]). Schedule-density and
//! promise-density reuse [`prep`] and [`ceiling`] and are added later.

pub mod agent;
pub mod ceiling;
pub mod metrics;
pub mod prep;

use clap::ValueEnum;
use std::fmt::{self, Display, Formatter};

/// Which density section a prep/run targets. Selects the component set and the
/// account/app/env naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DensitySection {
    Agent,
    Schedule,
    Promise,
}

impl DensitySection {
    pub fn as_str(self) -> &'static str {
        match self {
            DensitySection::Agent => "agent",
            DensitySection::Schedule => "schedule",
            DensitySection::Promise => "promise",
        }
    }
}

impl Display for DensitySection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Agent durability mode. Scenario 4 (resume-under-saturation) is durable-only
/// because ephemeral agents are not recoverable post-eviction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AgentMode {
    Durable,
    Ephemeral,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentMode::Durable => "durable",
            AgentMode::Ephemeral => "ephemeral",
        }
    }
}

impl Display for AgentMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How agents map to components within a cell.
///
/// - `Shared`: every agent is an instance of one shared component. The cell
///   measures pure resident-agent capacity with no component-cache pressure.
///   Identified in cell names and the result schema by `shared-component`.
/// - `PerAgent`: each agent uses its own component. Density-prep uploads many
///   byte-identical copies of the same WASM under distinct names, so the
///   registry mints a distinct `component_id` per agent. The raw blob is
///   deduplicated in object storage, but the executor's compiled-component
///   cache keys on `component_id` and produces one entry per agent — the
///   compiled-cache-thrash signal this mode exists to measure. Capped at
///   [`prep::PER_AGENT_COMPONENT_COUNT`] components (one per agent), and
///   identified in cell names and the result schema by `per-agent-component`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ComponentSharing {
    Shared,
    PerAgent,
}

impl ComponentSharing {
    /// Label used in cell names and the S3 result schema.
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentSharing::Shared => "shared-component",
            ComponentSharing::PerAgent => "per-agent-component",
        }
    }

    /// Upper bound on the agent ramp for this sharing mode.
    ///
    /// `PerAgent` is capped at the number of distinct components density-prep
    /// uploads (one component per agent). `Shared` is capped at the
    /// resident-agent count we expect the per-pod memory ceiling to be reached
    /// well before.
    pub fn upper_bound(self) -> u32 {
        match self {
            ComponentSharing::Shared => 10_000,
            ComponentSharing::PerAgent => prep::PER_AGENT_COMPONENT_COUNT,
        }
    }
}

impl Display for ComponentSharing {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The agent-count ramp the driver walks for a cell, in increasing order. The
/// driver creates/activates agents up to each step, takes its measurements,
/// then advances — until a catastrophic ceiling fires or the sharing mode's
/// [`ComponentSharing::upper_bound`] is hit.
///
/// Resolution is concentrated around the 500–1500 per-pod knee observed in
/// cloud-perf (halved from the 2-pod cloud-perf numbers since density is
/// single-pod), then coarsens. Steps past the upper bound are dropped.
pub const AGENT_RAMP: &[u32] = &[
    100, 250, 500, 750, 1000, 1500, 2000, 3000, 4000, 6000, 8000, 10000,
];

/// Returns the ramp steps applicable to `sharing`, dropping any step that
/// exceeds the upper bound and always including the upper bound itself as the
/// final step.
pub fn ramp_for(sharing: ComponentSharing) -> Vec<u32> {
    let bound = sharing.upper_bound();
    let mut steps: Vec<u32> = AGENT_RAMP.iter().copied().filter(|&n| n < bound).collect();
    steps.push(bound);
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn ramp_shared_caps_at_10000() {
        let steps = ramp_for(ComponentSharing::Shared);
        assert_eq!(*steps.last().unwrap(), 10_000);
        assert!(steps.windows(2).all(|w| w[0] < w[1]), "strictly increasing");
    }

    #[test]
    fn ramp_per_agent_caps_at_component_count() {
        let steps = ramp_for(ComponentSharing::PerAgent);
        assert_eq!(*steps.last().unwrap(), prep::PER_AGENT_COMPONENT_COUNT);
        assert!(
            steps.iter().all(|&n| n <= prep::PER_AGENT_COMPONENT_COUNT),
            "no step exceeds the per-agent component count"
        );
        assert!(steps.windows(2).all(|w| w[0] < w[1]), "strictly increasing");
    }
}
