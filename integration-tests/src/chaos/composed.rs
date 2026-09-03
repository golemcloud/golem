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

//! Two faults held at once, and whether they really were (GOL-381).
//!
//! Every other scenario in the suite injects one fault and reads the platform's
//! answer to it. The `MF` codes inject a second one inside the first one's
//! window, and that changes what can go wrong with the *run* rather than with
//! the platform: the two faults can miss each other. A kill that lands after
//! the storage came back is a perfectly ordinary S16 followed by a perfectly
//! ordinary S8, and it produces a result that looks exactly like a composed run
//! whose composition worked.
//!
//! So this module reports the shape of the composition rather than any
//! behaviour of the platform. It answers three questions, in the order they
//! stop mattering:
//!
//! 1. Did the second fault land at all?
//! 2. Did it land inside the first one's window?
//! 3. Was it inside for long enough to mean anything?
//!
//! None of them is about golem. All three are about whether the rest of the
//! report is worth reading, which is the same job [`crate::chaos::relay`]'s
//! pairing gate does for S2 and the forward-leg gate does for S9. The
//! difference is that those two can refuse before the window is spent, and this
//! one cannot: the driver has already handed the fault window to the workflow
//! by the time the second fault is due.
//!
//! ## Why the overlap is measured from the kill to the heal
//!
//! The two faults are not the same shape. The enclosing fault is a *condition*
//! that lasts from injection to heal: for MF1, storage is unreachable for
//! exactly as long as the NetworkChaos rules are installed. The inner fault is
//! an *instant* whose consequence persists: `pod-kill` deletes the pod once, and
//! what lasts is the cluster being one executor short.
//!
//! Overlap is therefore how much of the enclosing window the cluster spent
//! short-handed — from the kill to the heal — and not the intersection of two
//! durations. A `duration` on the inner PodChaos governs how long Chaos Mesh
//! considers its experiment running and has nothing to do with how long the pod
//! is gone.
//!
//! ## What is deliberately not here
//!
//! Whether the killed executor came back, and how long it took. That is not
//! knowable from the fault signals — the workflow reports what it asked the
//! cluster to do, not what the cluster did about it — and it is already
//! answered better elsewhere: the ownership samples show the shards leaving the
//! killed executor and returning, with timestamps, and the read-back shows
//! whether anything was lost on the way. Re-deriving a weaker version of that
//! from two timestamps would give a reader a second number to reconcile with
//! the first.

use crate::chaos::signal::FaultInjected;
use crate::chaos::split::round2;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One of the two faults, as the workflow reported it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposedLeg {
    /// Identifier of the Chaos Mesh object, so a leftover can be traced back.
    pub fault_id: String,
    /// Fault kind as the workflow named it, e.g. `pod-kill`.
    pub kind: String,
    /// What it was aimed at: a deployment name for an unpinned fault, a pod
    /// name for a pinned one.
    pub target: String,
    pub injected_at: DateTime<Utc>,
}

impl From<&FaultInjected> for ComposedLeg {
    fn from(signal: &FaultInjected) -> Self {
        Self {
            fault_id: signal.fault_id.clone(),
            kind: signal.kind.clone(),
            target: signal.target.clone(),
            injected_at: signal.injected_at,
        }
    }
}

/// A way the composition failed to be a composition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComposedViolation {
    /// No second fault was ever reported. The run injected one fault and is a
    /// single-fault scenario wearing an `MF` code.
    SecondaryNeverInjected,
    /// The second fault landed before the first one was injected, or after it
    /// healed. Two faults in sequence rather than one composed fault.
    SecondaryOutsidePrimary,
    /// The second fault landed inside the window but too near its end for the
    /// combination to have been held for any length of time.
    OverlapTooShort,
}

impl ComposedViolation {
    pub fn as_str(self) -> &'static str {
        match self {
            ComposedViolation::SecondaryNeverInjected => "secondary-never-injected",
            ComposedViolation::SecondaryOutsidePrimary => "secondary-outside-primary",
            ComposedViolation::OverlapTooShort => "overlap-too-short",
        }
    }
}

impl std::fmt::Display for ComposedViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One violation, with the evidence for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposedFinding {
    pub violation: ComposedViolation,
    pub detail: String,
}

/// How the two faults lined up.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposedFaultReport {
    /// The fault whose window the run's phases follow.
    pub primary: ComposedLeg,
    /// The fault injected inside it. Absent when it never landed, which is
    /// itself the first finding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<ComposedLeg>,
    /// Seconds from the first injection to the second. Negative means the
    /// second one landed first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_offset_secs: Option<f64>,
    /// Seconds the cluster spent under both faults: from the second injection
    /// to the first fault's heal.
    ///
    /// `None` when the run never saw a heal, which is an abort rather than a
    /// short overlap and is why the two cases are not folded together.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlap_secs: Option<f64>,
    /// That overlap as a share of the enclosing window, which is the figure
    /// that survives a change of window length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlap_percent: Option<f64>,
    /// The floor the overlap is judged against, echoed so the report can be
    /// read without the suite YAML.
    pub overlap_floor_secs: u64,
    pub findings: Vec<ComposedFinding>,
}

