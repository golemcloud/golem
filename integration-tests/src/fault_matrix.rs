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

//! The streaming fault-matrix harness: the fixture contract, the assertions for each path
//! guarantee, and the reports that each fault injector gives.
//!
//! # Fixture contract
//!
//! A fixture is a test component, built through one SDK, that defines the agent type
//! [`FIXTURE_AGENT_TYPE`] with a constructor that takes one `string` name. A row selects a
//! fixture only through a [`Fixture`] value, so a new SDK adds its fixture to [`FIXTURES`]
//! without a change to the harness or to the rows.
//!
//! Record `seq` of run `run` is the `u32` value `run * 100_000 + seq`, for `seq` in
//! `1..=count`. Every row uses a fresh run number, so a record from another run (a splice) is
//! visible in the record itself. The methods and their documented results:
//!
//! | Method | Operation | Result |
//! | -- | -- | -- |
//! | `create_gate() -> promise-id` | | A promise that a gated producer awaits |
//! | `produce(run, count, pause_after, gate) -> stream<u32>` | write | Records 1..=count. Before record `pause_after + 1` the producer awaits `gate`; `pause_after >= count` never waits |
//! | `produce_then_fail(run, fail_after) -> stream<u32>` | error | A prefix of records 1..=fail_after, then an explicit failure of the stream or of the invocation |
//! | `consume(input: stream<u32>) -> list<u32>` | read | Every input record, in order |
//! | `consume_then_drop(input: stream<u32>, keep) -> list<u32>` | drop | The first `keep` records; then the consumer drops the input |
//! | `pipe(target, run, count, keep) -> list<u32>` | write, read, drop | Writes records 1..=count into `consume` (or `consume_then_drop` when `keep < count`) of the fixture agent `target`, and returns its result |
//! | `read_remote(target, run, count, pause_after, gate) -> list<u32>` | read over RPC | The records of `produce` of the fixture agent `target` |
//!
//! A gate has the schema shape of the WIT `promise-id` record, with the component id nested as a
//! `{ uuid }` record. A fixture whose SDK represents promise ids in another shape declares its own
//! record of this shape, as the Go, Scala and MoonBit fixtures do.
//!
//! A producer writes records 1..=fail_after before it fails, but a write that the transport
//! accepted is not guaranteed to reach the reader before the failure, and an SDK may fail the
//! whole invocation (the TypeScript SDK traps on a failed stream source). The error operation
//! therefore guarantees only a contiguous prefix and an explicit failure, never a clean end.
//!
//! Cancel is the client's operation: the client cancels an output of `produce` as its
//! consumer, and the output then ends with a cancellation, never a clean end.
//!
//! A fixture whose SDK exposes Durable Streams routes also mounts the agent at
//! `/fault-matrix/{name}` and serves `PUT /numbered/{run}/{count}?delay_ms={delay_ms}`, which
//! writes records 1..=count with `delay_ms` before each record.

use std::fmt::{Display, Formatter};

/// The agent type that every fixture defines.
pub const FIXTURE_AGENT_TYPE: &str = "FaultMatrixFixture";

/// Distance between the record numbers of two runs.
pub const RUN_STRIDE: u32 = 100_000;

pub fn record(run: u32, seq: u32) -> u32 {
    run * RUN_STRIDE + seq
}

/// One SDK's fixture: a test component that implements the fixture contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixture {
    pub sdk: &'static str,
    /// The test component file name without `.wasm`.
    pub wasm: &'static str,
    /// The component name in the fixture's application manifest.
    pub component: &'static str,
    /// The fixture serves the Durable Streams route of the contract.
    pub durable_streams: bool,
}

pub const FIXTURES: &[Fixture] = &[
    Fixture {
        sdk: "rust",
        wasm: "golem_it_fault_matrix_rust_release",
        component: "golem-it:fault-matrix-rust",
        durable_streams: true,
    },
    Fixture {
        sdk: "ts",
        wasm: "golem_it_fault_matrix_ts",
        component: "golem-it:fault-matrix-ts",
        durable_streams: false,
    },
    Fixture {
        sdk: "effect",
        wasm: "golem_it_fault_matrix_effect",
        component: "golem-it:fault-matrix-effect",
        durable_streams: false,
    },
    Fixture {
        sdk: "scala",
        wasm: "golem_it_fault_matrix_scala",
        component: "golem-it:fault-matrix-scala",
        durable_streams: false,
    },
    Fixture {
        sdk: "moonbit",
        wasm: "golem_it_fault_matrix_moonbit",
        component: "golem-it:fault-matrix-moonbit",
        durable_streams: false,
    },
    Fixture {
        sdk: "go",
        wasm: "golem_it_fault_matrix_go",
        component: "golem-it:fault-matrix-go",
        durable_streams: false,
    },
];

