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

//! The response events of this replica, kept so that a client can resume a cut response stream
//! on the same replica. Each stream keeps its last 16 events.
//!
//! Retention: a stream whose call runs is kept; otherwise it is kept for 60 seconds after its
//! last JSON-RPC response or error, or for one hour after its last event.

use super::sharded::Sharded;
use super::store::McpSessionId;
use futures::{Stream, StreamExt, stream};
use rmcp::model::ServerJsonRpcMessage;
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use rmcp::transport::streamable_http_server::session::store::{
    EventId, EventStore, EventStoreError, EventStream,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

/// The events kept per stream.
const EVENTS_PER_STREAM: usize = 16;
/// How long a stream is kept after its last JSON-RPC response or error.
const COMPLETED_STREAM_KEPT: Duration = Duration::from_secs(60);
/// How long a stream that never completes is kept after its last event.
const IDLE_STREAM_KEPT: Duration = Duration::from_secs(60 * 60);

/// An event with its position in its stream.
type Event = (u64, ServerSseMessage);

#[derive(Debug)]
pub(super) struct StreamLog {
    events: VecDeque<Event>,
    next_seq: u64,
    /// Present while the stream can still get events; live replays subscribe to it.
    live: Option<broadcast::Sender<Event>>,
    last_event_at: Instant,
    completed_at: Option<Instant>,
    /// A cancelled stream refuses every further event.
    cancelled: bool,
    /// True while the call that writes the stream runs.
    running: bool,
    /// The session whose client may resume the stream. An unbound stream is never resumed.
    session: Option<McpSessionId>,
}

impl StreamLog {
    fn new(now: Instant) -> Self {
        Self {
            events: VecDeque::with_capacity(EVENTS_PER_STREAM + 1),
            next_seq: 0,
            // A sender without a receiver: nothing is dropped when a log is built under a
            // shard lock.
            live: Some(broadcast::Sender::new(EVENTS_PER_STREAM)),
            last_event_at: now,
            completed_at: None,
            cancelled: false,
            running: false,
            session: None,
        }
    }
}

/// A stored event: its position, the event to send to live replays, the sender to send it on
/// (taken out of the log for the terminal event, so that live replays end after it), and the
/// event that fell out of the log.
struct Stored {
    seq: u64,
    event: ServerSseMessage,
    live: Option<broadcast::Sender<Event>>,
    evicted: Option<Event>,
}

/// The event store refused the event of a cancelled stream.
#[derive(Debug, thiserror::Error)]
#[error("the response stream was cancelled")]
struct StreamCancelled;

fn is_terminal(event: &ServerSseMessage) -> bool {
    event.message.as_deref().is_some_and(|message| {
        matches!(
            message,
            ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_)
        )
    })
}

/// Appends the event to the log, unless the stream was cancelled.
fn append_event(log: &mut StreamLog, event: ServerSseMessage, now: Instant) -> Option<Stored> {
    if log.cancelled {
        return None;
    }
    let seq = log.next_seq;
    log.next_seq += 1;
    log.last_event_at = now;
    log.events.push_back((seq, event.clone()));
    let evicted = (log.events.len() > EVENTS_PER_STREAM)
        .then(|| log.events.pop_front())
        .flatten();
    let live = if is_terminal(&event) {
        log.completed_at = Some(now);
        log.live.take()
    } else {
        log.live.clone()
    };
    Some(Stored {
        seq,
        event,
        live,
        evicted,
    })
}

/// Makes the stream refuse every further event, and takes its live sender, whose drop ends
/// every live replay.
fn refuse_further_events(log: &mut StreamLog, now: Instant) -> Option<broadcast::Sender<Event>> {
    log.cancelled = true;
    log.completed_at.get_or_insert(now);
    log.live.take()
}

/// Lets the client of `session` resume the stream. A stream keeps the first session it gets.
fn attach_session(log: &mut StreamLog, session: McpSessionId) {
    log.session.get_or_insert(session);
}

