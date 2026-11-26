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

//! The oplog-shape invariants that `golem-worker-executor-tests/tests/AGENTS.md` requires of a
//! settled history, checked in one place.
//!
//! [`OplogShape::assert_settled`] checks:
//!
//! 1. Every invocation ID has exactly one `AgentInvocationStarted` and exactly one terminal
//!    (`AgentInvocationFinished` or `AgentInvocationFailed`), and no terminal or
//!    `AgentInvocationSettled` names an invocation that never started.
//! 2. Every `Start` has exactly one terminal (`End` or `Cancelled`).
//! 3. Within the segment of each invocation (from its `AgentInvocationStarted` to the next one),
//!    no `Start`, `End` or `Cancelled` comes after its `AgentInvocationSettled`, or after its terminal when
//!    the invocation has no `AgentInvocationSettled`. Entries between the terminal and the
//!    `AgentInvocationSettled` are background work and are allowed.
//!
//! Each documented exception must be named with the `Start` or invocation it applies to. A named
//! exception that does not occur fails the check too, so a stale exception cannot hide a
//! regression.

use crate::oplog_view::{OplogView, started_invocation_id};
use golem_common::model::InvocationId;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use std::collections::{HashMap, HashSet};

type EntryPredicate = Box<dyn Fn(&PublicOplogEntry) -> bool>;

/// A check of the settled-history invariants over a public oplog.
pub struct OplogShape<'a> {
    oplog: &'a [PublicOplogEntryWithIndex],
    crashed_during: Vec<OplogIndex>,
    left_incomplete_on_drop: Vec<OplogIndex>,
    settled_by_later_invocation: Vec<OplogIndex>,
    in_flight: Vec<InvocationId>,
    no_background_work: bool,
    forbidden: Vec<(String, EntryPredicate)>,
    expected_invocations: Option<Vec<InvocationId>>,
}

/// The positions in the oplog slice of one invocation's entries.
struct Segment {
    invocation_id: Option<InvocationId>,
    started: usize,
    end: usize,
    terminal: Option<usize>,
    settled: Option<usize>,
}

impl<'a> OplogShape<'a> {
    pub fn of(oplog: &'a [PublicOplogEntryWithIndex]) -> Self {
        Self {
            oplog,
            crashed_during: Vec::new(),
            left_incomplete_on_drop: Vec::new(),
            settled_by_later_invocation: Vec::new(),
            in_flight: Vec::new(),
            no_background_work: false,
            forbidden: Vec::new(),
            expected_invocations: None,
        }
    }

    /// The `Start` at `start` is incomplete because the test injected a crash during the call.
    pub fn crashed_during(mut self, start: OplogIndex) -> Self {
        self.crashed_during.push(start);
        self
    }

    /// The `Start` at `start` is a `LeaveIncompleteOnDrop` call that was in flight when its
    /// entity body was cancelled.
    pub fn left_incomplete_on_drop(mut self, start: OplogIndex) -> Self {
        self.left_incomplete_on_drop.push(start);
        self
    }

    /// Only the terminal of the call at `start` is written after its own invocation ended: a
    /// retained call or deferred destructor that a later invocation drives, or a task parked at
    /// a safe park point.
    pub fn settled_by_later_invocation(mut self, start: OplogIndex) -> Self {
        self.settled_by_later_invocation.push(start);
        self
    }

    /// The invocation has not settled yet: it may lack its terminal, and the positional rule
    /// does not apply to it.
    pub fn in_flight(mut self, invocation_id: &InvocationId) -> Self {
        self.in_flight.push(*invocation_id);
        self
    }

    /// No `Start`, `End` or `Cancelled` comes after the terminal of its invocation, even before an
    /// `AgentInvocationSettled`.
    pub fn no_background_work(mut self) -> Self {
        self.no_background_work = true;
        self
    }

    /// No entry of the oplog matches `predicate`; `what` names the entries in the failure.
    pub fn forbid(
        mut self,
        what: impl Into<String>,
        predicate: impl Fn(&PublicOplogEntry) -> bool + 'static,
    ) -> Self {
        self.forbidden.push((what.into(), Box::new(predicate)));
        self
    }

    /// The oplog starts exactly these invocations, each once, in any order.
    pub fn invocations(mut self, invocation_ids: &[InvocationId]) -> Self {
        self.expected_invocations = Some(invocation_ids.to_vec());
        self
    }

