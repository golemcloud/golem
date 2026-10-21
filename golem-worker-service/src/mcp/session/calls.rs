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

//! The calls of each session that run on this replica, so that `notifications/cancelled` and
//! DELETE can cancel them. The table holds plain values only; nothing in it refers to a request.

use super::event_store::ReplicaEventStore;
use super::sharded::Sharded;
use super::store::McpSessionId;
use rmcp::model::RequestId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;

/// Identifies one call in the table, unique per replica. Two calls with the same JSON-RPC
/// request ID have different entry IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct EntryId(u64);

#[derive(Debug)]
pub(super) struct RunningCall {
    request_id: RequestId,
    /// Cancelled when the call is cancelled. The response body ends on it, and the call's
    /// handler stops on it.
    token: CancellationToken,
    /// The call's response stream, once the layer has seen its first event.
    stream: Option<Arc<str>>,
    cancelled: bool,
}

type SessionCalls = HashMap<EntryId, RunningCall>;
type Table = HashMap<McpSessionId, SessionCalls>;

/// What [`RunningCalls::bind`] found for the call.
#[derive(Debug, PartialEq)]
pub(super) enum BindOutcome {
    Bound,
    /// The call has ended: its entry is gone.
    Gone,
    /// The call was cancelled before its stream was known.
    CancelledBefore,
}

/// What a cancel copies out of the table for one call: its token, and its response stream when
/// the stream is known.
#[derive(Debug)]
pub(super) struct CancelledCall {
    pub(super) token: CancellationToken,
    pub(super) stream: Option<Arc<str>>,
}

/// The entries removed for one ended call: the call, and its session's map when it was the
/// session's last call. Both are dropped by the caller after the shard lock is released.
type Removed = (Option<RunningCall>, Option<SessionCalls>);

fn insert_call(table: &mut Table, session: McpSessionId, entry: EntryId, call: RunningCall) {
    table.entry(session).or_default().insert(entry, call);
}

fn note_stream(
    table: &mut Table,
    session: McpSessionId,
    entry: EntryId,
    stream: Arc<str>,
) -> BindOutcome {
    match table
        .get_mut(&session)
        .and_then(|calls| calls.get_mut(&entry))
    {
        None => BindOutcome::Gone,
        Some(call) => {
            call.stream = Some(stream);
            if call.cancelled {
                BindOutcome::CancelledBefore
            } else {
                BindOutcome::Bound
            }
        }
    }
}

fn take_call(table: &mut Table, session: McpSessionId, entry: EntryId) -> Removed {
    let call = table
        .get_mut(&session)
        .and_then(|calls| calls.remove(&entry));
    let empty = table
        .get(&session)
        .is_some_and(HashMap::is_empty)
        .then(|| table.remove(&session))
        .flatten();
    (call, empty)
}

