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

use super::*;

/// A handle to an external blob upload that [`Oplog::add_start_with_reserved_raw_payload`] started
/// (spawned) but which may not have finished yet.
///
/// Cloneable so the same upload can be awaited by both the call that initiated it (before it
/// appends its `End`) and the commit barrier in the leaf oplog's `append` (before any entry
/// referencing the blob is persisted to indexed storage). It is `Send`, so a durable-call start can
/// return it and hold it across awaits. A `None` inner means the payload was stored inline, or
/// eagerly uploaded and already durable, so there is nothing to wait for.
#[derive(Clone)]
pub struct PendingUpload {
    inner: Option<Shared<BoxFuture<'static, Result<(), String>>>>,
}

impl PendingUpload {
    /// A payload that is already durable (inline, or an eager upload): waiting is a no-op.
    pub fn already_durable() -> Self {
        Self { inner: None }
    }

    /// A payload whose blob upload is in flight on the given shared future.
    pub fn spawned(upload: Shared<BoxFuture<'static, Result<(), String>>>) -> Self {
        Self {
            inner: Some(upload),
        }
    }

    /// Waits for the blob upload to finish, returning its result (`Ok(())` if there was nothing to
    /// wait for).
    pub async fn wait(&self) -> Result<(), String> {
        match &self.inner {
            Some(upload) => upload.clone().await,
            None => Ok(()),
        }
    }
}

impl Debug for PendingUpload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingUpload")
            .field("pending", &self.inner.is_some())
            .finish()
    }
}

/// A zero-sized, `!Send` token carried inside [`ReservedPayload`] to protect the leaf oplog's
/// `Start`-ordering critical section at compile time.
///
/// [`Oplog::add_start_with_reserved_raw_payload`] must, within a single non-yielding step of the
/// leaf oplog's actor job handler, reserve the request payload, build the `Start`, and assign its
/// index (`push`) with **no `.await` in between** — otherwise a concurrent call's `Start` could
/// interleave and break initiation-order determinism. The leaf implementation holds this guard
/// across that whole window. Because it is
/// `!Send`, holding it across an `.await` makes the enclosing (`async_trait`, hence `Send`-bound)
/// future fail to compile, so a future refactor that turns one of those synchronous steps into an
/// awaited one is rejected by the compiler instead of silently reordering. Never add a manual
/// `Send`/`Sync` impl.
#[must_use = "the reserve guard must be held until the Start is pushed, then dropped"]
pub struct ReserveGuard {
    _not_send: PhantomData<*const ()>,
}

/// The result of the primary oplog's payload reservation: a payload reference whose (possibly
/// large) blob upload has been *started* but not awaited, the [`PendingUpload`] tracking that
/// upload, and a [`ReserveGuard`] guarding the no-`.await` window up to the `Start` `push`.
#[must_use = "a reserved payload must be turned into a Start within the same non-yielding actor step"]
pub struct ReservedPayload {
    pub raw: RawOplogPayload,
    pub pending: PendingUpload,
    pub guard: ReserveGuard,
}

impl ReservedPayload {
    /// Builds a reserve result. Called by the leaf oplog from inside its actor task.
    pub fn new(raw: RawOplogPayload, pending: PendingUpload) -> Self {
        Self {
            raw,
            pending,
            guard: ReserveGuard {
                _not_send: PhantomData,
            },
        }
    }
}

/// The result of [`Oplog::add_start_with_reserved_raw_payload`]: the appended `Start`, its assigned
/// index, and the [`PendingUpload`] tracking its (possibly deferred) request-payload blob upload.
#[must_use = "the pending upload must be awaited before the matching End/Cancelled is appended"]
pub struct OrderedOplogStart {
    /// The index assigned to the appended `Start` entry.
    pub index: OplogIndex,
    /// The `Start` entry exactly as appended. Wrapper layers that mirror the buffered entries (e.g.
    /// the plugin-forwarding oplog) need it because the entry is built deep in the leaf from the
    /// reserved payload reference.
    pub entry: OplogEntry,
    /// Tracks the request payload's durable write. The caller must [`PendingUpload::wait`] on it
    /// before appending the matching `End`/`Cancelled`; the leaf oplog's `append` commit barrier is
    /// the backstop. A no-op for inline or eagerly-uploaded payloads.
    pub pending_upload: PendingUpload,
}

