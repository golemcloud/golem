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

//! The typed error of an invocation that the executor rejected or failed.

use golem_api_grpc::proto::golem::worker::{
    InvocationFailure, InvocationRejected, InvocationRejectionReason,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::fmt::{Display, Formatter};

/// The error of an invocation session that the executor rejected before acceptance, or that
/// finished with a failure. The DSL returns it inside an [`anyhow::Error`]; read it with
/// [`InvocationErrorExt`] or check it with [`crate::assert_worker_error!`].
#[derive(Debug)]
pub enum InvocationError {
    Rejected {
        rejected: InvocationRejected,
        /// The worker error of the rejection, when the executor sent one that converts.
        worker_error: Option<WorkerExecutorError>,
    },
    Failed {
        failure: InvocationFailure,
        /// The worker error of the failure, when the executor sent one that converts.
        worker_error: Option<WorkerExecutorError>,
    },
}

impl InvocationError {
    pub fn rejected(rejected: InvocationRejected) -> Self {
        let worker_error = rejected
            .worker_error
            .clone()
            .and_then(|error| WorkerExecutorError::try_from(error).ok());
        Self::Rejected {
            rejected,
            worker_error,
        }
    }

    pub fn failed(failure: InvocationFailure) -> Self {
        let worker_error = failure
            .worker_error
            .clone()
            .and_then(|error| WorkerExecutorError::try_from(error).ok());
        Self::Failed {
            failure,
            worker_error,
        }
    }

    pub fn worker_error(&self) -> Option<&WorkerExecutorError> {
        match self {
            Self::Rejected { worker_error, .. } | Self::Failed { worker_error, .. } => {
                worker_error.as_ref()
            }
        }
    }

    /// The reason of a rejection, or `None` for a failure or an unknown reason.
    pub fn rejection_reason(&self) -> Option<InvocationRejectionReason> {
        match self {
            Self::Rejected { rejected, .. } => {
                InvocationRejectionReason::try_from(rejected.reason).ok()
            }
            Self::Failed { .. } => None,
        }
    }
}

impl Display for InvocationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { rejected, .. } => {
                // The reason says as what the executor refused the invocation before acceptance.
                let reason = InvocationRejectionReason::try_from(rejected.reason)
                    .map(|reason| reason.as_str_name())
                    .unwrap_or("UNKNOWN");
                match &rejected.worker_error {
                    Some(worker_error) => write!(
                        f,
                        "Agent invocation rejected ({reason}): {} ({worker_error:?})",
                        rejected.error
                    ),
                    None => write!(
                        f,
                        "Agent invocation rejected ({reason}): {}",
                        rejected.error
                    ),
                }
            }
            Self::Failed { failure, .. } => write!(f, "Agent invocation failed: {failure:?}"),
        }
    }
}

impl std::error::Error for InvocationError {}

/// Reads the [`InvocationError`] inside an [`anyhow::Error`], also through added context.
pub trait InvocationErrorExt {
    fn invocation_error(&self) -> Option<&InvocationError>;

    fn worker_error(&self) -> Option<&WorkerExecutorError> {
        self.invocation_error()
            .and_then(InvocationError::worker_error)
    }
}

impl InvocationErrorExt for anyhow::Error {
    fn invocation_error(&self) -> Option<&InvocationError> {
        self.downcast_ref::<InvocationError>()
    }
}

/// Asserts that an [`anyhow::Error`] holds an [`InvocationError`] whose worker error matches
/// the pattern, with an optional guard and an optional message. On a mismatch it panics with the
/// message, the worker error it found and the full error.
///
/// ```ignore
/// assert_worker_error!(
///     error,
///     WorkerExecutorError::InvocationFailed { error: AgentError::InternalError(message), .. }
///         if message == "method gave up"
/// );
/// assert_worker_error!(error, WorkerExecutorError::AgentNotFound { .. }, "{mode:?}");
/// ```
#[macro_export]
macro_rules! assert_worker_error {
    ($error:expr, $($pattern:pat_param)|+ $(if $guard:expr)? $(,)?) => {
        $crate::assert_worker_error!(
            $error,
            $($pattern)|+ $(if $guard)?,
            "the invocation error"
        )
    };
    ($error:expr, $($pattern:pat_param)|+ $(if $guard:expr)?, $($message:tt)+) => {{
        let error: &::anyhow::Error = &$error;
        match $crate::InvocationErrorExt::worker_error(error) {
            ::std::option::Option::Some($($pattern)|+) $(if $guard)? => {}
            other => ::std::panic!(
                "{}: expected a worker error matching `{}`, found {:?}; the error: {:#}",
                ::std::format_args!($($message)+),
                ::std::stringify!($($pattern)|+ $(if $guard)?),
                other,
                error
            ),
        }
    }};
}