/// Marks the session's calls with `request_id` (every call when `None`) cancelled and copies
/// their tokens and streams.
fn mark_calls_cancelled(
    table: &mut Table,
    session: McpSessionId,
    request_id: Option<&RequestId>,
) -> Vec<CancelledCall> {
    table
        .get_mut(&session)
        .map(|calls| {
            calls
                .values_mut()
                .filter(|call| request_id.is_none_or(|id| *id == call.request_id))
                .map(|call| {
                    call.cancelled = true;
                    CancelledCall {
                        token: call.token.clone(),
                        stream: call.stream.clone(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) struct RunningCalls {
    table: Sharded<McpSessionId, SessionCalls>,
    next: AtomicU64,
    /// Test builds only: the number of calls in the table, so tests can wait for calls to end.
    #[cfg(test)]
    size: tokio::sync::watch::Sender<usize>,
}

impl RunningCalls {
    pub(super) fn new() -> Self {
        Self {
            table: Sharded::new(),
            next: AtomicU64::new(0),
            #[cfg(test)]
            size: tokio::sync::watch::Sender::new(0),
        }
    }

    /// Adds a call of an admitted session. The returned guard removes it when the request ends.
    pub(super) fn add(
        self: &Arc<Self>,
        events: &Arc<ReplicaEventStore>,
        session: McpSessionId,
        request_id: RequestId,
    ) -> (CancellationToken, RunningCallGuard) {
        let entry = EntryId(self.next.fetch_add(1, Ordering::Relaxed));
        let token = CancellationToken::new();
        let call = RunningCall {
            request_id,
            token: token.clone(),
            stream: None,
            cancelled: false,
        };
        self.table
            .with(&session, |table| insert_call(table, session, entry, call));
        #[cfg(test)]
        self.size.send_modify(|size| *size += 1);
        let guard = RunningCallGuard {
            calls: self.clone(),
            events: events.clone(),
            session,
            entry,
        };
        (token, guard)
    }

    /// Notes the call's response stream.
    pub(super) fn bind(&self, session: McpSessionId, entry: EntryId, stream: &str) -> BindOutcome {
        let stream = Arc::from(stream);
        self.table
            .with(&session, |table| note_stream(table, session, entry, stream))
    }

    /// Marks every call of the session with this request ID cancelled, duplicates included, and
    /// returns their tokens and streams.
    pub(super) fn cancel(
        &self,
        session: McpSessionId,
        request_id: &RequestId,
    ) -> Vec<CancelledCall> {
        self.table.with(&session, |table| {
            mark_calls_cancelled(table, session, Some(request_id))
        })
    }

    /// Marks every call of the session cancelled and returns their tokens and streams.
    pub(super) fn cancel_session(&self, session: McpSessionId) -> Vec<CancelledCall> {
        self.table
            .with(&session, |table| mark_calls_cancelled(table, session, None))
    }

    fn remove(&self, session: McpSessionId, entry: EntryId) -> Removed {
        let removed = self
            .table
            .with(&session, |table| take_call(table, session, entry));
        #[cfg(test)]
        if removed.0.is_some() {
            self.size.send_modify(|size| *size -= 1);
        }
        removed
    }

    /// Waits until no call of any session is in the table.
    #[cfg(test)]
    pub(super) async fn wait_until_empty(&self) {
        let mut size = self.size.subscribe();
        let _ = size.wait_for(|size| *size == 0).await;
    }

    /// The number of calls of every session in the table.
    #[cfg(test)]
    pub(super) fn total(&self) -> usize {
        *self.size.borrow()
    }

    #[cfg(test)]
    pub(super) fn count(&self, session: McpSessionId) -> usize {
        self.table.with(&session, |table| {
            table.get(&session).map_or(0, HashMap::len)
        })
    }
}

/// Owned by one request of a session. When the request ends, however it ends, the call leaves
/// the table and its stream stops counting as running. No table holds a guard.
pub(super) struct RunningCallGuard {
    calls: Arc<RunningCalls>,
    events: Arc<ReplicaEventStore>,
    session: McpSessionId,
    entry: EntryId,
}

impl RunningCallGuard {
    pub(super) fn entry(&self) -> EntryId {
        self.entry
    }
}

impl Drop for RunningCallGuard {
    fn drop(&mut self) {
        let (call, _empty) = self.calls.remove(self.session, self.entry);
        if let Some(stream) = call.as_ref().and_then(|call| call.stream.as_deref()) {
            self.events.call_ended(stream);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    use crate::mcp::session::test_ids::session;

    fn call(request_id: i64) -> RunningCall {
        RunningCall {
            request_id: RequestId::Number(request_id),
            token: CancellationToken::new(),
            stream: None,
            cancelled: false,
        }
    }

    #[test]
    fn cancel_reaches_every_call_with_the_request_id() {
        let mut table = Table::new();
        insert_call(&mut table, session(), EntryId(1), call(7));
        insert_call(&mut table, session(), EntryId(2), call(7));
        insert_call(&mut table, session(), EntryId(3), call(8));
        assert_eq!(
            note_stream(&mut table, session(), EntryId(2), "s2".into()),
            BindOutcome::Bound
        );

        let mut copied = mark_calls_cancelled(&mut table, session(), Some(&RequestId::Number(7)));
        copied.sort_by(|left, right| left.stream.cmp(&right.stream));
        assert_eq!(copied.len(), 2);
        assert_eq!(copied[0].stream, None);
        assert_eq!(copied[1].stream.as_deref(), Some("s2"));
        let session_calls = &table[&session()];
        assert!(session_calls[&EntryId(1)].cancelled);
        assert!(session_calls[&EntryId(2)].cancelled);
        assert!(!session_calls[&EntryId(3)].cancelled);
    }

    #[test]
    fn cancel_session_reaches_every_call() {
        let mut table = Table::new();
        insert_call(&mut table, session(), EntryId(1), call(7));
        insert_call(&mut table, session(), EntryId(2), call(7));
        insert_call(&mut table, session(), EntryId(3), call(8));
        assert_eq!(mark_calls_cancelled(&mut table, session(), None).len(), 3);
        assert!(table[&session()].values().all(|call| call.cancelled));
    }

    #[test]
    fn remove_takes_only_its_own_entry() {
        let mut table = Table::new();
        insert_call(&mut table, session(), EntryId(1), call(7));
        insert_call(&mut table, session(), EntryId(2), call(7));

        let (first, empty) = take_call(&mut table, session(), EntryId(1));
        assert_eq!(
            first.map(|call| call.request_id),
            Some(RequestId::Number(7))
        );
        assert!(empty.is_none());
        assert_eq!(
            mark_calls_cancelled(&mut table, session(), Some(&RequestId::Number(7))).len(),
            1
        );

        let (second, empty) = take_call(&mut table, session(), EntryId(2));
        assert!(second.is_some());
        assert_eq!(empty.map(|calls| calls.len()), Some(0));
        assert!(table.is_empty());
        assert!(matches!(
            take_call(&mut table, session(), EntryId(2)),
            (None, None)
        ));
    }

    #[test]
    fn bind_answers_gone_and_cancelled_before() {
        let mut table = Table::new();
        assert_eq!(
            note_stream(&mut table, session(), EntryId(1), "s".into()),
            BindOutcome::Gone
        );
        insert_call(&mut table, session(), EntryId(1), call(7));
        mark_calls_cancelled(&mut table, session(), None);
        assert_eq!(
            note_stream(&mut table, session(), EntryId(1), "s".into()),
            BindOutcome::CancelledBefore
        );
    }

    #[test]
    fn add_gives_fresh_entries_and_the_guard_removes_its_own() {
        let calls = Arc::new(RunningCalls::new());
        let events = Arc::new(ReplicaEventStore::new());
        let (first_token, first) = calls.add(&events, session(), RequestId::Number(7));
        let (second_token, second) = calls.add(&events, session(), RequestId::Number(7));
        assert_ne!(first.entry(), second.entry());
        assert_eq!(calls.count(session()), 2);
        drop(first);
        assert_eq!(calls.count(session()), 1);
        let copied = calls.cancel(session(), &RequestId::Number(7));
        assert_eq!(copied.len(), 1);
        copied.into_iter().for_each(|call| call.token.cancel());
        assert!(second_token.is_cancelled());
        assert!(!first_token.is_cancelled());
        drop(second);
        assert_eq!(calls.count(session()), 0);
        super::super::sharded::lock_check::assert_no_violation();
    }
}
