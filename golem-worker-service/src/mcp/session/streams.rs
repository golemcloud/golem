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

//! The open streams of each session on this replica, GET streams and resumed response streams
//! together: at most 32 per session, the oldest closed when a new one opens. Only the GET streams
//! get the session's notifications. A stream ends when its entry is dropped.

use super::sharded::Sharded;
use super::store::McpSessionId;
use rmcp::model::ServerJsonRpcMessage;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use tokio::sync::{mpsc, oneshot};

/// The streams kept open per session on one replica.
pub(super) const MAX_STREAMS_PER_SESSION: usize = 32;

pub(super) type StreamSender = mpsc::Sender<ServerJsonRpcMessage>;

/// An open stream of a session. Dropping it closes the stream.
enum OpenStream {
    /// A GET stream, which gets the session's notifications.
    Get(StreamSender),
    /// A resumed response stream, which gets no notifications.
    Resumed(oneshot::Sender<Infallible>),
}

impl OpenStream {
    /// The sender of a GET stream; `None` for a resumed stream.
    fn get_sender(&self) -> Option<&StreamSender> {
        match self {
            OpenStream::Get(sender) => Some(sender),
            OpenStream::Resumed(_) => None,
        }
    }

    /// True when the stream's response has ended.
    fn is_closed(&self) -> bool {
        match self {
            OpenStream::Get(sender) => sender.is_closed(),
            OpenStream::Resumed(sender) => sender.is_closed(),
        }
    }
}

/// Takes the closed streams out of the open streams, adds `new`, and takes out the oldest ones
/// above the limit, to be closed. The removed streams are returned so that they are dropped after
/// the lock is released.
fn open_stream<S>(open: &mut VecDeque<S>, new: S, is_closed: impl Fn(&S) -> bool) -> Vec<S> {
    let (mut removed, kept): (Vec<S>, Vec<S>) =
        std::mem::take(open).into_iter().partition(is_closed);
    *open = kept.into();
    open.push_back(new);
    let excess = open.len().saturating_sub(MAX_STREAMS_PER_SESSION);
    removed.extend(open.drain(..excess));
    removed
}

/// Takes the closed streams out of every session of a shard, and the sessions left without
/// streams. Both are returned so that they are dropped after the lock is released.
fn take_closed<S>(
    sessions: &mut HashMap<McpSessionId, VecDeque<S>>,
    is_closed: impl Fn(&S) -> bool,
) -> (Vec<S>, Vec<(McpSessionId, VecDeque<S>)>) {
    let closed = sessions
        .values_mut()
        .flat_map(|open| {
            let (closed, kept): (Vec<S>, Vec<S>) =
                std::mem::take(open).into_iter().partition(&is_closed);
            *open = kept.into();
            closed
        })
        .collect();
    let empty = sessions.extract_if(|_, open| open.is_empty()).collect();
    (closed, empty)
}

pub(super) struct OpenStreams {
    sessions: Sharded<McpSessionId, VecDeque<OpenStream>>,
}

impl OpenStreams {
    pub(super) fn new() -> Self {
        Self {
            sessions: Sharded::new(),
        }
    }

    /// Opens a GET stream of the session; it receives the session's notifications until it is
    /// closed.
    pub(super) fn admit(&self, session: McpSessionId) -> mpsc::Receiver<ServerJsonRpcMessage> {
        let (sender, receiver) = mpsc::channel(1);
        self.open(session, OpenStream::Get(sender));
        receiver
    }

    /// Opens a resumed response stream of the session. The returned receiver completes when the
    /// stream is closed.
    pub(super) fn admit_resumed(&self, session: McpSessionId) -> oneshot::Receiver<Infallible> {
        let (sender, receiver) = oneshot::channel();
        self.open(session, OpenStream::Resumed(sender));
        receiver
    }

    fn open(&self, session: McpSessionId, stream: OpenStream) {
        let closed = self.sessions.with(&session, |sessions| {
            open_stream(
                sessions.entry(session).or_default(),
                stream,
                OpenStream::is_closed,
            )
        });
        drop(closed);
    }