impl ComposedFaultReport {
    /// Reduces the two fault signals into the account.
    ///
    /// `primary_recovered_at` is the enclosing fault's heal. It is separate
    /// from the leg because the driver learns it later, and because a run that
    /// aborted before the heal still has a primary worth reporting.
    pub fn build(
        primary: &FaultInjected,
        primary_recovered_at: Option<DateTime<Utc>>,
        secondary: Option<&FaultInjected>,
        overlap_floor: Duration,
    ) -> Self {
        let floor_secs = overlap_floor.as_secs();
        let mut findings = Vec::new();

        let Some(secondary) = secondary else {
            findings.push(ComposedFinding {
                violation: ComposedViolation::SecondaryNeverInjected,
                detail: format!(
                    "the second fault never reported itself active, so {} ran on its own and \
                     nothing in this result describes two faults held at once",
                    primary.kind
                ),
            });
            return Self {
                primary: primary.into(),
                secondary: None,
                secondary_offset_secs: None,
                overlap_secs: None,
                overlap_percent: None,
                overlap_floor_secs: floor_secs,
                findings,
            };
        };

        let offset = secs_between(primary.injected_at, secondary.injected_at);
        let overlap = primary_recovered_at.map(|at| secs_between(secondary.injected_at, at));
        let window = primary_recovered_at.map(|at| secs_between(primary.injected_at, at));

        // Ordering first. An overlap computed from a second fault that landed
        // outside the window is a number with no meaning, so the two findings
        // are exclusive rather than cumulative.
        if offset < 0.0 {
            findings.push(ComposedFinding {
                violation: ComposedViolation::SecondaryOutsidePrimary,
                detail: format!(
                    "{} landed {}s before {} was injected, so the two faults ran in sequence",
                    secondary.kind,
                    round2(-offset),
                    primary.kind
                ),
            });
        } else if overlap.is_some_and(|overlap| overlap <= 0.0) {
            findings.push(ComposedFinding {
                violation: ComposedViolation::SecondaryOutsidePrimary,
                detail: format!(
                    "{} landed {}s after {} was injected, which is at or past its heal, so the \
                     two faults ran in sequence",
                    secondary.kind,
                    round2(offset),
                    primary.kind
                ),
            });
        } else if let Some(overlap) = overlap
            && overlap < floor_secs as f64
        {
            findings.push(ComposedFinding {
                violation: ComposedViolation::OverlapTooShort,
                detail: format!(
                    "the cluster was under both faults for {}s, short of the {floor_secs}s this \
                     run is judged by, so whatever the combination does had almost no time to \
                     happen in",
                    round2(overlap)
                ),
            });
        }

        Self {
            primary: primary.into(),
            secondary: Some(secondary.into()),
            secondary_offset_secs: Some(round2(offset)),
            overlap_secs: overlap.map(round2),
            overlap_percent: match (overlap, window) {
                (Some(overlap), Some(window)) if window > 0.0 && overlap > 0.0 => {
                    Some(round2(100.0 * overlap / window))
                }
                _ => None,
            },
            overlap_floor_secs: floor_secs,
            findings,
        }
    }

    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }

    /// Lines an operator has to read.
    pub fn attention_lines(&self) -> Vec<String> {
        self.findings
            .iter()
            .map(|f| format!("{}: {}", f.violation, f.detail))
            .collect()
    }

    /// Lines that make the run readable without being problems themselves.
    ///
    /// The composition's own shape belongs here even on a clean run: every
    /// number below it in the report was measured on a cluster under two
    /// faults, and a reader who does not know when the second one landed cannot
    /// place any of them.
    pub fn note_lines(&self) -> Vec<String> {
        let Some(secondary) = &self.secondary else {
            return Vec::new();
        };

        let mut lines = vec![format!(
            "{} on {} landed {}s into the {} on {}",
            secondary.kind,
            secondary.target,
            self.secondary_offset_secs.unwrap_or_default(),
            self.primary.kind,
            self.primary.target
        )];

        if let (Some(overlap), Some(percent)) = (self.overlap_secs, self.overlap_percent) {
            lines.push(format!(
                "both faults were in force for {overlap}s, {percent}% of the enclosing window"
            ));
        }

        lines
    }
}

