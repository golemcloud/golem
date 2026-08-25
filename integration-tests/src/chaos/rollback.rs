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

//! Did every agent come back to the build it was rolled back to (GOL-369)?
//!
//! S5 asks whether agents reach a *new* build when an executor dies mid-update.
//! S9 asks the return question, and the return leg is the one that matters
//! operationally: a rollback is what you reach for when the new build is
//! already going wrong, so it happening under a dying executor is exactly the
//! situation you would be in.
//!
//! ### Why the evidence is the running code, not the metadata
//!
//! `Counter::component_version` is compiled into each build — `1` in
//! `agent-counters`, `2` in `agent-counters-v2`, and nothing else differs
//! between them. Component metadata says which revision the platform *believes*
//! an agent is on; invoking `component_version` says what the code actually
//! executing reports. Only the second can distinguish a rollback that landed
//! from one the platform merely recorded.
//!
//! ### Why the forward leg is verified before the backward one
//!
//! If the agents never reached the new build, rolling them back returns them to
//! a build they never left, every check passes, and the run proves nothing. So
//! the forward leg is measured and the rollback is refused outright if too few
//! agents made it. The same instinct as S6's smoke round and S10's
//! how-much-did-the-kill-catch line: a clean report from a scenario that never
//! happened is the worst artifact this suite can produce.
//!
//! ### Why the control-plane retries are counted apart
//!
//! The workload's retry is one attempt, transport-only, under the original
//! idempotency key, and it exists to *expose* duplicate execution. The
//! rollback's per-agent update requests are a different thing entirely: they are
//! control-plane calls aimed at agents whose executor is about to be killed, and
//! a request refused because its owner just died says nothing about the
//! platform's correctness. Counting them together would let control-plane noise
//! read as workload trouble.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What the agents were running when asked.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionCensus {
    /// When the census was taken, for the report to label it.
    pub at: String,
    /// The version every agent was expected to report.
    pub expected: u32,
    pub agents: usize,
    pub on_expected: usize,
    /// Agents on some other build, by the version they reported. Non-empty is
    /// the finding; the key says which build they are stuck on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub on_other: BTreeMap<u32, usize>,
    /// Agents that answered nothing. Neither passed nor failed: an agent that
    /// cannot be read says nothing either way about which build it is on.
    pub unreadable: usize,
}

impl VersionCensus {
    /// Builds a census from what each agent reported.
    pub fn build(at: &str, expected: u32, observed: &BTreeMap<String, Option<u32>>) -> Self {
        let mut on_expected = 0;
        let mut unreadable = 0;
        let mut on_other: BTreeMap<u32, usize> = BTreeMap::new();
        for version in observed.values() {
            match version {
                Some(v) if *v == expected => on_expected += 1,
                Some(v) => *on_other.entry(*v).or_default() += 1,
                None => unreadable += 1,
            }
        }
        VersionCensus {
            at: at.to_string(),
            expected,
            agents: observed.len(),
            on_expected,
            on_other,
            unreadable,
        }
    }

    /// The share of agents on the expected build, out of those that answered.
    ///
    /// Unreadable agents are excluded from both halves rather than counted as
    /// failures: they are reported separately, and treating silence as a wrong
    /// answer would let a flaky read block a rollback that was fine.
    pub fn share_of_answered_percent(&self) -> Option<f64> {
        let answered = self.agents - self.unreadable;
        (answered > 0).then(|| self.on_expected as f64 * 100.0 / answered as f64)
    }
}

/// Rollback requests, counted apart from the workload's own retries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneAttempts {
    pub requested: u64,
    pub accepted_first_try: u64,
    pub accepted_after_retry: u64,
    /// Requests that never got through, even after the configured retries. Each
    /// one is an agent nobody asked to come back, so it explains a stale agent
    /// without excusing one.
    pub refused: u64,
    pub max_retries: u32,
}

impl ControlPlaneAttempts {
    pub fn accepted(&self) -> u64 {
        self.accepted_first_try + self.accepted_after_retry
    }
}

