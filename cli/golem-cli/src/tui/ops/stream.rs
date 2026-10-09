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

use super::AgentIdentity;
use crate::command_handler::{AgentStreamConsumer, AgentStreamItem, Handlers};
use crate::context::Context;
use crate::model::agent::RawAgentId;
use crate::tui::TuiEvent;
use crate::tui::context_executor::TuiContextId;
use golem_common::model::component::ComponentName;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::Sender;

pub(in crate::tui) const MAX_EVENTS: usize = 10_000;
pub(in crate::tui) const MAX_BYTES: usize = 16 * 1024 * 1024;
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_millis(250);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub(in crate::tui) enum StreamMessage {
    Connected,
    Reconnecting {
        attempt: u32,
        delay: Duration,
        error: Option<String>,
    },
    Item(AgentStreamItem),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tui) enum StreamStatus {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
        delay: Duration,
        error: Option<String>,
    },
    Failed(String),
}

impl StreamStatus {
    pub(in crate::tui) fn label(&self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting { .. } => "Reconnecting",
            Self::Failed(_) => "Failed",
        }
    }
}

pub(in crate::tui) struct StreamState {
    pub(in crate::tui) generation: u64,
    pub(in crate::tui) context_id: Option<TuiContextId>,
    pub(in crate::tui) target: Option<AgentIdentity>,
    pub(in crate::tui) status: StreamStatus,
    pub(in crate::tui) events: VecDeque<AgentStreamItem>,
    pub(in crate::tui) buffered_bytes: usize,
    pub(in crate::tui) unread: u64,
    pub(in crate::tui) server_missed: u64,
    pub(in crate::tui) locally_dropped: u64,
    pub(in crate::tui) follow: bool,
    pub(in crate::tui) scroll_from_end: usize,
    pub(in crate::tui) task: Option<tokio::task::JoinHandle<()>>,
}

impl Default for StreamState {
    fn default() -> Self {
        Self {
            generation: 0,
            context_id: None,
            target: None,
            status: StreamStatus::Disconnected,
            events: VecDeque::new(),
            buffered_bytes: 0,
            unread: 0,
            server_missed: 0,
            locally_dropped: 0,
            follow: true,
            scroll_from_end: 0,
            task: None,
        }
    }
}

impl StreamState {
    pub(in crate::tui) fn prepare_connect(
        &mut self,
        target: AgentIdentity,
        context_id: TuiContextId,
    ) -> u64 {
        self.abort_task();
        self.generation = self.generation.saturating_add(1);
        self.context_id = Some(context_id);
        self.target = Some(target);
        self.status = StreamStatus::Connecting;
        self.events.clear();
        self.buffered_bytes = 0;
        self.unread = 0;
        self.server_missed = 0;
        self.locally_dropped = 0;
        self.follow = true;
        self.scroll_from_end = 0;
        self.generation
    }

    pub(in crate::tui) fn attach_task(&mut self, task: tokio::task::JoinHandle<()>) {
        self.task = Some(task);
    }

    pub(in crate::tui) fn apply_message(
        &mut self,
        generation: u64,
        message: StreamMessage,
        visible: bool,
    ) {
        if generation != self.generation {
            return;
        }
        match message {
            StreamMessage::Connected => self.status = StreamStatus::Connected,
            StreamMessage::Reconnecting {
                attempt,
                delay,
                error,
            } => {
                self.status = StreamStatus::Reconnecting {
                    attempt,
                    delay,
                    error,
                }
            }
            StreamMessage::Item(item) => self.push(item, visible),
        }
    }

    pub(in crate::tui) fn finish(
        &mut self,
        generation: u64,
        context_id: TuiContextId,
        result: Result<(), String>,
    ) {
        if generation != self.generation || Some(context_id) != self.context_id {
            return;
        }
        self.task = None;
        self.status = match result {
            Ok(()) => StreamStatus::Disconnected,
            Err(error) => StreamStatus::Failed(error),
        };
    }

    pub(in crate::tui) fn disconnect(&mut self) {
        self.abort_task();
        self.generation = self.generation.saturating_add(1);
        self.context_id = None;
        self.status = StreamStatus::Disconnected;
    }