    /// Sends `message` to every GET stream of the session that has room for it. A stream whose
    /// client does not keep up misses the message, as a notification is not resent.
    pub(super) fn notify(&self, session: McpSessionId, message: ServerJsonRpcMessage) {
        let senders: Vec<StreamSender> = self.sessions.with(&session, |sessions| {
            sessions
                .get(&session)
                .map(|open| {
                    open.iter()
                        .filter_map(OpenStream::get_sender)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        });
        senders.iter().for_each(|sender| {
            let _ = sender.try_send(message.clone());
        });
    }

    /// Closes every open stream of the session.
    pub(super) fn close_session(&self, session: McpSessionId) {
        let closed = self
            .sessions
            .with(&session, |sessions| sessions.remove(&session));
        drop(closed);
    }

    /// Forgets the streams of one shard whose client has gone.
    pub(super) fn sweep(&self, shard: usize) {
        let swept = self.sessions.with_shard(shard, |sessions| {
            take_closed(sessions, OpenStream::is_closed)
        });
        drop(swept);
    }

    #[cfg(test)]
    pub(super) fn count(&self, session: McpSessionId) -> usize {
        self.sessions.with(&session, |sessions| {
            sessions.get(&session).map_or(0, VecDeque::len)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    use crate::mcp::session::test_ids::{other_session, session};

    #[test]
    fn open_stream_closes_the_oldest_above_the_limit() {
        let mut open: VecDeque<i32> = (0..31).collect();
        assert!(open_stream(&mut open, 31, |_| false).is_empty());
        assert_eq!(open.len(), 32);
        let closed = open_stream(&mut open, 32, |_| false);
        assert_eq!(closed, [0]);
        assert_eq!(open.front(), Some(&1));
        assert_eq!(open.back(), Some(&32));
    }

    #[test]
    fn take_closed_takes_closed_streams_and_empty_sessions() {
        let mut sessions = HashMap::from([
            (session(), VecDeque::from([1, -1, 2])),
            (other_session(), VecDeque::from([-3])),
        ]);
        let (closed, empty) = take_closed(&mut sessions, |stream: &i32| *stream < 0);
        let mut closed = closed;
        closed.sort();
        assert_eq!(closed, [-3, -1]);
        assert_eq!(
            empty.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [other_session()]
        );
        assert_eq!(sessions[&session()], VecDeque::from([1, 2]));
    }

    #[test]
    async fn notify_reaches_every_stream_of_the_session_only() {
        let streams = OpenStreams::new();
        let mut first = streams.admit(session());
        let mut second = streams.admit(session());
        let mut other = streams.admit(other_session());
        let message = ServerJsonRpcMessage::notification(
            rmcp::model::ServerNotification::ToolListChangedNotification(
                rmcp::model::ToolListChangedNotification {
                    method: Default::default(),
                    extensions: Default::default(),
                },
            ),
        );
        streams.notify(session(), message);
        assert!(first.try_recv().is_ok());
        assert!(second.try_recv().is_ok());
        assert!(other.try_recv().is_err());
        super::super::sharded::lock_check::assert_no_violation();
    }

    #[test]
    async fn finished_streams_do_not_count_when_a_stream_opens() {
        let streams = OpenStreams::new();
        let mut live = streams.admit(session());
        let finished: Vec<_> = (0..31).map(|_| streams.admit_resumed(session())).collect();
        drop(finished);
        assert_eq!(streams.count(session()), 32, "no sweep has run");
        let _newest = streams.admit_resumed(session());
        assert!(
            live.try_recv()
                .is_err_and(|error| error == mpsc::error::TryRecvError::Empty),
            "the live GET stream stays open"
        );
        assert_eq!(streams.count(session()), 2);
        super::super::sharded::lock_check::assert_no_violation();
    }

    #[test]
    async fn the_33rd_stream_closes_the_oldest() {
        let streams = OpenStreams::new();
        let mut first = streams.admit(session());
        let mut rest: Vec<_> = (0..31).map(|_| streams.admit(session())).collect();
        assert_eq!(streams.count(session()), 32);
        let _newest = streams.admit(session());
        assert_eq!(streams.count(session()), 32);
        assert!(first.recv().await.is_none(), "the oldest stream ends");
        assert!(
            rest[0]
                .try_recv()
                .is_err_and(|error| error == mpsc::error::TryRecvError::Empty)
        );
        streams.close_session(session());
        assert!(rest[0].recv().await.is_none());
        super::super::sharded::lock_check::assert_no_violation();
    }
}