/// The rollback account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackReport {
    /// The revision the agents were moved forward to, and the one they were
    /// rolled back to. The second carries the original build's code.
    pub forward_revision: u64,
    pub rollback_revision: u64,
    /// What the running code reports on each of those builds.
    pub forward_version: u32,
    pub rollback_version: u32,
    /// The forward leg, measured before the rollback was attempted.
    pub rolled_forward: VersionCensus,
    /// After recovery. `None` if the run aborted before it got there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back: Option<VersionCensus>,
    pub control: ControlPlaneAttempts,
    /// The floor the forward leg had to clear for the rollback to be worth
    /// attempting, from the suite YAML.
    pub rolled_forward_floor_percent: f64,
}

impl RollbackReport {
    /// Whether enough agents reached the new build for a rollback to mean
    /// anything.
    pub fn forward_leg_landed(&self) -> bool {
        self.rolled_forward
            .share_of_answered_percent()
            .is_some_and(|share| share >= self.rolled_forward_floor_percent)
    }

    /// Agents still on the build they were supposed to leave, after recovery.
    pub fn stuck_on_the_new_build(&self) -> usize {
        self.rolled_back
            .as_ref()
            .and_then(|census| census.on_other.get(&self.forward_version).copied())
            .unwrap_or(0)
    }

    /// The lines that need a human.
    pub fn attention_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        if !self.forward_leg_landed() {
            lines.push(format!(
                "S9: only {} of {} agents reached revision {} before the rollback ({}% of those \
                 that answered, against a {:.0}% floor). Rolling agents back to a build they \
                 never left proves nothing, so this run does not test rollback.",
                self.rolled_forward.on_expected,
                self.rolled_forward.agents,
                self.forward_revision,
                self.rolled_forward
                    .share_of_answered_percent()
                    .map(|s| format!("{s:.1}"))
                    .unwrap_or_else(|| "n/a".to_string()),
                self.rolled_forward_floor_percent
            ));
        }

        let stuck = self.stuck_on_the_new_build();
        if stuck > 0 {
            lines.push(format!(
                "S9: {stuck} agent(s) still report component version {} after recovery, not the \
                 {} they were rolled back to",
                self.forward_version, self.rollback_version
            ));
        }

        if self.control.refused > 0 {
            lines.push(format!(
                "S9: {} of {} rollback requests were refused even after {} control-plane \
                 retries. Those agents were never asked to come back, which explains a stale \
                 agent without excusing one.",
                self.control.refused, self.control.requested, self.control.max_retries
            ));
        }

