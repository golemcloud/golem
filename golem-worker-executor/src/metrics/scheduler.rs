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

use lazy_static::lazy_static;
use prometheus::*;
use std::time::Duration;

lazy_static! {
    pub static ref SCHEDULED_ACTION_LAG_SECONDS: HistogramVec = register_histogram_vec!(
        "scheduled_action_lag_seconds",
        "Wall-clock delay in seconds between scheduled_at and the time the action fires",
        &["executor_id"],
        crate::metrics::SCHEDULER_LAG_BUCKETS.to_vec()
    )
    .unwrap();
    pub static ref SCHEDULER_QUEUE_DEPTH: GaugeVec = register_gauge_vec!(
        "scheduler_queue_depth",
        "Count of matching actions to process at the start of each scheduler process() iteration",
        &["executor_id"]
    )
    .unwrap();
    pub static ref SCHEDULER_DUE_ACTION_BACKLOG_AFTER_TICK: GaugeVec = register_gauge_vec!(
        "scheduler_due_action_backlog_after_tick",
        "Due scheduled actions not yet acknowledged after scheduler tick processing",
        &["executor_id"]
    )
    .unwrap();
    pub static ref SCHEDULER_TICK_DURATION_SECONDS: HistogramVec = register_histogram_vec!(
        "scheduler_tick_duration_seconds",
        "Wall time of a single scheduler process() iteration",
        &["executor_id"],
        crate::metrics::SCHEDULER_TICK_DURATION_BUCKETS.to_vec()
    )
    .unwrap();
    pub static ref SCHEDULED_ACTION_PROCESSING_SECONDS: HistogramVec = register_histogram_vec!(
        "scheduled_action_processing_seconds",
        "Time to process a claimed scheduled action before acknowledgement",
        &["executor_id", "action_kind"],
        crate::metrics::SCHEDULER_TICK_DURATION_BUCKETS.to_vec()
    )
    .unwrap();
    pub static ref SCHEDULED_ACTION_SIZE_BYTES: HistogramVec = register_histogram_vec!(
        "scheduled_action_size_bytes",
        "Serialized blob size in bytes of a ScheduledAction at insert time",
        &["executor_id", "action_kind"],
        crate::metrics::BLOB_SIZE_BUCKETS.to_vec()
    )
    .unwrap();
}

pub fn record_scheduled_action_lag(lag: Duration) {
    SCHEDULED_ACTION_LAG_SECONDS
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .observe(lag.as_secs_f64());
}

pub fn set_scheduler_queue_depth(depth: usize) {
    SCHEDULER_QUEUE_DEPTH
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .set(depth as f64);
}

pub fn set_scheduler_due_action_backlog_after_tick(backlog: u64) {
    SCHEDULER_DUE_ACTION_BACKLOG_AFTER_TICK
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .set(backlog as f64);
}

pub fn record_scheduler_tick_duration(duration: Duration) {
    SCHEDULER_TICK_DURATION_SECONDS
        .with_label_values(&[crate::metrics::storage::executor_id()])
        .observe(duration.as_secs_f64());
}

pub fn record_scheduled_action_processing(action_kind: &'static str, duration: Duration) {
    SCHEDULED_ACTION_PROCESSING_SECONDS
        .with_label_values(&[crate::metrics::storage::executor_id(), action_kind])
        .observe(duration.as_secs_f64());
}

pub fn record_scheduled_action_size(action_kind: &'static str, bytes: usize) {
    SCHEDULED_ACTION_SIZE_BYTES
        .with_label_values(&[crate::metrics::storage::executor_id(), action_kind])
        .observe(bytes as f64);
}

/// Maps a `ScheduledAction` to its metric `action_kind` label value.
pub fn action_kind_label(action: &golem_common::model::ScheduledAction) -> &'static str {
    use golem_common::model::ScheduledAction;
    match action {
        ScheduledAction::CompletePromise { .. } => "complete_promise",
        ScheduledAction::ArchiveOplog { .. } => "archive_oplog",
        ScheduledAction::Invoke { .. } | ScheduledAction::InvokeEphemeral { .. } => "invoke",
        ScheduledAction::ExpireDurableStreamSession { .. } => "expire_stream_session",
        ScheduledAction::Resume { .. } => "resume",
    }
}
