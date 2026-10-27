mod attachments;
mod commit;
mod io;
mod metadata;
mod reads;
mod retained;
mod selection;
mod settling;

pub(super) use commit::is_auto_skippable_hint;
use reads::is_active_stream_observation;
use retained::{cancelled_before_delivery, custom_subtree_entry_is_drainable};
pub(super) use selection::{
    head_owner, scope_entry_owner, start_parent, terminal_kind, terminal_start_index,
};
#[cfg(test)]
pub(super) use selection::{missing_start_waits_for, nearest_active_body};
pub(super) use settling::historical_reconstruction_owner_failure;

use super::claims::{AdmissionRefusal, RecordedStart, StartClaim, matching_admission_refusal};
use super::{
    AbandonedStarts, AgentInvocationResult, AgentInvocationStartedEntry, Arc, AtomicBool,
    AtomicOplogIndex, AtomicU8, BlockedOn, CHUNK_SIZE, CompletionMarker, CompletionMarkers,
    ConcurrentReplayResolver, CursorState, DeletedRegions, DurableInvocationId, EntityAttribution,
    ForkResult, Future, GuestReadOrder, HashMap, HashSet, Hasher, HostFunctionName, HostResponse,
    HostResponseGolemApiFork, InvocationContextStack, LivePublicationOutcome, LogLevel,
    MetroHash128, MissingStart, Mutex, MutexGuard, Notify, Oplog, OplogEntry,
    OplogEntryLookupResult, OplogIndex, OplogOps, OplogRegion, OplogScopeProjection, Ordering,
    OwnedAgentId, PublishedPosition, QuotaClass, ReplayCallHandle, ReplayCursor, ReplayEvent,
    ReplayState, ReplayToLiveOutcome, ReplayToLiveRole, ReplayTransitionPhase, Resolution,
    RetainedStart, RetainedStartCounts, StartClaimAttempt, StartsByInvocationId,
    StreamObservationPositions, ToolAttachmentReconstruction, TransactionMarkers, VecDeque,
    WorkerExecutorError, debug, warn,
};
#[cfg(any(test, feature = "test-utils"))]
use super::{CardId, StoredCard};
use crate::durable_host::PositionalRead;
use crate::durable_host::tool::operation::OwnerToolOperation;
use crate::services::oplog::UnreadableOplog;
use golem_common::model::oplog::payload::types::{
    SerializableToolAdmissionRefusal, SerializableToolPositionalRequest,
};

type PositionalAdmission = (OwnerToolOperation, SerializableToolPositionalRequest);

/// An in-progress cursor-advance transaction. Holds [`ReplayCursor::state`] for its whole lifetime
/// and is the only type permitted to advance the physical reader. Retained claims can separately
/// publish their claimed position in `last_replayed_non_hint_index`. Its methods may `await` oplog
/// reads / payload downloads while the lock is held (exactly as the old marker lock did), but they
/// never `await` a resolver receiver and never call a `ReplayState` method that re-acquires the
/// lock. It accumulates whether cursor progress should be signalled; the public entry point notifies
/// (via [`ReplayCursor::finish_tx`]) after the guard is dropped.
pub(super) struct CursorTx<'a> {
    pub(super) cursor: &'a ReplayCursor,
    pub(super) st: MutexGuard<'a, CursorState>,
    /// Normally released with the transaction. Consuming a `CompletionDelivered` marker transfers
    /// it to the matching [`ReplayDeliveryBarrier`] so no later transaction can advance first.
    advance_gate: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// Set when this transaction's positional read parked at a `CompletionDelivered` marker.
    /// Optional readers use it to distinguish that global barrier from an ordinary predicate
    /// mismatch, which may be returned to their caller immediately.
    pub(super) blocked_on_completion_delivery: bool,
    /// Set when this transaction claimed a retained `Start` whose terminal the cursor had already
    /// committed: the claim was resolved on the spot, so no resolver awaiter remains for it.
    resolved_retained_claim: Option<OplogIndex>,
    /// Set while this transaction commits a retained `Start` or its attached terminal: the replay
    /// events recorded meanwhile (by the entry itself and the hints skipped after it) are deferred
    /// to that `Start` ([`RetainedStart::deferred_events`]) instead of being published. Holds the
    /// retained `Start` index and the index of the entry currently being committed past.
    retaining_for: Option<(OplogIndex, OplogIndex)>,
    notify_progress: bool,
}

/// Retained-record bookkeeping only. This capability cannot read or commit the physical head,
/// acquire an advance gate, or publish live. Lookahead for resolver prefetch is non-consuming.
pub(super) struct RetainedClaims<'a> {
    pub(super) cursor: &'a ReplayCursor,
    pub(super) st: &'a mut CursorState,
    resolved_retained_claim: &'a mut Option<OplogIndex>,
    notify_progress: &'a mut bool,
}

/// What a cursor read does with an unclaimed durable-call `Start` its predicate did not accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnclaimedStarts {
    /// Commit past the `Start` and retain it for its owner's claim (positional consumers, terminal
    /// drains, tail waits: readers that must not park on another operation's record).
    Retain,
    /// Leave the `Start` at the head and return `None` (a claim's head fast path: the head is
    /// simply not this claim's `Start`).
    Leave,
}

/// Which unconditional positional reader parked, for [`CursorTx::check_parked_positional_read`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PositionalReader {
    /// A Store's ordinary positional read while its guest replays.
    Ordinary,
    /// The primary Store's walk to `AgentInvocationFinished` after the replayed guest produced
    /// its invocation result.
    InvocationBoundary,
}

/// The Store whose replay consumes an entry left at the cursor head, as the entry itself records
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HeadOwner {
    /// The entry does not record which Store appended it. A top-level `Start` is such an entry.
    Unattributed,
    /// The primary agent Store appended the entry.
    Agent,
    /// The entity body whose invocation `Start` is at this index appended the entry.
    EntityBody(OplogIndex),
    /// A nested `Start`: the durable call, scope or entity invocation whose `Start` is at this
    /// index owns it.
    ParentStart(OplogIndex),
}

/// What [`CursorTx::decide_missing_start`] does with the entry at the cursor head.
enum SiblingRecord {
    /// A `Start` that a concurrent guest call recorded: the cursor retains it for its owner.
    Retain(OplogIndex, OplogEntry),
    /// A record of the body of a retained entity invocation, which that body consumes once its
    /// invocation is claimed.
    BodyOfRetainedInvocation,
    /// Not a record of a concurrent guest call.
    None,
}

/// What the cursor knows about an entry that a scan passes at or after the cursor head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HeadDrainFacts {
    /// The entry is a `Start` that a claim already took ahead of the cursor.
    pub(super) claimed_ahead: bool,
    /// The entry is an `End` or `Cancelled` that a registered awaiter owns.
    pub(super) awaited_terminal: bool,
    /// A native guest read must acknowledge this active observation before advancement.
    pub(super) guest_observation: bool,
}