    pub(in crate::tui) fn reset(&mut self) {
        self.disconnect();
        self.target = None;
        self.events.clear();
        self.buffered_bytes = 0;
        self.unread = 0;
        self.server_missed = 0;
        self.locally_dropped = 0;
        self.follow = true;
        self.scroll_from_end = 0;
    }

    pub(in crate::tui) fn mark_visible(&mut self) {
        self.unread = 0;
    }

    pub(in crate::tui) fn scroll_up(&mut self, rows: usize) {
        self.scroll_from_end = self
            .scroll_from_end
            .saturating_add(rows)
            .min(self.events.len().saturating_sub(1));
        self.follow = self.scroll_from_end == 0;
    }

    pub(in crate::tui) fn scroll_down(&mut self, rows: usize) {
        self.scroll_from_end = self.scroll_from_end.saturating_sub(rows);
        self.follow = self.scroll_from_end == 0;
    }

    pub(in crate::tui) fn scroll_to_start(&mut self) {
        self.scroll_from_end = self.events.len().saturating_sub(1);
        self.follow = self.events.len() <= 1;
    }

    pub(in crate::tui) fn follow_latest(&mut self) {
        self.scroll_from_end = 0;
        self.follow = true;
    }

    pub(in crate::tui) fn visible_range(&self, height: usize) -> std::ops::Range<usize> {
        let end = self.events.len().saturating_sub(self.scroll_from_end);
        end.saturating_sub(height)..end
    }

    fn push(&mut self, item: AgentStreamItem, visible: bool) {
        self.push_with_limits(item, visible, MAX_EVENTS, MAX_BYTES);
    }

    fn push_with_limits(
        &mut self,
        item: AgentStreamItem,
        visible: bool,
        max_events: usize,
        max_bytes: usize,
    ) {
        if let AgentStreamItem::ServerLagged { missed, .. } = &item {
            self.server_missed = self.server_missed.saturating_add(*missed);
        }
        if !visible || !self.follow {
            self.unread = self.unread.saturating_add(1);
        }
        if !self.follow {
            self.scroll_from_end = self.scroll_from_end.saturating_add(1);
        }
        self.buffered_bytes = self.buffered_bytes.saturating_add(item_size(&item));
        self.events.push_back(item);
        while self.events.len() > max_events || self.buffered_bytes > max_bytes {
            let Some(removed) = self.events.pop_front() else {
                break;
            };
            self.buffered_bytes = self.buffered_bytes.saturating_sub(item_size(&removed));
            self.locally_dropped = self.locally_dropped.saturating_add(1);
            self.scroll_from_end = self
                .scroll_from_end
                .min(self.events.len().saturating_sub(1));
        }
    }