/// Lets the client of `session` resume the stream of its running call, and keeps the stream
/// while the call runs.
fn attach_call(log: &mut StreamLog, session: McpSessionId) {
    attach_session(log, session);
    log.running = true;
}

/// The call that writes the stream has ended. The retention time counts from now, so the stream
/// is not dropped before the call's last event is stored.
fn finish_call(log: &mut StreamLog, now: Instant) {
    log.running = false;
    log.last_event_at = now;
}

fn events_after(log: &StreamLog, seq: Option<u64>) -> Vec<Event> {
    log.events
        .iter()
        .filter(|(stored, _)| seq.is_none_or(|seq| *stored > seq))
        .cloned()
        .collect()
}

/// The stored events after `seq`, and a subscription to later events while the stream can still
/// get any. A completed or cancelled stream gives a finite replay.
fn split_replay(log: &StreamLog, seq: u64) -> (Vec<Event>, Option<broadcast::Receiver<Event>>) {
    (
        events_after(log, Some(seq)),
        log.live.as_ref().map(broadcast::Sender::subscribe),
    )
}

fn keep_stream(log: &StreamLog, now: Instant) -> bool {
    log.running
        || match log.completed_at {
            Some(completed_at) => now.duration_since(completed_at) < COMPLETED_STREAM_KEPT,
            None => now.duration_since(log.last_event_at) < IDLE_STREAM_KEPT,
        }
}

fn event_id(stream: &str, seq: u64) -> String {
    format!("{stream}/{seq}")
}

/// The stream and position an event ID names.
pub(super) fn parse_event_id(id: &str) -> Option<(&str, u64)> {
    let (stream, seq) = id.rsplit_once('/')?;
    Some((stream, seq.parse().ok()?))
}

pub(super) struct ReplicaEventStore {
    logs: Sharded<Box<str>, StreamLog>,
}

impl ReplicaEventStore {
    pub(super) fn new() -> Self {
        Self {
            logs: Sharded::new(),
        }
    }

    /// Runs `step` on the stream's log, created when missing.
    fn with_log<R>(&self, stream: &str, step: impl FnOnce(&mut StreamLog) -> R) -> R {
        let now = Instant::now();
        self.logs.with(stream, |logs| match logs.get_mut(stream) {
            Some(log) => step(log),
            None => step(
                logs.entry(Box::from(stream))
                    .or_insert_with(|| StreamLog::new(now)),
            ),
        })
    }

    /// Lets the client of `session` resume the stream; the stream has no running call.
    pub(super) fn bind_response(&self, stream: &str, session: McpSessionId) {
        self.with_log(stream, |log| attach_session(log, session));
    }

    /// Lets the client of `session` resume the stream of its call, and keeps the stream while
    /// the call runs.
    pub(super) fn bind_call(&self, stream: &str, session: McpSessionId) {
        self.with_log(stream, |log| attach_call(log, session));
    }

    pub(super) fn call_ended(&self, stream: &str) {
        let now = Instant::now();
        self.with_log(stream, |log| finish_call(log, now));
    }

    /// Cancels the stream: it refuses every further event, and its live replays end.
    pub(super) fn mark_cancelled(&self, stream: &str) {
        let now = Instant::now();
        let live = self.with_log(stream, |log| refuse_further_events(log, now));
        drop(live);
    }