    /// Panics with every broken invariant and a rendering of the oplog.
    #[track_caller]
    pub fn assert_settled(self) {
        if let Err(error) = self.ensure_settled() {
            panic!("{error}");
        }
    }

    /// Fails with every broken invariant and a rendering of the oplog. For a test that must run
    /// its own cleanup after a failure instead of unwinding past it.
    pub fn ensure_settled(self) -> anyhow::Result<()> {
        let violations = self.violations();
        anyhow::ensure!(
            violations.is_empty(),
            "the oplog is not a settled history:\n  - {}\noplog: {}",
            violations.join("\n  - "),
            self.oplog.describe()
        );
        Ok(())
    }

    fn violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        let segments = self.segments();
        if segments.is_empty() {
            violations.push("the oplog has no AgentInvocationStarted".to_string());
        }
        self.check_invocations(&segments, &mut violations);
        self.check_calls(&segments, &mut violations);
        self.check_positions(&segments, &mut violations);
        for (what, predicate) in &self.forbidden {
            for entry in self.oplog {
                if predicate(&entry.entry) {
                    violations.push(format!(
                        "forbidden {what} at {}: {}",
                        entry.oplog_index,
                        entry.entry.entry_type()
                    ));
                }
            }
        }
        violations
    }

    fn segments(&self) -> Vec<Segment> {
        let starts: Vec<usize> = self
            .oplog
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry.entry, PublicOplogEntry::AgentInvocationStarted(_)))
            .map(|(position, _)| position)
            .collect();
        starts
            .iter()
            .enumerate()
            .map(|(i, &started)| {
                let end = starts.get(i + 1).copied().unwrap_or(self.oplog.len());
                let PublicOplogEntry::AgentInvocationStarted(params) = &self.oplog[started].entry
                else {
                    unreachable!()
                };
                let invocation_id = started_invocation_id(&params.invocation).cloned();
                let names = |id: &InvocationId| invocation_id.as_ref().is_none_or(|own| own == id);
                let find = |is_match: &dyn Fn(&PublicOplogEntry) -> bool| {
                    (started + 1..end).find(|&position| is_match(&self.oplog[position].entry))
                };
                let terminal = find(&|entry| match entry {
                    PublicOplogEntry::AgentInvocationFinished(params) => {
                        names(&params.invocation_id)
                    }
                    PublicOplogEntry::AgentInvocationFailed(params) => names(&params.invocation_id),
                    _ => false,
                });
                let settled = find(&|entry| {
                    matches!(entry, PublicOplogEntry::AgentInvocationSettled(params)
                        if names(&params.invocation_id))
                });
                Segment {
                    invocation_id,
                    started,
                    end,
                    terminal,
                    settled,
                }
            })
            .collect()
    }

    fn check_invocations(&self, segments: &[Segment], violations: &mut Vec<String>) {
        let mut started: HashMap<&InvocationId, usize> = HashMap::new();
        for segment in segments {
            if let Some(id) = &segment.invocation_id {
                *started.entry(id).or_default() += 1;
            }
        }
        for (id, count) in &started {
            if *count != 1 {
                violations.push(format!("invocation {id} is started {count} times"));
            }
            let record = self.oplog.invocation(id);
            let in_flight = self.in_flight.contains(id);
            match record.terminals.len() {
                1 => {}
                0 if in_flight => {}
                0 => violations.push(format!("invocation {id} has no terminal")),
                n => violations.push(format!("invocation {id} has {n} terminals")),
            }
            if record.settled.len() > 1 {
                violations.push(format!(
                    "invocation {id} is settled {} times",
                    record.settled.len()
                ));
            }
        }
        for entry in self.oplog {
            let named = match &entry.entry {
                PublicOplogEntry::AgentInvocationFinished(params) => Some(&params.invocation_id),
                PublicOplogEntry::AgentInvocationFailed(params) => Some(&params.invocation_id),
                PublicOplogEntry::AgentInvocationSettled(params) => Some(&params.invocation_id),
                _ => None,
            };
            if let Some(id) = named
                && !started.contains_key(id)
                && !segments.iter().any(|segment| {
                    segment.invocation_id.is_none()
                        && (segment.terminal == Some(self.position(entry))
                            || segment.settled == Some(self.position(entry)))
                })
            {
                violations.push(format!(
                    "{} at {} names invocation {id}, which never started",
                    entry.entry.entry_type(),
                    entry.oplog_index
                ));
            }
        }
        for id in &self.in_flight {
            if !started.contains_key(id) {
                violations.push(format!("in-flight invocation {id} never started"));
            } else if !self.oplog.invocation(id).settled.is_empty() {
                violations.push(format!(
                    "invocation {id} is named in flight, but it is settled"
                ));
            }
        }
        if let Some(expected) = &self.expected_invocations {
            let expected_set: HashSet<_> = expected.iter().collect();
            if expected_set.len() != expected.len() {
                violations.push(format!(
                    "the expected invocations repeat a key: {expected:?}"
                ));
            }
            let actual: HashSet<_> = started.keys().copied().collect();
            if actual != expected_set {
                violations.push(format!(
                    "the oplog starts {actual:?}, expected exactly {expected_set:?}"
                ));
            }
        }
    }

    fn check_calls(&self, segments: &[Segment], violations: &mut Vec<String>) {
        let named_incomplete = |start: OplogIndex| {
            self.crashed_during.contains(&start) || self.left_incomplete_on_drop.contains(&start)
        };
        for entry in self.oplog {
            let PublicOplogEntry::Start(params) = &entry.entry else {
                continue;
            };
            let start = entry.oplog_index;
            let terminals = self.oplog.call(start).terminal_count();
            match terminals {
                1 if named_incomplete(start) => violations.push(format!(
                    "the Start at {start} ({}) is named incomplete, but it has a terminal",
                    params.function_name
                )),
                1 => {}
                0 if named_incomplete(start) => {}
                0 => violations.push(format!(
                    "the Start at {start} ({}) has no terminal",
                    params.function_name
                )),
                n => violations.push(format!(
                    "the Start at {start} ({}) has {n} terminals",
                    params.function_name
                )),
            }
        }
        for (what, starts) in [
            ("crashed during", &self.crashed_during),
            ("left incomplete on drop", &self.left_incomplete_on_drop),
            (
                "settled by a later invocation",
                &self.settled_by_later_invocation,
            ),
        ] {
            for start in starts {
                if self.oplog.call(*start).start.is_none() {
                    violations.push(format!("the call {what} at {start} has no Start"));
                }
            }
        }
        for &start in &self.settled_by_later_invocation {
            let call = self.oplog.call(start);
            let terminals = [call.ends.as_slice(), call.cancellations.as_slice()].concat();
            let (Some(start_entry), [terminal]) = (call.start, terminals.as_slice()) else {
                violations.push(format!(
                    "the call settled by a later invocation at {start} has {} terminals",
                    call.terminal_count()
                ));
                continue;
            };
            let own = self.segment_at(segments, self.position(start_entry));
            let terminal_position = self.position(terminal);
            let later = match own {
                None => self.segment_at(segments, terminal_position).is_some(),
                Some(own) => {
                    terminal_position >= segments[own].end
                        || self
                            .boundary(&segments[own])
                            .is_some_and(|boundary| terminal_position > boundary)
                }
            };
            if !later {
                violations.push(format!(
                    "the call at {start} is named settled by a later invocation, but its \
                     terminal at {} is inside its own invocation",
                    terminal.oplog_index
                ));
            }
        }
    }

    fn check_positions(&self, segments: &[Segment], violations: &mut Vec<String>) {
        self.check_attributed_positions(segments, violations);
        for segment in segments {
            let in_flight = segment
                .invocation_id
                .as_ref()
                .is_some_and(|id| self.in_flight.contains(id));
            if segment.terminal.is_none() && !in_flight {
                violations.push(format!(
                    "the AgentInvocationStarted at {} has no terminal before the next \
                     AgentInvocationStarted",
                    self.oplog[segment.started].oplog_index
                ));
            }
            let Some(boundary) = self.boundary(segment) else {
                continue;
            };
            for entry in &self.oplog[boundary + 1..segment.end] {
                let kind = match &entry.entry {
                    PublicOplogEntry::Start(params) => format!("Start({})", params.function_name),
                    PublicOplogEntry::End(params) => {
                        if self
                            .settled_by_later_invocation
                            .contains(&params.start_index)
                        {
                            continue;
                        }
                        format!("End(@{})", params.start_index)
                    }
                    PublicOplogEntry::Cancelled(params) => {
                        if self
                            .settled_by_later_invocation
                            .contains(&params.start_index)
                        {
                            continue;
                        }
                        format!("Cancelled(@{})", params.start_index)
                    }
                    _ => continue,
                };
                violations.push(format!(
                    "{kind} at {} comes after the {} of invocation {} at {}",
                    entry.oplog_index,
                    self.oplog[boundary].entry.entry_type(),
                    segment
                        .invocation_id
                        .as_ref()
                        .map_or("without an ID".to_string(), ToString::to_string),
                    self.oplog[boundary].oplog_index
                ));
            }
        }
    }

    /// Applies the positional rule to each `Start` by the invocation that it belongs to, not only
    /// by the segment that it lies in: a `Start` names its invocation through `driver`, its
    /// numbered `invocation_id`, or its `parent_start_index`. A call of an earlier invocation that
    /// starts or ends after that invocation's boundary breaks the rule even when a later
    /// `AgentInvocationStarted` comes first.
    fn check_attributed_positions(&self, segments: &[Segment], violations: &mut Vec<String>) {
        let mut roots: HashMap<OplogIndex, OplogIndex> = HashMap::new();
        let mut boundaries: HashMap<OplogIndex, usize> = HashMap::new();
        for segment in segments {
            let root = self.oplog[segment.started].oplog_index;
            roots.insert(root, root);
            if let Some(boundary) = self.boundary(segment) {
                boundaries.insert(root, boundary);
            }
        }
        for (position, entry) in self.oplog.iter().enumerate() {
            let PublicOplogEntry::Start(params) = &entry.entry else {
                continue;
            };
            let start = entry.oplog_index;
            let Some(root) = params
                .driver
                .or_else(|| {
                    params
                        .invocation_id
                        .and_then(|id| roots.get(&id.anchor).copied())
                })
                .or_else(|| {
                    params
                        .parent_start_index
                        .and_then(|parent| roots.get(&parent).copied())
                })
            else {
                continue;
            };
            roots.insert(start, root);
            let Some(&boundary) = boundaries.get(&root) else {
                continue;
            };
            // The segment check already reports an entry that lies after the boundary inside the
            // invocation's own segment.
            let root_segment = self.segment_at(segments, self.position_of_index(root));
            let outside_root_segment =
                |position: usize| self.segment_at(segments, position) != root_segment;
            if position > boundary && outside_root_segment(position) {
                violations.push(format!(
                    "Start({}) at {start} of the invocation at {root} comes after its {} at {}",
                    params.function_name,
                    self.oplog[boundary].entry.entry_type(),
                    self.oplog[boundary].oplog_index
                ));
            }
            if self.settled_by_later_invocation.contains(&start) {
                continue;
            }
            let call = self.oplog.call(start);
            for terminal in call.ends.iter().chain(call.cancellations.iter()) {
                let terminal_position = self.position(terminal);
                if terminal_position > boundary && outside_root_segment(terminal_position) {
                    violations.push(format!(
                        "the terminal at {} of Start({}) at {start} of the invocation at {root} \
                         comes after its {} at {}",
                        terminal.oplog_index,
                        params.function_name,
                        self.oplog[boundary].entry.entry_type(),
                        self.oplog[boundary].oplog_index
                    ));
                }
            }
        }
    }

    fn position_of_index(&self, index: OplogIndex) -> usize {
        self.oplog
            .iter()
            .position(|entry| entry.oplog_index == index)
            .expect("the index belongs to the oplog")
    }

    /// The position after which no `Start` or `End` of the segment may come, or `None` when the
    /// positional rule does not apply.
    fn boundary(&self, segment: &Segment) -> Option<usize> {
        let in_flight = segment
            .invocation_id
            .as_ref()
            .is_some_and(|id| self.in_flight.contains(id));
        if in_flight {
            None
        } else if self.no_background_work {
            segment.terminal
        } else {
            segment.settled.or(segment.terminal)
        }
    }

    fn segment_at(&self, segments: &[Segment], position: usize) -> Option<usize> {
        segments
            .iter()
            .position(|segment| segment.started <= position && position < segment.end)
    }

    fn position(&self, entry: &PublicOplogEntryWithIndex) -> usize {
        self.oplog
            .iter()
            .position(|candidate| std::ptr::eq(candidate, entry))
            .expect("the entry belongs to the oplog")
    }
}