/// Seconds from `from` to `to`, negative when `to` is earlier.
fn secs_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_milliseconds() as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use test_r::test;

    const FLOOR: Duration = Duration::from_secs(20);

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn signal(kind: &str, at: DateTime<Utc>) -> FaultInjected {
        FaultInjected {
            fault_id: format!("chaos-mf1-{kind}"),
            kind: kind.to_string(),
            target: "worker-executor".to_string(),
            injected_at: at,
        }
    }

    fn violations(report: &ComposedFaultReport) -> Vec<ComposedViolation> {
        report.findings.iter().map(|f| f.violation).collect()
    }

    /// The shape the scenario exists to produce: the kill lands halfway through
    /// the outage and the cluster is short-handed for the rest of it.
    #[test]
    fn a_kill_inside_the_window_is_clean_and_reports_its_overlap() {
        let primary = signal("network-partition", t0());
        let secondary = signal("pod-kill", t0() + TimeDelta::seconds(30));
        let report = ComposedFaultReport::build(
            &primary,
            Some(t0() + TimeDelta::seconds(60)),
            Some(&secondary),
            FLOOR,
        );

        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.secondary_offset_secs, Some(30.0));
        assert_eq!(report.overlap_secs, Some(30.0));
        assert_eq!(report.overlap_percent, Some(50.0));
    }

    /// The failure this whole module exists for. Without it the run reads as a
    /// clean composed result, because every other account in the report is
    /// perfectly happy to describe two faults that never met.
    #[test]
    fn a_kill_after_the_heal_is_two_faults_in_sequence() {
        let primary = signal("network-partition", t0());
        let secondary = signal("pod-kill", t0() + TimeDelta::seconds(90));
        let report = ComposedFaultReport::build(
            &primary,
            Some(t0() + TimeDelta::seconds(60)),
            Some(&secondary),
            FLOOR,
        );

        assert_eq!(
            violations(&report),
            vec![ComposedViolation::SecondaryOutsidePrimary]
        );
    }

    /// Nothing here can be judged against a floor, so the ordering finding is
    /// raised alone rather than alongside a meaningless overlap.
    #[test]
    fn a_kill_before_the_outage_is_reported_once_not_twice() {
        let primary = signal("network-partition", t0() + TimeDelta::seconds(30));
        let secondary = signal("pod-kill", t0());
        let report = ComposedFaultReport::build(
            &primary,
            Some(t0() + TimeDelta::seconds(90)),
            Some(&secondary),
            FLOOR,
        );

        assert_eq!(
            violations(&report),
            vec![ComposedViolation::SecondaryOutsidePrimary]
        );
        assert_eq!(report.secondary_offset_secs, Some(-30.0));
    }

    /// Inside the window, but with seconds to spare rather than a window to
    /// measure anything in.
    #[test]
    fn a_kill_at_the_very_end_of_the_window_is_too_short() {
        let primary = signal("network-partition", t0());
        let secondary = signal("pod-kill", t0() + TimeDelta::seconds(55));
        let report = ComposedFaultReport::build(
            &primary,
            Some(t0() + TimeDelta::seconds(60)),
            Some(&secondary),
            FLOOR,
        );

        assert_eq!(
            violations(&report),
            vec![ComposedViolation::OverlapTooShort]
        );
        assert_eq!(report.overlap_secs, Some(5.0));
    }

    /// A missing second fault is a finding about the run, and the report says
    /// so rather than leaving a reader to notice the absent field.
    #[test]
    fn a_composition_that_never_happened_is_a_finding() {
        let primary = signal("network-partition", t0());
        let report =
            ComposedFaultReport::build(&primary, Some(t0() + TimeDelta::seconds(60)), None, FLOOR);

        assert_eq!(
            violations(&report),
            vec![ComposedViolation::SecondaryNeverInjected]
        );
        assert!(report.secondary.is_none());
        assert!(report.note_lines().is_empty());
    }

    /// An abort before the heal leaves the overlap unknowable. Reporting it as
    /// zero would raise `overlap-too-short` against a run that may well have
    /// been composed correctly.
    #[test]
    fn an_unhealed_run_reports_no_overlap_rather_than_a_short_one() {
        let primary = signal("network-partition", t0());
        let secondary = signal("pod-kill", t0() + TimeDelta::seconds(30));
        let report = ComposedFaultReport::build(&primary, None, Some(&secondary), FLOOR);

        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(report.overlap_secs, None);
        assert_eq!(report.overlap_percent, None);
        assert_eq!(report.secondary_offset_secs, Some(30.0));
    }

    /// The composition's shape is context on every run, including clean ones:
    /// every figure below it was measured under two faults at once.
    #[test]
    fn a_clean_composition_still_explains_itself() {
        let primary = signal("network-partition", t0());
        let secondary = signal("pod-kill", t0() + TimeDelta::seconds(30));
        let report = ComposedFaultReport::build(
            &primary,
            Some(t0() + TimeDelta::seconds(60)),
            Some(&secondary),
            FLOOR,
        );

        assert!(report.attention_lines().is_empty());
        assert_eq!(report.note_lines().len(), 2);
    }
}