    /// The events after `last_event_id`, followed by the stream's later events while it can
    /// still get any. `None` when the stream is unknown or not bound to `session`.
    pub(super) fn replay(
        self: &Arc<Self>,
        last_event_id: &str,
        session: McpSessionId,
    ) -> Option<impl Stream<Item = (EventId, ServerSseMessage)> + Send + 'static> {
        let (stream, seq) = parse_event_id(last_event_id)?;
        let (stored, live) = self.logs.with(stream, |logs| {
            logs.get(stream)
                .filter(|log| log.session == Some(session))
                .map(|log| split_replay(log, seq))
        })?;
        // The replay outlives this call, so it owns the stream ID, shared by its two parts.
        let key: Arc<str> = Arc::from(stream);
        let last = stored.last().map_or(seq, |(seq, _)| *seq);
        let live = live.map(|receiver| self.clone().live_replay(key.clone(), receiver, last));
        Some(
            stream::iter(stored)
                .chain(stream::iter(live).flatten())
                .map(move |(seq, event)| (event_id(&key, seq), event)),
        )
    }

    /// The live events after `last`. Events at or before `last` are dropped; after a lag the log
    /// is read again for the missed events.
    fn live_replay(
        self: Arc<Self>,
        key: Arc<str>,
        receiver: broadcast::Receiver<Event>,
        last: u64,
    ) -> impl Stream<Item = Event> + Send + 'static {
        stream::unfold(
            (self, key, receiver, last),
            |(events, key, mut receiver, last)| async move {
                let batch = match receiver.recv().await {
                    Ok(event) => vec![event],
                    Err(RecvError::Lagged(_)) => events.logs.with(&*key, |logs| {
                        logs.get(&*key)
                            .map(|log| events_after(log, Some(last)))
                            .unwrap_or_default()
                    }),
                    Err(RecvError::Closed) => return None,
                };
                let batch: Vec<Event> = batch.into_iter().filter(|(seq, _)| *seq > last).collect();
                let last = batch.last().map_or(last, |(seq, _)| *seq);
                Some((stream::iter(batch), (events, key, receiver, last)))
            },
        )
        .flatten()
    }

    /// Drops the logs of one shard that are no longer kept.
    pub(super) fn sweep(&self, shard: usize, now: Instant) {
        let dropped: Vec<(Box<str>, StreamLog)> = self.logs.with_shard(shard, |logs| {
            logs.extract_if(|_, log| !keep_stream(log, now)).collect()
        });
        drop(dropped);
    }

    /// The session a stream is bound to, and whether its call runs.
    #[cfg(test)]
    pub(super) fn binding(&self, stream: &str) -> Option<(Option<McpSessionId>, bool)> {
        self.logs.with(stream, |logs| {
            logs.get(stream).map(|log| (log.session, log.running))
        })
    }

    #[cfg(test)]
    pub(super) fn is_kept(&self, stream: &str) -> bool {
        self.logs.with(stream, |logs| logs.contains_key(stream))
    }
}

#[async_trait::async_trait]
impl EventStore for ReplicaEventStore {
    async fn store_event(
        &self,
        stream_id: &str,
        event: &ServerSseMessage,
    ) -> Result<EventId, EventStoreError> {
        let now = Instant::now();
        let stored = self.with_log(stream_id, |log| append_event(log, event.clone(), now));
        let Stored {
            seq,
            event,
            live,
            evicted,
        } = stored.ok_or(StreamCancelled)?;
        if let Some(live) = live {
            // No live replay is subscribed most of the time; then there is nobody to tell.
            let _ = live.send((seq, event));
        }
        drop(evicted);
        Ok(event_id(stream_id, seq))
    }

    /// The session layer serves every resume itself, after checking that the stream belongs to
    /// the requesting session, so rmcp never asks.
    async fn replay_events_after(
        &self,
        _last_event_id: &str,
    ) -> Result<EventStream, EventStoreError> {
        Ok(Box::pin(stream::empty()))
    }
}