#[cfg(test)]
mod tests {
    use super::OplogShape;
    use golem_common::model::caller::{Caller, ClaimVisibility, PublicCaller};
    use golem_common::model::card::{PublicInvocationWalletPin, WalletVersionToken};
    use golem_common::model::oplog::public_oplog_entry::{
        AgentInvocationFailedParams, AgentInvocationSettledParams, AgentInvocationStartedParams,
        EndParams, SaveSnapshotParameters, StartParams,
    };
    use golem_common::model::oplog::{
        DurableFunctionType, OplogIndex, PublicAgentInvocation, PublicOplogEntry,
        PublicOplogEntryAttribution, PublicOplogEntryWithIndex,
    };
    use golem_common::model::{InvocationId, Timestamp};
    use test_r::test;

    fn snapshot_started() -> PublicOplogEntry {
        PublicOplogEntry::AgentInvocationStarted(AgentInvocationStartedParams {
            timestamp: Timestamp::now_utc(),
            invocation: PublicAgentInvocation::SaveSnapshot(SaveSnapshotParameters {
                caller: PublicCaller::new(&Caller::anonymous(), ClaimVisibility::Hidden),
            }),
            wallet_pin: PublicInvocationWalletPin {
                wallet_token: WalletVersionToken {
                    wallet_id_hash: [0; 32],
                    generation: 0,
                },
                scope_card_id: None,
            },
        })
    }

