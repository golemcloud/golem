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

//! What a failed invocation actually tells the driver (GOL-366).
//!
//! Everything downstream — whether an operation is retried, whether it counts
//! towards the read-back bounds, whether a run fails outright — hangs off this
//! one question: *did the platform execute the work?* Four answers are
//! distinguishable from the client side, and collapsing any two of them loses
//! something the scenario exists to measure:
//!
//! | Class | The exchange | Did it execute? |
//! | -- | -- | -- |
//! | [`ErrorClass::Response`] | Answered with a definite, non-retryable status | **No** — refused before any work started |
//! | [`ErrorClass::Application`] | Answered `500` carrying a `workerError` | **It ran** — the agent itself failed |
//! | [`ErrorClass::Platform`] | Answered, but with a transient server-side status | Unknowable |
//! | [`ErrorClass::Transport`] | Never completed — reset, timeout, no status | Unknowable |
//!
//! Only [`ErrorClass::Transport`] may be retried. That is not a load-shedding
//! choice, it is what makes duplicate execution *visible*: the one permitted
//! retry goes out under the original idempotency key, so a platform that
//! executes it a second time reveals itself. Retrying a `Platform` 5xx too
//! would blur the picture without adding evidence, and retrying a `Response`
//! rejection is pointless by definition.
//!
//! ### Why this is a module rather than a predicate
//!
//! The first cut of this (in the S12 driver) was a single
//! `is_definite_rejection` that downcast the error chain to [`reqwest::Error`].
//! It never matched. The error that actually arrives from an invocation is
//! [`golem_client::Error<AgentError>`], whose `Reqwest` variant is a plain field
//! rather than a `thiserror` source, so it is not reachable by walking
//! [`anyhow::Error::chain`] — the predicate answered "not a rejection" for
//! *every* failure and every operation was recorded as in-doubt. Classifying
//! against the concrete client error type is what makes the distinction real.

use golem_client::api::{AgentError, WorkerError};
use golem_client::model::ErrorBodyWithOptionalWorkerError;
use golem_client::{Error as ClientError, ErrorInfo};

/// How much a failed attempt tells the driver about whether the platform
/// executed the work. See the module documentation for the full table.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorClass {
    /// The service answered with a definite, non-retryable status: the request
    /// arrived, was understood, and was refused. Nothing executed.
    Response,
    /// The exchange never completed — connection reset, request timeout, no
    /// status at all. The request may have arrived and executed anyway; from
    /// the client side the two are indistinguishable. The only retryable class.
    Transport,
    /// The service answered, but with a transient server-side status (`5xx`
    /// with no worker error, `408`, `429`). It saw the request; whether it got
    /// far enough to execute it is not knowable from here.
    Platform,
    /// The invocation reached the agent and the agent itself failed — a `500`
    /// carrying `workerError`. The work definitely *ran*; whether its effect
    /// committed is a property of the failure, not something the client can
    /// see.
    Application,
}

