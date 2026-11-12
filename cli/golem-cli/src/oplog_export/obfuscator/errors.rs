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

//! Error text. The public `Error` entry holds only `AgentError::message()` and the error logs,
//! so the client cannot read the variant. A message that is exactly a fixed Golem text is kept;
//! every other message is free text.

use super::value::{Region, Walker, record, text};
use crate::oplog_export::model::{ReportField, ReportNode};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{
    AgentError, AgentTerminatedByQuotaError, EphemeralCannotSuspendError,
    EphemeralFuelExhaustedError, EphemeralSleepTooLongError, ReadOnlyViolationError,
};
use golem_common::model::quota::ResourceName;
use std::collections::BTreeSet;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorText {
    /// `message()` is a literal of the executor.
    Fixed,
    /// `message()` is the text the variant holds.
    Free,
}

pub fn agent_error_text(error: &AgentError) -> ErrorText {
    match error {
        AgentError::Unknown(_) => ErrorText::Free,
        AgentError::InvalidRequest(_) => ErrorText::Free,
        AgentError::StackOverflow => ErrorText::Fixed,
        AgentError::OutOfMemory => ErrorText::Fixed,
        AgentError::ExceededMemoryLimit => ErrorText::Fixed,
        AgentError::InternalError(_) => ErrorText::Free,
        AgentError::DeterministicTrap(_) => ErrorText::Free,
        AgentError::TransientError(_) => ErrorText::Free,
        AgentError::PermanentError(_) => ErrorText::Free,
        AgentError::ExceededTableLimit => ErrorText::Fixed,
        AgentError::ExceededHttpCallLimit => ErrorText::Fixed,
        AgentError::ExceededRpcCallLimit => ErrorText::Fixed,
        AgentError::AgentTerminatedByQuota(_) => ErrorText::Fixed,
        AgentError::EphemeralSleepTooLong(_) => ErrorText::Fixed,
        AgentError::EphemeralFuelExhausted(_) => ErrorText::Fixed,
        AgentError::EphemeralCannotSuspend(_) => ErrorText::Fixed,
        AgentError::ReadOnlyViolation(_) => ErrorText::Fixed,
        AgentError::PermissionDenied(_) => ErrorText::Free,
    }
}

/// A distinct number per variant, so a test can check that the samples cover every variant.
pub fn agent_error_ordinal(error: &AgentError) -> usize {
    match error {
        AgentError::Unknown(_) => 0,
        AgentError::InvalidRequest(_) => 1,
        AgentError::StackOverflow => 2,
        AgentError::OutOfMemory => 3,
        AgentError::ExceededMemoryLimit => 4,
        AgentError::InternalError(_) => 5,
        AgentError::DeterministicTrap(_) => 6,
        AgentError::TransientError(_) => 7,
        AgentError::PermanentError(_) => 8,
        AgentError::ExceededTableLimit => 9,
        AgentError::ExceededHttpCallLimit => 10,
        AgentError::ExceededRpcCallLimit => 11,
        AgentError::AgentTerminatedByQuota(_) => 12,
        AgentError::EphemeralSleepTooLong(_) => 13,
        AgentError::EphemeralFuelExhausted(_) => 14,
        AgentError::EphemeralCannotSuspend(_) => 15,
        AgentError::ReadOnlyViolation(_) => 16,
        AgentError::PermissionDenied(_) => 17,
    }
}

/// One value of each `AgentError` variant.
pub fn agent_error_samples() -> Vec<AgentError> {
    let text = || "sample".to_string();
    vec![
        AgentError::Unknown(text()),
        AgentError::InvalidRequest(text()),
        AgentError::StackOverflow,
        AgentError::OutOfMemory,
        AgentError::ExceededMemoryLimit,
        AgentError::InternalError(text()),
        AgentError::DeterministicTrap(text()),
        AgentError::TransientError(text()),
        AgentError::PermanentError(text()),
        AgentError::ExceededTableLimit,
        AgentError::ExceededHttpCallLimit,
        AgentError::ExceededRpcCallLimit,
        AgentError::AgentTerminatedByQuota(AgentTerminatedByQuotaError {
            environment_id: EnvironmentId::new(),
            resource_name: ResourceName(text()),
        }),
        AgentError::EphemeralSleepTooLong(EphemeralSleepTooLongError {
            requested_nanos: 2,
            max_nanos: 1,
        }),
        AgentError::EphemeralFuelExhausted(EphemeralFuelExhaustedError { overdraft_limit: 1 }),
        AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError { reason: text() }),
        AgentError::ReadOnlyViolation(ReadOnlyViolationError {
            method: text(),
            host_function: text(),
        }),
        AgentError::PermissionDenied(text()),
    ]
}

/// The messages of the variants whose `message()` is a literal.
pub static FIXED_ERROR_TEXTS: LazyLock<BTreeSet<String>> = LazyLock::new(|| {
    agent_error_samples()
        .iter()
        .filter(|error| agent_error_text(error) == ErrorText::Fixed)
        .map(|error| error.message().to_string())
        .collect()
});

/// The code literals that the executor writes at the start of `FailedUpdate.details`.
pub const FAILED_UPDATE_DETAIL_CODES: [&str; 9] = [
    "UPDATE_SNAPSHOT_UNAVAILABLE",
    "UPDATE_SNAPSHOT_INCOMPATIBLE",
    "UPDATE_REPLAY_FAILED",
    "UPDATE_RESTORE_NEEDS_FILESYSTEM_SNAPSHOTS",
    "UPDATE_SNAPSHOT_RESTORE_FAILED",
    "UPDATE_TARGET_NOT_FOUND",
    "UPDATE_TARGET_REFUSED",
    "UPDATE_BLOB_STORAGE_REFUSED",
    "UPDATE_NEEDS_FILESYSTEM_SNAPSHOTS",
];

impl Walker<'_, '_> {
    /// The `error` of an `Error` entry: a fixed Golem text by exact match, or error text.
    pub(super) fn agent_error_message(&self, message: &str) -> ReportNode {
        if FIXED_ERROR_TEXTS.contains(message) {
            text(message)
        } else {
            self.str_leaf(message, Region::Error)
        }
    }

    /// `FailedUpdate.details`: a known code prefix is kept, the rest is error text.
    pub(super) fn failed_update_details(&self, details: &str) -> ReportNode {
        for code in FAILED_UPDATE_DETAIL_CODES {
            if details == code {
                return text(code);
            }
            if let Some(rest) = details
                .strip_prefix(code)
                .and_then(|rest| rest.strip_prefix(": "))
            {
                return record(
                    None,
                    vec![
                        ReportField {
                            name: Some("code".to_string()),
                            value: text(code),
                        },
                        ReportField {
                            name: Some("detail".to_string()),
                            value: self.str_leaf(rest, Region::Error),
                        },
                    ],
                );
            }
        }
        self.str_leaf(details, Region::Error)
    }
}