pub enum DurableStreamOplogRecord {
    Registered(Option<OplogIndex>, Box<StreamRegisteredRecord>),
    Items(Option<OplogIndex>, StreamItemsRecord),
    End(Option<OplogIndex>, StreamEndRecord),
    Cancel(Option<OplogIndex>, StreamCancelRecord),
    Session(Option<OplogIndex>, Box<StreamSessionRecord>),
    InlineEntry(OplogEntry),
}

impl DurableStreamOplogRecord {
    pub(super) fn serialize(&self) -> Result<Vec<u8>, String> {
        match self {
            Self::Registered(_, record) => serialize(record),
            Self::Items(_, record) => serialize(record),
            Self::End(_, record) => serialize(record),
            Self::Cancel(_, record) => serialize(record),
            Self::Session(_, record) => serialize(record),
            Self::InlineEntry(_) => Ok(Vec::new()),
        }
    }

    pub(super) fn into_entry(self, raw: RawOplogPayload) -> Result<OplogEntry, String> {
        match self {
            Self::Registered(entity_parent_start_index, record) => {
                Ok(OplogEntry::stream_registered(
                    entity_parent_start_index,
                    raw.into_payload_with_cache(Arc::from(record))?,
                    Some(DurableStreamEventSummary::Registered),
                ))
            }
            Self::Items(entity_parent_start_index, record) => {
                let summary = DurableStreamEventSummary::items(&record);
                Ok(OplogEntry::stream_items(
                    entity_parent_start_index,
                    raw.into_payload_with_cache(Arc::new(record))?,
                    Some(summary),
                ))
            }
            Self::End(entity_parent_start_index, record) => {
                let summary = DurableStreamEventSummary::end(&record);
                Ok(OplogEntry::stream_end(
                    entity_parent_start_index,
                    raw.into_payload_with_cache(Arc::new(record))?,
                    Some(summary),
                ))
            }
            Self::Cancel(entity_parent_start_index, record) => Ok(OplogEntry::stream_cancel(
                entity_parent_start_index,
                raw.into_payload_with_cache(Arc::new(record))?,
                Some(DurableStreamEventSummary::Cancelled),
            )),
            Self::Session(entity_parent_start_index, record) => {
                let summary = DurableStreamEventSummary::session(&record);
                Ok(OplogEntry::stream_session(
                    entity_parent_start_index,
                    raw.into_payload_with_cache(Arc::from(record))?,
                    summary,
                ))
            }
            Self::InlineEntry(entry) => Ok(entry),
        }
    }

    pub fn into_inline_entry(self) -> OplogEntry {
        match self {
            Self::Registered(entity_parent_start_index, record) => OplogEntry::stream_registered(
                entity_parent_start_index,
                OplogPayload::Inline(record),
                Some(DurableStreamEventSummary::Registered),
            ),
            Self::Items(entity_parent_start_index, record) => {
                let summary = DurableStreamEventSummary::items(&record);
                OplogEntry::stream_items(
                    entity_parent_start_index,
                    OplogPayload::Inline(Box::new(record)),
                    Some(summary),
                )
            }
            Self::End(entity_parent_start_index, record) => {
                let summary = DurableStreamEventSummary::end(&record);
                OplogEntry::stream_end(
                    entity_parent_start_index,
                    OplogPayload::Inline(Box::new(record)),
                    Some(summary),
                )
            }
            Self::Cancel(entity_parent_start_index, record) => OplogEntry::stream_cancel(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record)),
                Some(DurableStreamEventSummary::Cancelled),
            ),
            Self::Session(entity_parent_start_index, record) => {
                let summary = DurableStreamEventSummary::session(&record);
                OplogEntry::stream_session(
                    entity_parent_start_index,
                    OplogPayload::Inline(record),
                    summary,
                )
            }
            Self::InlineEntry(entry) => entry,
        }
    }
}

pub type DurableStreamBatchBuilder =
    Box<dyn FnOnce(OplogIndex) -> Vec<DurableStreamOplogRecord> + Send>;

pub type ReservedRawStartBuilder =
    Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>;

pub type IndexedReservedStartBuilder =
    Box<dyn FnOnce(OplogIndex) -> Result<(Vec<u8>, ReservedRawStartBuilder), String> + Send>;
