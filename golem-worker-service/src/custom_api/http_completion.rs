// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use std::fmt;

use bytes::Bytes;

use super::http_envelope::{ResponseBodyPolicy, ResponseHead};

/// Completion gate between a response producer and the public HTTP body.
///
/// A fixed-length response retains its final byte until both independent
/// producers (the response body and the invocation session) have completed.
/// An unbounded response can stream immediately, but cannot publish EOF until
/// both producers complete.
pub(super) struct BodyGate {
    mode: Mode,
    seen: u64,
    held: Option<u8>,
    body_eof: bool,
    session_succeeded: bool,
    terminal: bool,
    aborted: bool,
}

#[derive(Clone, Copy)]
enum Mode {
    Bodyless,
    Unknown,
    Fixed(u64),
}

#[derive(Default, Eq, PartialEq)]
pub(super) struct GateOutput {
    pub bytes: Option<Bytes>,
    pub terminal: Option<GateTerminal>,
}

impl fmt::Debug for GateOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GateOutput")
            .field("bytes_len", &self.bytes.as_ref().map(Bytes::len))
            .field("terminal", &self.terminal)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GateTerminal {
    Complete,
    Abort(CompletionError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CompletionError {
    ExcessBody,
    PrematureEof,
    SessionFailure,
    ProducerFailure,
    InvalidTransition,
}

impl BodyGate {
    pub(super) fn new(head: &ResponseHead) -> Self {
        let mode = match head.body_policy {
            ResponseBodyPolicy::Bodyless => Mode::Bodyless,
            ResponseBodyPolicy::Stream => match head.content_length {
                Some(length) => Mode::Fixed(length),
                None => Mode::Unknown,
            },
        };
        Self {
            mode,
            seen: 0,
            held: None,
            // The caller must still dispose of the body producer. Bodyless
            // completion means that disposal, rather than polling, acknowledges it.
            body_eof: matches!(mode, Mode::Bodyless),
            session_succeeded: false,
            terminal: false,
            aborted: false,
        }
    }

    /// Bodyless heads are withheld until this becomes true. Streaming heads
    /// may be committed immediately.
    #[cfg(test)]
    pub(super) fn may_commit_head(&self) -> bool {
        !self.aborted && (!matches!(self.mode, Mode::Bodyless) || self.session_succeeded)
    }

    pub(super) fn push(&mut self, bytes: Bytes) -> GateOutput {
        if self.terminal || self.body_eof || matches!(self.mode, Mode::Bodyless) {
            return self.abort(CompletionError::InvalidTransition);
        }
        let amount = match u64::try_from(bytes.len()) {
            Ok(amount) => amount,
            Err(_) => return self.abort(CompletionError::ExcessBody),
        };
        let next = match self.seen.checked_add(amount) {
            Some(next) => next,
            None => return self.abort(CompletionError::ExcessBody),
        };
        if let Mode::Fixed(limit) = self.mode {
            if next > limit {
                return self.abort(CompletionError::ExcessBody);
            }
            self.seen = next;
            let bytes = if next == limit && !bytes.is_empty() {
                self.held = bytes.last().copied();
                (bytes.len() > 1).then(|| bytes.slice(..bytes.len() - 1))
            } else {
                (!bytes.is_empty()).then_some(bytes)
            };
            GateOutput {
                bytes,
                terminal: None,
            }
        } else {
            self.seen = next;
            GateOutput {
                bytes: (!bytes.is_empty()).then_some(bytes),
                terminal: None,
            }
        }
    }

    pub(super) fn body_eof(&mut self) -> GateOutput {
        if self.terminal || self.body_eof {
            return self.abort(CompletionError::InvalidTransition);
        }
        if let Mode::Fixed(limit) = self.mode
            && self.seen != limit
        {
            return self.abort(CompletionError::PrematureEof);
        }
        self.body_eof = true;
        self.complete_if_ready()
    }

    pub(super) fn session_success(&mut self) -> GateOutput {
        if self.terminal || self.session_succeeded {
            return self.abort(CompletionError::InvalidTransition);
        }
        self.session_succeeded = true;
        self.complete_if_ready()
    }

    pub(super) fn session_failure(&mut self) -> GateOutput {
        self.abort(CompletionError::SessionFailure)
    }

    pub(super) fn producer_failure(&mut self) -> GateOutput {
        self.abort(CompletionError::ProducerFailure)
    }

    fn complete_if_ready(&mut self) -> GateOutput {
        if self.body_eof && self.session_succeeded {
            self.terminal = true;
            GateOutput {
                bytes: self.held.take().map(|byte| Bytes::copy_from_slice(&[byte])),
                terminal: Some(GateTerminal::Complete),
            }
        } else {
            GateOutput::default()
        }
    }

    fn abort(&mut self, error: CompletionError) -> GateOutput {
        self.held = None;
        self.terminal = true;
        self.aborted = true;
        GateOutput {
            bytes: None,
            terminal: Some(GateTerminal::Abort(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderMap, StatusCode};
    use serde_json::Value;
    use test_r::test;

    fn head(length: Option<u64>, policy: ResponseBodyPolicy) -> ResponseHead {
        ResponseHead {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            content_length: length,
            body_policy: policy,
        }
    }

    fn corpus_case(id: &str) -> Value {
        let corpus: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        )))
        .unwrap();
        corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap_or_else(|| panic!("missing shared vector {id}"))
            .clone()
    }

    fn decode_hex(hex: &str) -> Bytes {
        Bytes::from(
            hex.as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect::<Vec<_>>(),
        )
    }

    struct Observed {
        body: Vec<u8>,
        terminal: Option<GateTerminal>,
        committed_heads: u64,
        body_polls: u64,
        may_commit_head: bool,
    }

    fn run_lifecycle(id: &str) -> (Value, Observed) {
        let case = corpus_case(id);
        let events = case["input"]["events"].as_array().unwrap();
        let method_is_head = case["input"]["method"] == "HEAD";
        let head_event = events
            .iter()
            .filter_map(Value::as_str)
            .find(|event| event.starts_with("response-head:"))
            .unwrap();
        let length = head_event
            .split("content-length=")
            .nth(1)
            .map(|value| value.parse().unwrap());
        let policy = if method_is_head {
            ResponseBodyPolicy::Bodyless
        } else {
            ResponseBodyPolicy::Stream
        };
        let mut gate = BodyGate::new(&head(length, policy));
        let mut observed = Observed {
            body: Vec::new(),
            terminal: None,
            committed_heads: 0,
            body_polls: 0,
            may_commit_head: false,
        };
        for event in events.iter().filter_map(Value::as_str) {
            let output = if let Some(hex) = event.strip_prefix("response-chunk:") {
                observed.body_polls += 1;
                gate.push(decode_hex(hex))
            } else {
                match event {
                    "response-eof" => {
                        observed.body_polls += 1;
                        gate.body_eof()
                    }
                    "session-success" => gate.session_success(),
                    "session-failure" => gate.session_failure(),
                    "executor-loss" => gate.producer_failure(),
                    "commit-head" => {
                        observed.committed_heads += u64::from(gate.may_commit_head());
                        GateOutput::default()
                    }
                    _ => GateOutput::default(),
                }
            };
            if let Some(bytes) = output.bytes {
                observed.body.extend_from_slice(&bytes);
            }
            if output.terminal.is_some() {
                observed.terminal = output.terminal;
            }
        }
        observed.may_commit_head = gate.may_commit_head();
        (case, observed)
    }

    fn assert_gate_expectations(case: &Value, observed: &Observed) {
        let expect = &case["expect"];
        if let Some(hex) = expect.get("body_hex").and_then(Value::as_str) {
            assert_eq!(observed.body, decode_hex(hex));
        }
        if let Some(maximum) = expect
            .get("maximum_public_body_bytes")
            .and_then(Value::as_u64)
        {
            assert!(observed.body.len() as u64 <= maximum);
        }
        if let Some(expected) = expect.get("public_abort").and_then(Value::as_bool) {
            assert_eq!(
                matches!(observed.terminal, Some(GateTerminal::Abort(_))),
                expected
            );
        }
        if let Some(expected) = expect.get("successful_eof").and_then(Value::as_bool) {
            assert_eq!(observed.terminal == Some(GateTerminal::Complete), expected);
        }
        if let Some(expected) = expect.get("committed_heads").and_then(Value::as_u64) {
            assert_eq!(observed.committed_heads, expected);
        }
        if let Some(expected) = expect.get("body_polls").and_then(Value::as_u64) {
            assert_eq!(observed.body_polls, expected);
        }
    }

    #[test]
    fn shared_lifecycle_vectors_drive_the_gate() {
        for id in [
            "lifecycle-body-eof-session-fails",
            "lifecycle-fixed-length-session-fails",
            "lifecycle-fixed-length-session-succeeds",
            "lifecycle-short-promised-response",
            "lifecycle-overlong-response",
            "lifecycle-executor-loss-after-head",
        ] {
            let (case, observed) = run_lifecycle(id);
            assert_gate_expectations(&case, &observed);
            if matches!(observed.terminal, Some(GateTerminal::Abort(_))) {
                assert!(!observed.may_commit_head, "{id} allowed commit after abort");
            }
        }
    }

    #[test]
    fn shared_bodyless_disposal_vectors_drive_the_gate() {
        for id in [
            "lifecycle-head-disposal-session-fails",
            "lifecycle-head-disposal-session-succeeds",
        ] {
            let (case, observed) = run_lifecycle(id);
            assert_gate_expectations(&case, &observed);
            let expected_commits = case["expect"]["committed_application_heads"]
                .as_u64()
                .unwrap();
            assert_eq!(u64::from(observed.may_commit_head), expected_commits);
        }
    }

    #[test]
    fn fixed_length_only_retains_the_promised_final_byte() {
        let mut gate = BodyGate::new(&head(Some(4), ResponseBodyPolicy::Stream));
        assert_eq!(
            gate.push(Bytes::from_static(b"ab")).bytes.unwrap(),
            b"ab"[..]
        );
        assert_eq!(
            gate.push(Bytes::from_static(b"cd")).bytes.unwrap(),
            b"c"[..]
        );
        assert_eq!(gate.body_eof(), GateOutput::default());
        let done = gate.session_success();
        assert_eq!(done.bytes.unwrap(), b"d"[..]);
        assert_eq!(done.terminal, Some(GateTerminal::Complete));
    }

    #[test]
    fn output_debug_is_opaque() {
        let output = GateOutput {
            bytes: Some(Bytes::from_static(b"secret-response")),
            terminal: None,
        };
        assert_eq!(
            format!("{output:?}"),
            "GateOutput { bytes_len: Some(15), terminal: None }"
        );
    }
}
