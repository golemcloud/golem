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
mod tests;
