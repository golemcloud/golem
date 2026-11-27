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
    pub static ref SCHEDULED_ACTION_LAG_SECONDS: HistogramVec =
        catalog::SCHEDULED_ACTION_LAG_SECONDS.register();
    pub static ref SCHEDULER_QUEUE_DEPTH: GaugeVec = catalog::SCHEDULER_QUEUE_DEPTH.register();
    pub static ref SCHEDULER_DUE_ACTION_BACKLOG_AFTER_TICK: GaugeVec =
        catalog::SCHEDULER_DUE_ACTION_BACKLOG_AFTER_TICK.register();
    pub static ref SCHEDULER_TICK_DURATION_SECONDS: HistogramVec =
        catalog::SCHEDULER_TICK_DURATION_SECONDS.register();
    pub static ref SCHEDULED_ACTION_PROCESSING_SECONDS: HistogramVec =
        catalog::SCHEDULED_ACTION_PROCESSING_SECONDS.register();
    pub static ref SCHEDULED_ACTION_SIZE_BYTES: HistogramVec =
        catalog::SCHEDULED_ACTION_SIZE_BYTES.register();
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
        ScheduledAction::DeleteFinishedInvocation { .. } => "delete_finished_invocation",
        ScheduledAction::WakeInvocation { .. } => "wake_invocation",
        ScheduledAction::ExpireDurableStreamSession { .. } => "expire_stream_session",
        ScheduledAction::Resume { .. } => "resume",
    }
}