    fn abort_task(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for StreamState {
    fn drop(&mut self) {
        self.abort_task();
    }
}

struct TuiStreamConsumer {
    generation: u64,
    control_tx: Sender<TuiEvent>,
    output_tx: Sender<TuiEvent>,
    connected: AtomicBool,
}

impl TuiStreamConsumer {
    fn take_connected(&self) -> bool {
        self.connected.swap(false, Ordering::AcqRel)
    }
}

#[async_trait::async_trait]
impl AgentStreamConsumer for TuiStreamConsumer {
    async fn connected(&self) {
        self.connected.store(true, Ordering::Release);
        let _ = self
            .control_tx
            .send(TuiEvent::StreamMessage {
                generation: self.generation,
                message: StreamMessage::Connected,
            })
            .await;
    }

    async fn consume(&self, item: AgentStreamItem) {
        let _ = self
            .output_tx
            .send(TuiEvent::StreamMessage {
                generation: self.generation,
                message: StreamMessage::Item(item),
            })
            .await;
    }
}

pub(in crate::tui) async fn run_session(
    context: Arc<Context>,
    target: AgentIdentity,
    generation: u64,
    control_tx: Sender<TuiEvent>,
    output_tx: Sender<TuiEvent>,
) -> anyhow::Result<()> {
    let consumer = Arc::new(TuiStreamConsumer {
        generation,
        control_tx: control_tx.clone(),
        output_tx,
        connected: AtomicBool::new(false),
    });
    let connection = context
        .agent_handler()
        .stream_connection(
            &ComponentName(target.component),
            &RawAgentId(target.agent_id),
            consumer.clone(),
        )
        .await?;
    let mut attempt = 0_u32;
    loop {
        let result = connection.run_once().await;
        if consumer.take_connected() {
            attempt = 0;
        }
        attempt = attempt.saturating_add(1);
        let delay = reconnect_delay(attempt);
        let error = result.err().map(|error| format!("{error:#}"));
        control_tx
            .send(TuiEvent::StreamMessage {
                generation,
                message: StreamMessage::Reconnecting {
                    attempt,
                    delay,
                    error,
                },
            })
            .await?;
        tokio::time::sleep(delay).await;
    }
}

fn reconnect_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(16);
    INITIAL_RECONNECT_DELAY
        .saturating_mul(2_u32.saturating_pow(exponent))
        .min(MAX_RECONNECT_DELAY)
}

pub(in crate::tui) fn item_size(item: &AgentStreamItem) -> usize {
    std::mem::size_of::<AgentStreamItem>()
        + match item {
            AgentStreamItem::Stdout { message, .. }
            | AgentStreamItem::Stderr { message, .. }
            | AgentStreamItem::PluginError { message, .. } => message.len(),
            AgentStreamItem::Log {
                context, message, ..
            } => context.len().saturating_add(message.len()),
            AgentStreamItem::InvocationStarted { function, .. }
            | AgentStreamItem::InvocationFinished { function, .. } => function.len(),
            AgentStreamItem::SnapshotRecoveryFailed { error, .. }
            | AgentStreamItem::StreamError { error, .. } => error.len(),
            AgentStreamItem::SnapshotRecoverySucceeded { .. }
            | AgentStreamItem::ServerLagged { .. }
            | AgentStreamItem::StreamClosed { .. } => 0,
        }
}

#[cfg(test)]
mod tests {
    use super::{StreamState, StreamStatus, reconnect_delay};
    use crate::command_handler::AgentStreamItem;
    use golem_common::model::Timestamp;
    use std::time::Duration;
    use test_r::test;

    fn stdout(message: &str) -> AgentStreamItem {
        AgentStreamItem::Stdout {
            timestamp: Timestamp::now_utc(),
            message: message.to_string(),
        }
    }

    #[test]
    fn stream_buffer_evicts_by_count_and_tracks_local_drops() {
        let mut state = StreamState::default();
        state.push_with_limits(stdout("one"), false, 2, usize::MAX);
        state.push_with_limits(stdout("two"), false, 2, usize::MAX);
        state.push_with_limits(stdout("three"), false, 2, usize::MAX);

        assert_eq!(state.events.len(), 2);
        assert_eq!(state.locally_dropped, 1);
        assert_eq!(state.unread, 3);
    }

    #[test]
    fn stream_buffer_evicts_by_bytes() {
        let mut state = StreamState::default();
        let item = stdout("large payload");
        let one_item = super::item_size(&item);
        state.push_with_limits(item.clone(), true, 10, one_item);
        state.push_with_limits(item, true, 10, one_item);

        assert_eq!(state.events.len(), 1);
        assert_eq!(state.locally_dropped, 1);
        assert_eq!(state.unread, 0);
    }

    #[test]
    fn server_and_local_drop_counts_stay_distinct() {
        let mut state = StreamState::default();
        state.push_with_limits(
            AgentStreamItem::ServerLagged {
                timestamp: Timestamp::now_utc(),
                missed: 17,
            },
            true,
            1,
            usize::MAX,
        );
        state.push_with_limits(stdout("next"), true, 1, usize::MAX);

        assert_eq!(state.server_missed, 17);
        assert_eq!(state.locally_dropped, 1);
    }

    #[test]
    fn reconnect_delay_is_exponential_and_capped() {
        assert_eq!(reconnect_delay(1), Duration::from_millis(250));
        assert_eq!(reconnect_delay(4), Duration::from_secs(2));
        assert_eq!(reconnect_delay(100), Duration::from_secs(30));
    }

    #[test]
    fn explicit_disconnect_preserves_buffer_for_inspection() {
        let mut state = StreamState::default();
        state.status = StreamStatus::Connected;
        state.push_with_limits(stdout("kept"), true, 10, usize::MAX);

        state.disconnect();

        assert_eq!(state.status, StreamStatus::Disconnected);
        assert_eq!(state.events.len(), 1);
    }
}