impl ErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorClass::Response => "response",
            ErrorClass::Transport => "transport",
            ErrorClass::Platform => "platform",
            ErrorClass::Application => "application",
        }
    }

    /// Whether the platform definitely refused the request without executing
    /// it. Only [`ErrorClass::Response`] qualifies: every other class leaves
    /// genuine doubt, and the read-back carries that doubt through as the width
    /// of a range rather than resolving it by guessing.
    pub fn is_definite_rejection(self) -> bool {
        matches!(self, ErrorClass::Response)
    }

    /// Whether the bounded same-key retry is allowed to fire for this class
    /// under a transport-only policy.
    pub fn is_retryable_transport_failure(self) -> bool {
        matches!(self, ErrorClass::Transport)
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classifies a failed invocation.
///
/// Unrecognised errors fall through to [`ErrorClass::Transport`], which is the
/// safe direction: it widens the band of doubt and permits one same-key retry.
/// Guessing the other way would manufacture a duplicate-execution finding out
/// of an error the driver simply could not read.
pub fn classify(error: &anyhow::Error) -> ErrorClass {
    // `downcast_ref` searches the whole chain, so the `.context(...)` layers the
    // DSL adds do not hide the client error underneath them.
    if let Some(client_error) = error.downcast_ref::<ClientError<AgentError>>() {
        return classify_client_error(client_error, |item| match item {
            AgentError::Error500(body) => Some(body),
            _ => None,
        });
    }
    if let Some(client_error) = error.downcast_ref::<ClientError<WorkerError>>() {
        return classify_client_error(client_error, |item| match item {
            WorkerError::Error500(body) => Some(body),
            _ => None,
        });
    }
    ErrorClass::Transport
}

/// The shared body of [`classify`], parameterised only by how to reach the
/// `500` payload — the one place the two generated error enums differ in a way
/// that matters here.
fn classify_client_error<T, F>(error: &ClientError<T>, worker_error: F) -> ErrorClass
where
    T: ErrorInfo,
    F: Fn(&T) -> Option<&ErrorBodyWithOptionalWorkerError>,
{
    match error {
        // Reached the service and got a modelled answer back.
        ClientError::Item(item) => {
            if worker_error(item).is_some_and(|body| body.worker_error.is_some()) {
                // A `500` that names the failing worker is the platform
                // reporting, accurately, that the agent ran and trapped.
                return ErrorClass::Application;
            }
            classify_status(item.status_code())
        }
        // An answer arrived with a status the spec does not model. Still an
        // answer, so it is graded the same way.
        ClientError::Unexpected { code, .. } => classify_status(*code),
        // The exchange itself failed.
        ClientError::Reqwest(_) | ClientError::Middleware(_) | ClientError::ReqwestHeader(_) => {
            ErrorClass::Transport
        }
        // A `2xx` body the client could not parse. The service answered, so the
        // work very likely ran — treating this as a refusal would be wrong.
        ClientError::Serde(_) => ErrorClass::Platform,
    }
}

/// Grades an HTTP status the service actually returned.
///
/// `408` and `429` sit with the `5xx`s deliberately: both are the server saying
/// "not now" rather than "no", and a request that timed out server-side may
/// well have executed.
fn classify_status(status: u16) -> ErrorClass {
    match status {
        408 | 429 => ErrorClass::Platform,
        500..=599 => ErrorClass::Platform,
        _ => ErrorClass::Response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_client::model::{ErrorBody, ErrorsBody};
    use test_r::test;

    fn agent_error(item: AgentError) -> anyhow::Error {
        anyhow::Error::new(ClientError::Item(item))
    }

    fn error_body() -> ErrorBody {
        ErrorBody {
            code: "404".to_string(),
            error: "boom".to_string(),
            cause: None,
        }
    }

    fn server_error(worker_error: Option<&str>) -> AgentError {
        AgentError::Error500(ErrorBodyWithOptionalWorkerError {
            code: "500".to_string(),
            error: "internal".to_string(),
            worker_error: worker_error.map(|cause| golem_client::model::WorkerErrorDetails {
                cause: cause.to_string(),
                stderr: String::new(),
            }),
        })
    }

    // ── The distinction the whole read-back rests on ────────────────────────

    /// The regression this module exists for: before it, an invocation failure
    /// arrived as `ClientError<AgentError>` and the old `reqwest`-based
    /// predicate could not see it, so a flat refusal was filed as "cannot tell".
    #[test]
    fn a_modelled_client_error_is_recognised_as_a_definite_refusal() {
        let error = agent_error(AgentError::Error400(ErrorsBody {
            code: "400".to_string(),
            errors: vec!["bad request".to_string()],
            cause: None,
        }));
        assert_eq!(classify(&error), ErrorClass::Response);
        assert!(classify(&error).is_definite_rejection());
    }

    /// The context layers the DSL wraps around a failure must not hide the
    /// client error underneath them.
    #[test]
    fn wrapping_context_does_not_hide_the_client_error() {
        let error = agent_error(AgentError::Error404(error_body()))
            .context("invoking sleep_and_increment")
            .context("chaos pinned stream");
        assert_eq!(classify(&error), ErrorClass::Response);
    }

    #[test]
    fn a_worker_failure_is_an_application_error_not_a_platform_one() {
        let error = agent_error(server_error(Some("unreachable executed")));
        assert_eq!(classify(&error), ErrorClass::Application);
    }

    /// A bare `500` says nothing about the agent, only about the platform.
    #[test]
    fn a_server_error_without_a_worker_error_is_a_platform_error() {
        let error = agent_error(server_error(None));
        assert_eq!(classify(&error), ErrorClass::Platform);
    }

    #[test]
    fn timeout_and_throttle_statuses_are_platform_errors_not_refusals() {
        assert_eq!(classify_status(408), ErrorClass::Platform);
        assert_eq!(classify_status(429), ErrorClass::Platform);
        assert_eq!(classify_status(503), ErrorClass::Platform);
        assert!(!ErrorClass::Platform.is_definite_rejection());
    }

    #[test]
    fn other_client_statuses_are_definite_refusals() {
        for status in [400, 401, 403, 404, 409, 413, 415, 422] {
            assert_eq!(
                classify_status(status),
                ErrorClass::Response,
                "status {status} should be a definite refusal"
            );
        }
    }

    /// An error the driver cannot read must widen the band of doubt, never
    /// narrow it — an unreadable failure treated as a refusal would invent a
    /// duplicate-execution finding out of nothing.
    #[test]
    fn an_uninterpretable_error_falls_through_to_transport() {
        let error = anyhow::anyhow!("connection reset by peer");
        assert_eq!(classify(&error), ErrorClass::Transport);
        assert!(!classify(&error).is_definite_rejection());
    }

    // ── The retry rule ──────────────────────────────────────────────────────

    /// Only transport failures retry. The retry is evidence-gathering, not
    /// load-shedding: it re-sends the original key so a second execution
    /// becomes visible.
    #[test]
    fn only_transport_failures_are_retryable() {
        assert!(ErrorClass::Transport.is_retryable_transport_failure());
        assert!(!ErrorClass::Platform.is_retryable_transport_failure());
        assert!(!ErrorClass::Response.is_retryable_transport_failure());
        assert!(!ErrorClass::Application.is_retryable_transport_failure());
    }
}