        if let Some(census) = &self.rolled_back
            && census.unreadable > 0
        {
            lines.push(format!(
                "S9: {} agent(s) could not be read after recovery, so the run cannot say which \
                 build they are on",
                census.unreadable
            ));
        }
        lines
    }

    /// Lines a reader needs in order to interpret the run.
    pub fn note_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "S9: revision {} carries build v{}, revision {} carries build v{} again",
            self.forward_revision,
            self.forward_version,
            self.rollback_revision,
            self.rollback_version
        )];
        lines.push(format!(
            "S9 forward leg: {} of {} agents on version {} before the rollback ({} unreadable)",
            self.rolled_forward.on_expected,
            self.rolled_forward.agents,
            self.forward_version,
            self.rolled_forward.unreadable
        ));
        lines.push(format!(
            "S9 rollback requests: {} asked, {} accepted first try, {} after a retry, {} refused \
             (up to {} control-plane retries)",
            self.control.requested,
            self.control.accepted_first_try,
            self.control.accepted_after_retry,
            self.control.refused,
            self.control.max_retries
        ));
        if let Some(census) = &self.rolled_back {
            lines.push(format!(
                "S9 return leg: {} of {} agents on version {} after recovery ({} unreadable, \
                 {:?} elsewhere)",
                census.on_expected,
                census.agents,
                census.expected,
                census.unreadable,
                census.on_other
            ));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn census(
        expected: u32,
        on_expected: usize,
        on_other: &[(u32, usize)],
        unreadable: usize,
    ) -> VersionCensus {
        let mut observed: BTreeMap<String, Option<u32>> = BTreeMap::new();
        let mut n = 0;
        for _ in 0..on_expected {
            observed.insert(format!("agent-{n:04}"), Some(expected));
            n += 1;
        }
        for (version, count) in on_other {
            for _ in 0..*count {
                observed.insert(format!("agent-{n:04}"), Some(*version));
                n += 1;
            }
        }
        for _ in 0..unreadable {
            observed.insert(format!("agent-{n:04}"), None);
            n += 1;
        }
        VersionCensus::build("test", expected, &observed)
    }

    fn report(forward: VersionCensus, back: Option<VersionCensus>) -> RollbackReport {
        RollbackReport {
            forward_revision: 2,
            rollback_revision: 3,
            forward_version: 2,
            rollback_version: 1,
            rolled_forward: forward,
            rolled_back: back,
            control: ControlPlaneAttempts {
                requested: 200,
                accepted_first_try: 200,
                max_retries: 2,
                ..Default::default()
            },
            rolled_forward_floor_percent: 90.0,
        }
    }

    /// An unreadable agent is not a wrong answer. Counting it as one would let a
    /// flaky read block a rollback that was perfectly fine.
    #[test]
    fn silence_is_excluded_from_the_share_rather_than_counted_against_it() {
        // 90 on the expected build, 10 silent, none actually wrong.
        let c = census(2, 90, &[], 10);
        assert_eq!(c.agents, 100);
        assert_eq!(c.unreadable, 10);
        assert_eq!(c.share_of_answered_percent(), Some(100.0));
        assert!(report(c, None).forward_leg_landed());
    }

    /// The gate the whole scenario rests on: rolling agents back to a build
    /// they never left would pass every check downstream.
    #[test]
    fn a_forward_leg_that_did_not_land_refuses_the_rollback() {
        // 50 of 100 answered agents made it, against a 90% floor.
        let r = report(census(2, 50, &[(1, 50)], 0), None);
        assert!(!r.forward_leg_landed());
        assert!(
            r.attention_lines()
                .iter()
                .any(|l| l.contains("does not test rollback")),
            "the operator has to be told the run proved nothing: {:?}",
            r.attention_lines()
        );
    }

    /// A census with nothing readable cannot clear the gate, rather than
    /// clearing it vacuously on an empty average.
    #[test]
    fn a_census_nobody_answered_does_not_clear_the_gate() {
        let c = census(2, 0, &[], 40);
        assert_eq!(c.share_of_answered_percent(), None);
        assert!(!report(c, None).forward_leg_landed());
    }

    /// The finding: agents still on the build they were rolled back from.
    #[test]
    fn agents_still_on_the_old_build_after_recovery_are_raised() {
        let r = report(census(2, 200, &[], 0), Some(census(1, 197, &[(2, 3)], 0)));
        assert_eq!(r.stuck_on_the_new_build(), 3);
        assert!(
            r.attention_lines()
                .iter()
                .any(|l| l.contains("still report component version 2")),
            "{:?}",
            r.attention_lines()
        );
    }

    /// A clean return raises nothing.
    #[test]
    fn a_rollback_that_landed_everywhere_raises_nothing() {
        let r = report(census(2, 200, &[], 0), Some(census(1, 200, &[], 0)));
        assert_eq!(r.stuck_on_the_new_build(), 0);
        assert!(r.attention_lines().is_empty(), "{:?}", r.attention_lines());
    }

    /// A refused control-plane request explains a stale agent without excusing
    /// one, so it is raised even when the return leg otherwise looks clean.
    #[test]
    fn refused_rollback_requests_are_raised_even_on_a_clean_return() {
        let mut r = report(census(2, 200, &[], 0), Some(census(1, 200, &[], 0)));
        r.control = ControlPlaneAttempts {
            requested: 200,
            accepted_first_try: 190,
            accepted_after_retry: 6,
            refused: 4,
            max_retries: 2,
        };
        assert_eq!(r.control.accepted(), 196);
        assert!(
            r.attention_lines()
                .iter()
                .any(|l| l.contains("were refused even after")),
            "{:?}",
            r.attention_lines()
        );
    }

    /// Retries are counted apart from first-try acceptances, because the two
    /// say different things about how the control plane behaved under a kill.
    #[test]
    fn first_try_and_retried_acceptances_are_counted_apart() {
        let control = ControlPlaneAttempts {
            requested: 10,
            accepted_first_try: 7,
            accepted_after_retry: 2,
            refused: 1,
            max_retries: 2,
        };
        assert_eq!(control.accepted(), 9);
        assert_eq!(control.accepted() + control.refused, control.requested);
    }
}