pub fn fixture(sdk: &str) -> Fixture {
    *FIXTURES
        .iter()
        .find(|fixture| fixture.sdk == sdk)
        .unwrap_or_else(|| panic!("no fault-matrix fixture for {sdk}"))
}

/// How a delivered stream ended, as the client observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    /// The producer ended the stream.
    End,
    /// The stream ended with an explicit failure.
    Failed(String),
    /// The stream ended with a cancellation.
    Cancelled(String),
    /// A late attach reached the end of the live suffix of an unrecorded output.
    OutputNotRecorded,
    /// The client stopped reading before any terminal.
    Open,
}

/// The records and the terminal that a client read from one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub records: Vec<u32>,
    pub terminal: Terminal,
}

impl Delivery {
    pub fn new(records: Vec<u32>, terminal: Terminal) -> Self {
        Self { records, terminal }
    }
}

/// A delivery that breaks its path guarantee. The message names the first bad position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation(pub String);

impl Display for Violation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Violation {}

/// Checks that `records` are records `first..first + records.len()` of `run`, in order.
fn check_contiguous(run: u32, first: u32, records: &[u32]) -> Result<(), Violation> {
    for (index, &actual) in records.iter().enumerate() {
        let expected = record(run, first + index as u32);
        if actual != expected {
            let detail = if actual / RUN_STRIDE != run {
                format!(
                    "record {actual} is from run {}, not run {run} (splice)",
                    actual / RUN_STRIDE
                )
            } else if actual < expected {
                format!(
                    "record {} repeats or goes back (duplicate or reorder)",
                    actual % RUN_STRIDE
                )
            } else {
                format!(
                    "record {} is missing, the next record is {} (gap or reorder)",
                    expected % RUN_STRIDE,
                    actual % RUN_STRIDE
                )
            };
            return Err(Violation(format!(
                "position {}: expected record {}, got {actual}: {detail}",
                index + 1,
                expected
            )));
        }
    }
    Ok(())
}

/// Resuming path: exactly records 1..=count of `run`, in order, then the end of the stream.
pub fn assert_resumed(run: u32, count: u32, delivery: &Delivery) -> Result<(), Violation> {
    check_contiguous(run, 1, &delivery.records)?;
    if delivery.records.len() < count as usize {
        return Err(Violation(format!(
            "position {}: expected record {}, the stream delivered only {} record(s) and then {:?}",
            delivery.records.len() + 1,
            record(run, delivery.records.len() as u32 + 1),
            delivery.records.len(),
            delivery.terminal
        )));
    }
    if delivery.records.len() > count as usize {
        return Err(Violation(format!(
            "position {}: expected the end after {count} record(s), got record {}",
            count + 1,
            delivery.records[count as usize]
        )));
    }
    if delivery.terminal != Terminal::End {
        return Err(Violation(format!(
            "position {}: expected the end of the stream, got {:?}",
            count + 1,
            delivery.terminal
        )));
    }
    Ok(())
}

/// Fail-clean path: the delivered prefix 1..=k of `run`, then an explicit failure or
/// cancellation. Returns k.
pub fn assert_fail_clean(run: u32, delivery: &Delivery) -> Result<usize, Violation> {
    check_contiguous(run, 1, &delivery.records)?;
    match &delivery.terminal {
        Terminal::Failed(_) | Terminal::Cancelled(_) => Ok(delivery.records.len()),
        other => Err(Violation(format!(
            "position {}: expected an explicit failure after the prefix, got {other:?}",
            delivery.records.len() + 1
        ))),
    }
}