/// The shard visited on sweep tick `tick`: one shard per tick, all shards in turn.
pub(super) fn sweep_step(tick: usize) -> usize {
    tick % super::sharded::SHARDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ErrorData, JsonRpcError, JsonRpcNotification, ServerNotification};
    use test_r::test;

    use crate::mcp::session::test_ids::{other_session, session};

    fn notification(n: u32) -> ServerSseMessage {
        ServerSseMessage::from_message(ServerJsonRpcMessage::Notification(JsonRpcNotification {
            jsonrpc: Default::default(),
            notification: ServerNotification::CustomNotification(
                rmcp::model::CustomNotification::new(format!("test/{n}"), None),
            ),
        }))
    }

    fn terminal() -> ServerSseMessage {
        ServerSseMessage::from_message(ServerJsonRpcMessage::Error(JsonRpcError::new(
            Some(rmcp::model::RequestId::Number(1)),
            ErrorData::internal_error("done", None),
        )))
    }

    fn method(event: &ServerSseMessage) -> String {
        match event.message.as_deref() {
            Some(ServerJsonRpcMessage::Notification(notification)) => {
                match &notification.notification {
                    ServerNotification::CustomNotification(custom) => custom.method.clone(),
                    _ => "other".into(),
                }
            }
            Some(ServerJsonRpcMessage::Error(_)) => "terminal".into(),
            _ => "none".into(),
        }
    }

    #[test]
    async fn retained_keeps_the_last_16_events_and_completes_on_a_terminal() {
        let now = Instant::now();
        let mut log = StreamLog::new(now);
        let evicted: Vec<Option<u64>> = (0..17)
            .map(|n| {
                append_event(&mut log, notification(n), now)
                    .unwrap()
                    .evicted
                    .map(|(seq, _)| seq)
            })
            .collect();
        assert_eq!(evicted[15], None);
        assert_eq!(evicted[16], Some(0));
        assert_eq!(log.events.len(), 16);
        assert_eq!(log.events.front().map(|(seq, _)| *seq), Some(1));
        assert!(log.live.is_some());
        assert!(log.completed_at.is_none());

        let stored = append_event(&mut log, terminal(), now).unwrap();
        assert!(
            stored.live.is_some(),
            "the terminal is sent to live replays"
        );
        assert!(log.live.is_none(), "live replays end after the terminal");
        assert_eq!(log.completed_at, Some(now));
    }

    #[test]
    async fn a_cancelled_log_refuses_events_and_ends_live_replays() {
        let now = Instant::now();
        let mut log = StreamLog::new(now);
        append_event(&mut log, notification(0), now).unwrap();
        let live = refuse_further_events(&mut log, now);
        assert!(live.is_some());
        assert!(log.live.is_none());
        assert_eq!(log.completed_at, Some(now));
        assert!(append_event(&mut log, terminal(), now).is_none());
        assert_eq!(log.events.len(), 1);
    }

    #[test]
    async fn split_replay_is_live_only_while_the_stream_runs() {
        let now = Instant::now();
        let mut log = StreamLog::new(now);
        (0..3).for_each(|n| {
            append_event(&mut log, notification(n), now).unwrap();
        });
        let (stored, live) = split_replay(&log, 0);
        assert_eq!(
            stored.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(live.is_some());
        append_event(&mut log, terminal(), now).unwrap();
        let (stored, live) = split_replay(&log, 2);
        assert_eq!(stored.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(), [3]);
        assert!(live.is_none());
        let mut cancelled_log = StreamLog::new(now);
        refuse_further_events(&mut cancelled_log, now);
        assert!(split_replay(&cancelled_log, 0).1.is_none());
    }

    #[test]
    async fn events_after_skips_what_was_delivered() {
        let now = Instant::now();
        let mut log = StreamLog::new(now);
        (0..4).for_each(|n| {
            append_event(&mut log, notification(n), now).unwrap();
        });
        assert_eq!(
            events_after(&log, Some(1))
                .iter()
                .map(|(seq, _)| *seq)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(events_after(&log, None).len(), 4);
    }

    /// Runs `test` on a runtime whose clock is paused.
    fn paused(test: impl Future<Output = ()>) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(test)
    }

    #[test]
    fn keep_stream_table() {
        let start = Instant::now();
        let hour = Duration::from_secs(3600);
        let log_at = |running: bool, completed: Option<Duration>| {
            let mut log = StreamLog::new(start);
            log.running = running;
            log.completed_at = completed.map(|after| start + after);
            log
        };
        [
            ("running, old", log_at(true, None), start + 2 * hour, true),
            (
                "completed 59 s ago",
                log_at(false, Some(Duration::ZERO)),
                start + Duration::from_secs(59),
                true,
            ),
            (
                "completed 60 s ago",
                log_at(false, Some(Duration::ZERO)),
                start + Duration::from_secs(60),
                false,
            ),
            (
                "idle under an hour",
                log_at(false, None),
                start + hour - Duration::from_secs(1),
                true,
            ),
            ("idle an hour", log_at(false, None), start + hour, false),
        ]
        .into_iter()
        .for_each(|(row, log, now, expected)| {
            assert_eq!(keep_stream(&log, now), expected, "{row}")
        });
    }

    #[test]
    fn a_call_that_just_ended_keeps_its_old_log() {
        paused(async {
            let store = ReplicaEventStore::new();
            let id = store.store_event("s", &notification(0)).await.unwrap();
            assert_eq!(id, "s/0");
            store.bind_call("s", session());
            tokio::time::advance(Duration::from_secs(2 * 3600)).await;
            store.call_ended("s");
            store.sweep(store_shard(&store, "s"), Instant::now());
            assert!(
                store.is_kept("s"),
                "the hour counts from the end of the call"
            );
        })
    }

    fn store_shard(store: &ReplicaEventStore, stream: &str) -> usize {
        (0..super::super::sharded::SHARDS)
            .find(|shard| {
                store
                    .logs
                    .with_shard(*shard, |logs| logs.contains_key(stream))
            })
            .unwrap()
    }

    #[test]
    async fn store_event_creates_the_log_of_an_unknown_stream() {
        let store = ReplicaEventStore::new();
        assert_eq!(
            store.store_event("new", &notification(0)).await.unwrap(),
            "new/0"
        );
        assert_eq!(
            store.store_event("new", &notification(1)).await.unwrap(),
            "new/1"
        );
        store.mark_cancelled("new");
        assert!(store.store_event("new", &terminal()).await.is_err());
    }

    #[test]
    async fn replay_serves_only_the_bound_session_and_follows_a_running_stream() {
        let store = Arc::new(ReplicaEventStore::new());
        let priming = store
            .store_event("s", &ServerSseMessage::retry(Duration::from_secs(3)))
            .await
            .unwrap();
        store.store_event("s", &notification(1)).await.unwrap();
        assert!(store.replay(&priming, session()).is_none(), "not bound");
        store.bind_call("s", session());
        let other = other_session();
        assert!(store.replay(&priming, other).is_none(), "another session");
        assert!(store.replay("unknown/0", session()).is_none());
        assert!(store.replay("no-position", session()).is_none());

        let replay = store.replay(&priming, session()).unwrap();
        store.store_event("s", &notification(2)).await.unwrap();
        store.store_event("s", &terminal()).await.unwrap();
        let replayed: Vec<(String, String)> =
            tokio::time::timeout(Duration::from_secs(5), replay.collect::<Vec<_>>())
                .await
                .unwrap()
                .into_iter()
                .map(|(id, event)| (id, method(&event)))
                .collect();
        assert_eq!(
            replayed,
            [
                ("s/1".to_string(), "test/1".to_string()),
                ("s/2".to_string(), "test/2".to_string()),
                ("s/3".to_string(), "terminal".to_string()),
            ]
        );
        super::super::sharded::lock_check::assert_no_violation();
    }

    #[test]
    fn sweep_step_visits_one_shard_per_tick() {
        let shards = super::super::sharded::SHARDS;
        assert_eq!(sweep_step(0), 0);
        assert_eq!(sweep_step(1), 1);
        assert_eq!(sweep_step(shards - 1), shards - 1);
        assert_eq!(sweep_step(shards), 0);
    }

    #[test]
    fn sweep_drops_only_streams_that_are_not_kept() {
        paused(async {
            let store = ReplicaEventStore::new();
            store.store_event("done", &terminal()).await.unwrap();
            store
                .store_event("running", &notification(0))
                .await
                .unwrap();
            store.bind_call("running", session());
            tokio::time::advance(Duration::from_secs(61)).await;
            let now = Instant::now();
            (0..super::super::sharded::SHARDS).for_each(|shard| store.sweep(shard, now));
            assert!(!store.is_kept("done"));
            assert!(store.is_kept("running"));
        })
    }
}
