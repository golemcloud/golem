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

//! Shared log emission policy used by both the P2 (`DurableWorkerCtx::emit_log_event`) and the
//! P3 (`p3::cli`) log paths.
//!
//! The policy decides, for a worker log event (stdout/stderr/log), how it is forwarded to the
//! worker event service, whether it is written to the oplog, and how replay deduplication
//! (`seen_log`) interacts with these decisions. The two preview versions only differ in how they
//! access worker state, not in the policy itself.

use crate::durable_host::PublicDurableWorkerState;
use crate::durable_host::replay_state::ReplayState;
use crate::model::event::InternalWorkerEvent;
use crate::services::HasWorker;
use crate::services::oplog::Oplog;
use crate::workerctx::{LogEventEmitBehaviour, PublicWorkerIo, WorkerCtx};
use golem_common::model::OwnedAgentId;
use golem_common::model::oplog::{LogLevel, OplogEntry};
use std::sync::Arc;

/// Applies the common log emission policy for a single worker log event.
///
/// `is_live` must be sampled from the worker state at the time of the call; `oplog` must be the
/// worker's private oplog (used by the [`LogEventEmitBehaviour::Always`] branch, which appends
/// without going through the invocation queue).
pub async fn emit_log_event_with_state<Ctx: WorkerCtx>(
    event: InternalWorkerEvent,
    has_oplog_processor: bool,
    owned_agent_id: &OwnedAgentId,
    public_state: &PublicDurableWorkerState<Ctx>,
    replay_state: &ReplayState,
    oplog: &Arc<dyn Oplog>,
    is_live: bool,
) {
    if let Some(entry) = event.as_oplog_entry()
        && let OplogEntry::Log {
            level,
            context,
            message,
            ..
        } = &entry
    {
        // Oplog processor plugin logs are emitted into the server log because
        // they cannot be easily watched with CLI tools.
        if has_oplog_processor {
            match level {
                LogLevel::Stdout | LogLevel::Debug | LogLevel::Trace => {
                    tracing::debug!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
                LogLevel::Stderr | LogLevel::Info => {
                    tracing::info!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
                LogLevel::Warn => {
                    tracing::warn!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
                LogLevel::Error | LogLevel::Critical => {
                    tracing::error!(
                        plugin_agent = %owned_agent_id,
                        context,
                        "Plugin: {message}"
                    );
                }
            }
        }

        match Ctx::LOG_EVENT_EMIT_BEHAVIOUR {
            LogEventEmitBehaviour::LiveOnly => {
                // Stdout and stderr writes are persistent and overwritten by sending the data to
                // the event service instead of the real output stream
                if is_live {
                    if !replay_state.seen_log(*level, context, message).await {
                        // haven't seen this log before
                        public_state.event_service().emit_event(event.clone(), true);
                        public_state.worker().add_to_oplog(entry).await;
                    } else {
                        // we have persisted emitting this log before, so we mark it as non-live and
                        // remove the entry from the seen log set.
                        // note that we still call emit_event because we need replayed log events
                        // for improved error reporting in case of invocation failures
                        public_state
                            .event_service()
                            .emit_event(event.clone(), false);
                        replay_state.remove_seen_log(*level, context, message).await;
                    }
                }
            }
            LogEventEmitBehaviour::Always => {
                public_state.event_service().emit_event(event.clone(), true);

                if is_live && !replay_state.seen_log(*level, context, message).await {
                    oplog.add(entry).await;
                }
            }
        }
    }
}