/// Late attach: a contiguous live suffix k..=count of `run`, then the `output-not-recorded`
/// marker. Returns k.
pub fn assert_late_attach(run: u32, count: u32, delivery: &Delivery) -> Result<u32, Violation> {
    let Some(&first) = delivery.records.first() else {
        return match delivery.terminal {
            Terminal::OutputNotRecorded => Ok(count + 1),
            ref other => Err(Violation(format!(
                "position 1: expected a record or the output-not-recorded marker, got {other:?}"
            ))),
        };
    };
    if first / RUN_STRIDE != run || first % RUN_STRIDE == 0 || first % RUN_STRIDE > count {
        return Err(Violation(format!(
            "position 1: record {first} is not a record 1..={count} of run {run}"
        )));
    }
    let start = first % RUN_STRIDE;
    check_contiguous(run, start, &delivery.records)?;
    let last = start + delivery.records.len() as u32 - 1;
    if last > count {
        return Err(Violation(format!(
            "position {}: expected the marker after record {count}, got another record",
            count - start + 2
        )));
    }
    if last < count {
        return Err(Violation(format!(
            "position {}: expected record {}, the suffix ended with {:?}",
            delivery.records.len() + 1,
            record(run, last + 1),
            delivery.terminal
        )));
    }
    if delivery.terminal != Terminal::OutputNotRecorded {
        return Err(Violation(format!(
            "position {}: expected the output-not-recorded marker, got {:?}",
            delivery.records.len() + 1,
            delivery.terminal
        )));
    }
    Ok(start)
}

/// What an injector did. Each row asserts the effect, so a fault that did not happen fails the
/// row instead of letting it pass vacuously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectorEffect {
    /// The executor processes were killed and started again: their process IDs changed.
    ExecutorRestarted {
        before: Vec<Option<u32>>,
        after: Vec<Option<u32>>,
    },
    /// The test's own worker service was killed and started again on the same ports.
    WorkerServiceRestarted { before: u32, after: u32 },
    /// The relay reset this many client connections.
    ClientDropped { resets: u64 },
    /// A new component revision reached the agent while its stream was open.
    Deployed { before: u64, after: u64 },
    /// The client dropped its connection and attached again with its saved session.
    BrowserReloaded {
        resets: u64,
        epoch_before: u64,
        epoch_after: u64,
    },
}

