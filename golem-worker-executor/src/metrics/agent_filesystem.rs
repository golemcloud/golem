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

use golem_common::metrics::catalog;
use lazy_static::lazy_static;
use prometheus::*;
use std::time::Duration;

lazy_static! {
    static ref PRESSURE_RECOVERY_SECONDS: HistogramVec =
        catalog::FILESYSTEM_PRESSURE_RECOVERY_SECONDS.register();
    static ref PRESSURE_UNLOAD_ATTEMPTS: IntCounterVec =
        catalog::FILESYSTEM_PRESSURE_UNLOAD_ATTEMPTS_TOTAL.register();
    static ref STORAGE_REFUSALS: IntCounterVec =
        catalog::FILESYSTEM_STORAGE_REFUSALS_TOTAL.register();
    static ref PROJECT_CLEANUP_FAILURES: IntCounterVec =
        catalog::FILESYSTEM_PROJECT_CLEANUP_FAILURES_TOTAL.register();
    static ref PROJECT_STALE_RECLAIMS: IntCounter =
        catalog::FILESYSTEM_PROJECT_STALE_RECLAIMS_TOTAL.register();
}

label_values! {
    /// How a recovery of free space ended, as the `outcome` label.
    PressureRecovery {
        /// The unloads of idle agents freed enough space, and the write tries again.
        Recovered => "recovered",
        /// No idle agent was left to unload before the free space reached the target.
        TargetNotReached => "target_not_reached",
        /// The recovery did not end before its deadline.
        Deadline => "deadline",
        /// The free space of the volume could not be observed.
        ObservationFailed => "observation_failed",
        /// The cleanup of an unloaded agent filesystem failed.
        CleanupFailed => "cleanup_failed",
        /// The volume had enough free space, so the failure has another cause.
        NotUnderPressure => "not_under_pressure",
        /// The executor is shutting down and has no agents to unload.
        Unavailable => "unavailable",
    }
}

label_values! {
    /// How one unload of an idle agent in a recovery of free space ended, as the `outcome` label.
    PressureUnload {
        /// The agent was unloaded and its filesystem cleaned up.
        Unloaded => "unloaded",
        /// The agent was no longer an idle candidate when the unload ran, for example because
        /// it got new work or is being deleted. The recovery goes on with the next candidate.
        Ineligible => "ineligible",
        /// The agent was stopped, but the cleanup of its filesystem failed. The recovery stops.
        CleanupFailed => "cleanup_failed",
        /// The unload task panicked or was cancelled. The recovery stops.
        TaskFailed => "task_failed",
        /// The deadline of the recovery passed before the unload finished. The unload task can
        /// still finish later.
        Deadline => "deadline",
    }
}

label_values! {
    /// Why an agent filesystem call found no space, as the `cause` label.
    StorageRefusal {
        /// The agent used up its own quota.
        AgentQuota => "agent_quota",
        /// The volume has no space left, and a recovery did not free enough.
        PhysicalCapacity => "physical_capacity",
    }
}

label_values! {
    /// The cleanup of a managed XFS project that failed, as the `operation` label.
    ProjectCleanup {
        /// The delete of an agent filesystem.
        Delete => "delete",
        /// The fallback cleanup of an agent filesystem that was dropped without a delete.
        Drop => "drop",
        /// The cleanup of a project that a new agent filesystem found left over at its path.
        Stale => "stale",
    }
}

/// Exports each label value at zero, so that a dashboard or an alert reads 0 instead of no data
/// before the first event. Called when the executor opens an XFS volume for the agent
/// filesystems. `project_quotas` tells that the volume has managed XFS projects.
pub fn record_agent_filesystem_volume_opened(project_quotas: bool) {
    for outcome in PressureRecovery::ALL {
        PRESSURE_RECOVERY_SECONDS.with_label_values(&[outcome.label()]);
    }
    for outcome in PressureUnload::ALL {
        PRESSURE_UNLOAD_ATTEMPTS.with_label_values(&[outcome.label()]);
    }
    for cause in StorageRefusal::ALL {
        STORAGE_REFUSALS.with_label_values(&[cause.label()]);
    }
    if project_quotas {
        for operation in ProjectCleanup::ALL {
            PROJECT_CLEANUP_FAILURES.with_label_values(&[operation.label()]);
        }
        lazy_static::initialize(&PROJECT_STALE_RECLAIMS);
    }
}

pub fn record_pressure_recovery(outcome: PressureRecovery, elapsed: Duration) {
    PRESSURE_RECOVERY_SECONDS
        .with_label_values(&[outcome.label()])
        .observe(elapsed.as_secs_f64());
}

pub fn record_pressure_unload_attempt(outcome: PressureUnload) {
    PRESSURE_UNLOAD_ATTEMPTS
        .with_label_values(&[outcome.label()])
        .inc();
}

pub fn record_storage_refusal(cause: StorageRefusal) {
    STORAGE_REFUSALS.with_label_values(&[cause.label()]).inc();
}

pub fn record_project_cleanup_failure(operation: ProjectCleanup) {
    PROJECT_CLEANUP_FAILURES
        .with_label_values(&[operation.label()])
        .inc();
}

pub fn record_project_stale_reclaim() {
    PROJECT_STALE_RECLAIMS.inc();
}