    fn scope_start(parent: u64) -> PublicOplogEntry {
        PublicOplogEntry::Start(StartParams {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(OplogIndex::from_u64(parent)),
            function_name: "scope".into(),
            invocation_id: None,
            observational_owner: None,
            driver: None,
            request: None,
            durable_function_type: DurableFunctionType::WriteLocal.into(),
            span_started: None,
            denial: None,
        })
    }

    fn failed(invocation_id: InvocationId, start: u64) -> PublicOplogEntry {
        PublicOplogEntry::AgentInvocationFailed(AgentInvocationFailedParams {
            timestamp: Timestamp::now_utc(),
            invocation_id,
            error: "snapshot failed".into(),
            start_index: OplogIndex::from_u64(start),
        })
    }

    fn settled(invocation_id: InvocationId) -> PublicOplogEntry {
        PublicOplogEntry::AgentInvocationSettled(AgentInvocationSettledParams {
            timestamp: Timestamp::now_utc(),
            invocation_id,
            consumed_fuel: 0,
        })
    }

    fn end(start: u64) -> PublicOplogEntry {
        PublicOplogEntry::End(EndParams {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(start),
            response: None,
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        })
    }

    fn indexed(entries: Vec<PublicOplogEntry>) -> Vec<PublicOplogEntryWithIndex> {
        entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| PublicOplogEntryWithIndex {
                oplog_index: OplogIndex::from_u64(index as u64 + 1),
                attribution: PublicOplogEntryAttribution::agent(),
                entry,
            })
            .collect()
    }

    #[test]
    fn a_scope_cannot_end_after_its_invocation_settled() {
        let invocation_id = InvocationId::new();
        let oplog = indexed(vec![
            snapshot_started(),
            scope_start(1),
            failed(invocation_id, 1),
            settled(invocation_id),
            end(2),
        ]);
        let error = OplogShape::of(&oplog).ensure_settled().unwrap_err();
        assert!(
            error.to_string().contains("End(@2) at 5 comes after"),
            "{error}"
        );
    }

    #[test]
    fn a_scope_cannot_end_inside_a_later_invocation() {
        let first = InvocationId::new();
        let second = InvocationId::new();
        let oplog = indexed(vec![
            snapshot_started(),
            scope_start(1),
            failed(first, 1),
            settled(first),
            snapshot_started(),
            end(2),
            failed(second, 5),
            settled(second),
        ]);
        let error = OplogShape::of(&oplog).ensure_settled().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("the terminal at 6 of Start(scope) at 2 of the invocation at 1"),
            "{error}"
        );
    }
}