impl InjectorEffect {
    /// Fails unless the injector had its effect.
    pub fn ensure_happened(&self) -> Result<(), Violation> {
        let happened = match self {
            Self::ExecutorRestarted { before, after } => {
                !before.is_empty()
                    && before.len() == after.len()
                    && before.iter().zip(after).all(|(before, after)| {
                        before.is_some() && after.is_some() && before != after
                    })
            }
            Self::WorkerServiceRestarted { before, after } => before != after,
            Self::ClientDropped { resets } => *resets > 0,
            Self::Deployed { before, after } => after > before,
            Self::BrowserReloaded {
                resets,
                epoch_before,
                epoch_after,
            } => *resets > 0 && epoch_after > epoch_before,
        };
        if happened {
            Ok(())
        } else {
            Err(Violation(format!("the fault did not happen: {self:?}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    const RUN: u32 = 7;

    fn records(seqs: impl IntoIterator<Item = u32>) -> Vec<u32> {
        seqs.into_iter().map(|seq| record(RUN, seq)).collect()
    }

    fn fails_at(result: Result<impl std::fmt::Debug, Violation>, position: usize) {
        let error = result.expect_err("the assertion accepted a bad delivery");
        assert!(
            error.0.starts_with(&format!("position {position}:")),
            "the message does not name position {position}: {error}"
        );
    }

    #[test]
    fn resumed_accepts_exactly_one_to_n_in_order() {
        assert_resumed(RUN, 5, &Delivery::new(records(1..=5), Terminal::End)).unwrap();
    }

    #[test]
    fn resumed_rejects_a_gap() {
        fails_at(
            assert_resumed(RUN, 5, &Delivery::new(records([1, 2, 4, 5]), Terminal::End)),
            3,
        );
    }

    #[test]
    fn resumed_rejects_a_duplicate() {
        fails_at(
            assert_resumed(
                RUN,
                5,
                &Delivery::new(records([1, 2, 3, 3, 4, 5]), Terminal::End),
            ),
            4,
        );
    }

    #[test]
    fn resumed_rejects_a_swap_even_when_count_and_last_match() {
        fails_at(
            assert_resumed(
                RUN,
                5,
                &Delivery::new(records([1, 3, 2, 4, 5]), Terminal::End),
            ),
            2,
        );
    }

    #[test]
    fn resumed_rejects_a_splice_from_another_run() {
        let mut spliced = records(1..=5);
        spliced[3] = record(RUN + 1, 4);
        let error = assert_resumed(RUN, 5, &Delivery::new(spliced, Terminal::End)).unwrap_err();
        assert!(error.0.starts_with("position 4:"), "{error}");
        assert!(error.0.contains("splice"), "{error}");
    }

    #[test]
    fn resumed_rejects_a_short_or_long_or_unterminated_delivery() {
        fails_at(
            assert_resumed(RUN, 5, &Delivery::new(records(1..=4), Terminal::End)),
            5,
        );
        fails_at(
            assert_resumed(RUN, 5, &Delivery::new(records(1..=6), Terminal::End)),
            6,
        );
        fails_at(
            assert_resumed(
                RUN,
                5,
                &Delivery::new(records(1..=5), Terminal::Failed("x".into())),
            ),
            6,
        );
        fails_at(
            assert_resumed(RUN, 5, &Delivery::new(records(1..=5), Terminal::Open)),
            6,
        );
    }

    #[test]
    fn fail_clean_accepts_a_prefix_then_an_explicit_failure() {
        assert_eq!(
            assert_fail_clean(
                RUN,
                &Delivery::new(records(1..=3), Terminal::Failed("boom".into()))
            ),
            Ok(3)
        );
        assert_eq!(
            assert_fail_clean(RUN, &Delivery::new(vec![], Terminal::Cancelled("c".into()))),
            Ok(0)
        );
    }

    #[test]
    fn fail_clean_rejects_a_clean_end_after_the_prefix() {
        fails_at(
            assert_fail_clean(RUN, &Delivery::new(records(1..=3), Terminal::End)),
            4,
        );
    }

    #[test]
    fn fail_clean_rejects_a_gap_hidden_by_the_failure() {
        fails_at(
            assert_fail_clean(
                RUN,
                &Delivery::new(records([1, 3]), Terminal::Failed("boom".into())),
            ),
            2,
        );
    }

    #[test]
    fn late_attach_accepts_a_live_suffix_then_the_marker() {
        assert_eq!(
            assert_late_attach(
                RUN,
                9,
                &Delivery::new(records(4..=9), Terminal::OutputNotRecorded)
            ),
            Ok(4)
        );
    }

    #[test]
    fn late_attach_rejects_a_suffix_without_the_marker() {
        fails_at(
            assert_late_attach(RUN, 9, &Delivery::new(records(4..=9), Terminal::End)),
            7,
        );
    }

    #[test]
    fn late_attach_rejects_a_gap_inside_the_suffix() {
        fails_at(
            assert_late_attach(
                RUN,
                9,
                &Delivery::new(records([4, 5, 7, 8, 9]), Terminal::OutputNotRecorded),
            ),
            3,
        );
    }

    #[test]
    fn late_attach_rejects_a_suffix_that_stops_before_the_last_record() {
        fails_at(
            assert_late_attach(
                RUN,
                9,
                &Delivery::new(records(4..=8), Terminal::OutputNotRecorded),
            ),
            6,
        );
    }

    #[test]
    fn injector_effects_reject_a_fault_that_did_not_happen() {
        let no_op = [
            InjectorEffect::ExecutorRestarted {
                before: vec![Some(10)],
                after: vec![Some(10)],
            },
            InjectorEffect::ExecutorRestarted {
                before: vec![],
                after: vec![],
            },
            InjectorEffect::WorkerServiceRestarted {
                before: 10,
                after: 10,
            },
            InjectorEffect::ClientDropped { resets: 0 },
            InjectorEffect::Deployed {
                before: 1,
                after: 1,
            },
            InjectorEffect::BrowserReloaded {
                resets: 1,
                epoch_before: 2,
                epoch_after: 2,
            },
        ];
        for effect in no_op {
            assert!(effect.ensure_happened().is_err(), "{effect:?} was accepted");
        }
        InjectorEffect::ExecutorRestarted {
            before: vec![Some(10), Some(11)],
            after: vec![Some(12), Some(13)],
        }
        .ensure_happened()
        .unwrap();
        InjectorEffect::ClientDropped { resets: 2 }
            .ensure_happened()
            .unwrap();
    }
}
