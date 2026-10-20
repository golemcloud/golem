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

//! Per-worker read-only method result cache.
//!
//! - looked up at `Worker::invoke` and `Worker::invoke_and_await` (covers
//!   gRPC and wasm-rpc);
//! - on the Await path (`Worker::invoke_and_await`), concurrent first-time
//!   misses on the same `ReadOnlyCacheKey` *coalesce* through
//!   `golem_common::cache::Cache::get_or_insert_simple`: only the first
//!   caller runs the underlying invocation and populates the cache, every
//!   concurrent Await caller receives the same result;
//! - on the fire-and-forget path (`Worker::invoke`), misses do not block:
//!   each miss enqueues normally and a detached observer fills the cache on
//!   completion. The Await coalescer and the fire-and-forget observer use
//!   identical key/entry shapes, so an Await coalesce and a concurrent
//!   observer fill produce the same `ReadOnlyCacheEntry` and the race is
//!   benign;
//! - the key embeds the per-worker epoch and component revision, so mutations
//!   (and component updates / reverts) invalidate lazily on the next lookup;
//! - the epoch is bumped when a *mutating invocation successfully completes*
//!   (in [`DurableWorkerCtx::on_agent_invocation_success`]), NOT when it is
//!   merely enqueued. This is what lets a cached read-only value keep serving
//!   while a slow / queued mutation is in flight;
//! - the observer fills under the epoch captured at enqueue, never the epoch
//!   at completion, and `populate_read_only_cache` rechecks the live epoch
//!   before insert to drop populates raced by an intervening mutation;
//! - dropped together with the `Worker`.

use golem_common::model::AgentInvocation;
use golem_common::model::AgentInvocationOutput;
use golem_common::model::agent::{AgentTypeName, Principal};
use golem_common::model::component::ComponentRevision;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::schema::SchemaValue;
use golem_common::schema::agent::AgentMethodSchema;
use std::fmt::Debug;
use std::hash::Hash;
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub(crate) enum AdmissionSignal {
    Waiting,
    Immediate,
    Queued,
    Failure(golem_service_base::error::worker_executor::WorkerExecutorError),
}

#[derive(Debug, Clone)]
pub(crate) enum PendingReadOnlyCacheEntry {
    Immediate,
    Invocation(PendingReadOnlyInvocation),
}

#[derive(Debug, Clone)]
pub(crate) struct PendingReadOnlyInvocation {
    inner: std::sync::Arc<PendingReadOnlyInvocationInner>,
}

#[derive(Debug)]
struct PendingReadOnlyInvocationInner {
    admission: tokio::sync::watch::Sender<AdmissionSignal>,
}

impl PendingReadOnlyInvocation {
    pub fn new() -> Self {
        let (admission, _) = tokio::sync::watch::channel(AdmissionSignal::Waiting);
        Self {
            inner: std::sync::Arc::new(PendingReadOnlyInvocationInner { admission }),
        }
    }

    pub fn publish(&self, signal: AdmissionSignal) {
        self.inner.admission.send_if_modified(|current| {
            if matches!(current, AdmissionSignal::Waiting) {
                *current = signal;
                true
            } else {
                false
            }
        });
    }

    pub async fn admission(&self) -> AdmissionSignal {
        let mut receiver = self.inner.admission.subscribe();
        loop {
            let signal = receiver.borrow_and_update().clone();
            if !matches!(signal, AdmissionSignal::Waiting) {
                return signal;
            }
            if receiver.changed().await.is_err() {
                return AdmissionSignal::Failure(
                    golem_service_base::error::worker_executor::WorkerExecutorError::runtime(
                        "read-only admission owner disappeared",
                    ),
                );
            }
        }
    }
}

/// Identifies an entry in the per-worker read-only result cache.
///
/// `epoch` and `component_revision` are part of the key so mutations and
/// component updates lazily invalidate cached entries.
///
/// `principal_digest` is populated only when the method's
/// [`AgentMethodSchema::read_only`] has `uses_principal == true`.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ReadOnlyCacheKey {
    pub method_name: String,
    pub component_revision: ComponentRevision,
    pub epoch: u64,
    pub input_digest: [u8; 32],
    pub principal_digest: Option<[u8; 32]>,
}

/// A successfully cached read-only invocation output, plus the optional
/// wall-clock expiry derived from the method's `CachePolicy::Ttl`.
#[derive(Debug, Clone)]
pub struct ReadOnlyCacheEntry {
    pub output: AgentInvocationOutput,
    pub expires_at: Option<Instant>,
}

impl ReadOnlyCacheEntry {
    pub fn is_expired(&self, now: Instant) -> bool {
        match self.expires_at {
            Some(at) => now >= at,
            None => false,
        }
    }
}

/// How an invocation affects the read-only cache.
///
/// - `ReadOnly`: captures the current epoch for later cache fill.
/// - `Mutating`: epoch invalidation is deferred to *successful completion*,
///   so a queued/running mutation does not invalidate
///   the cache for the duration of its run.
/// - `UnknownAssumeMutating`: classification failed; treat as `Mutating`.
///
/// Updates and reverts still bump the epoch eagerly (see
/// `Worker::enqueue_update` and `Worker::revert`), because they describe a
/// state change that is effectively in flight regardless of how the next
/// invocation goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvocationEffect {
    ReadOnly,
    Mutating,
    UnknownAssumeMutating,
}

impl InvocationEffect {
    /// True if the effect represents a state-changing invocation. Today this
    /// is informational — the actual epoch bump happens on successful
    /// completion in `DurableWorkerCtx::on_agent_invocation_success`, not at
    /// enqueue.
    pub fn is_mutating(self) -> bool {
        match self {
            InvocationEffect::ReadOnly => false,
            InvocationEffect::Mutating | InvocationEffect::UnknownAssumeMutating => true,
        }
    }
}

/// Looks up the [`AgentMethodSchema`] for `method_name` on `agent_type` in the
/// already-loaded component metadata. Returns `None` if either is missing;
/// callers should fall back to the normal invocation path.
pub fn resolve_read_only_method(
    metadata: &ComponentMetadata,
    agent_type: &AgentTypeName,
    method_name: &str,
) -> Option<AgentMethodSchema> {
    let at = metadata.find_agent_type_by_name_ref(agent_type)?;
    at.methods.iter().find(|m| m.name == method_name).cloned()
}

/// Statically classifies an [`AgentInvocation`] for cache invalidation
/// purposes. Read-only agent methods are detected by inspecting the given
/// component metadata snapshot.
pub fn classify_invocation(
    metadata: Option<&ComponentMetadata>,
    agent_type: Option<&AgentTypeName>,
    invocation: &AgentInvocation,
) -> InvocationEffect {
    match invocation {
        AgentInvocation::AgentMethod { method_name, .. } => match (metadata, agent_type) {
            (Some(meta), Some(agent_type)) => {
                match resolve_read_only_method(meta, agent_type, method_name) {
                    Some(method) => {
                        if method.read_only.is_some() {
                            InvocationEffect::ReadOnly
                        } else {
                            InvocationEffect::Mutating
                        }
                    }
                    None => InvocationEffect::UnknownAssumeMutating,
                }
            }
            _ => InvocationEffect::UnknownAssumeMutating,
        },
        AgentInvocation::AgentInitialization { .. }
        | AgentInvocation::ExternalTool { .. }
        | AgentInvocation::ManualUpdate { .. }
        | AgentInvocation::LoadSnapshot { .. }
        | AgentInvocation::SaveSnapshot { .. }
        | AgentInvocation::ProcessOplogEntries { .. } => InvocationEffect::Mutating,
    }
}

/// Canonical byte encoding of a [`SchemaValue`] for the cache digest.
/// Uses `desert_rust`'s deterministic `BinaryCodec` serialization, so equal
/// inputs collide and tuple-order / multimodal-name differences do not.
pub fn canonicalize_schema_value(input: &SchemaValue) -> Vec<u8> {
    desert_rust::serialize_to_byte_vec(input).expect("SchemaValue serialization is infallible")
}

pub fn principal_bytes(principal: &Principal) -> Vec<u8> {
    desert_rust::serialize_to_byte_vec(principal).expect("Principal serialization is infallible")
}

pub fn digest_bytes(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Builds the cache key for a read-only invocation.
pub fn build_read_only_cache_key(
    method_name: &str,
    input: &SchemaValue,
    principal: Option<&Principal>,
    component_revision: ComponentRevision,
    epoch: u64,
) -> ReadOnlyCacheKey {
    let input_bytes = canonicalize_schema_value(input);
    let input_digest = digest_bytes(&input_bytes);
    let principal_digest = principal.map(|p| digest_bytes(&principal_bytes(p)));
    ReadOnlyCacheKey {
        method_name: method_name.to_string(),
        component_revision,
        epoch,
        input_digest,
        principal_digest,
    }
}

#[cfg(test)]
mod tests;

use crate::worker::*;

impl<Ctx: WorkerCtx> Worker<Ctx> {
    /// Extracts the read-only context for `invocation` by looking up the
    /// method's `read_only` config on the currently-loaded component
    /// metadata. Returns `None` for non-`AgentMethod` invocations and for
    /// methods that are not declared `#[read_only]`.
    pub(in crate::worker) fn read_only_context_for(
        &self,
        invocation: &AgentInvocation,
    ) -> Option<ReadOnlyContext> {
        let AgentInvocation::AgentMethod {
            method_name,
            input,
            principal,
            scope_card,
            ..
        } = invocation
        else {
            return None;
        };

        let snapshot = self.current_component.load();
        let component_revision = snapshot.revision;
        let metadata = &snapshot.metadata;
        let agent_type_opt = self.parsed_agent_id.as_ref().map(|p| p.agent_type.clone());

        let agent_type = agent_type_opt.as_ref()?;
        let method = read_only_cache::resolve_read_only_method(metadata, agent_type, method_name)?;
        let cfg = method.read_only.as_ref()?;

        Some(ReadOnlyContext {
            method_name: method_name.clone(),
            input: input.clone(),
            principal: principal.clone(),
            cfg: cfg.clone(),
            component_revision,
            cacheable: scope_card.is_none(),
        })
    }
    /// Bumps the read-only cache epoch, lazily invalidating all cached entries
    /// (the epoch is part of the cache key). Called from
    /// `DurableWorkerCtx::on_agent_invocation_success` immediately after a
    /// mutating invocation's `AgentInvocationFinished` is committed, so a
    /// cached read-only result keeps serving while the mutation is queued /
    /// running. Also called from
    /// `enqueue_update`/`revert` where the change is effectively in flight.
    pub(crate) fn bump_read_only_cache_epoch(&self) {
        self.read_only_cache_epoch.fetch_add(1, Ordering::SeqCst);
    }
    /// Classifies a just-completed `AgentMethod` invocation by `method_name`
    /// against the worker's in-memory component snapshot.
    ///
    /// Returns `true` for any invocation that should invalidate cached
    /// read-only results: a non-read-only method, an unknown method (safe
    /// default), or an `AgentMethod` on a worker with no `parsed_agent_id`.
    /// Returns `false` only when the method is explicitly `read_only`.
    ///
    /// Used by `DurableWorkerCtx::on_agent_invocation_success` to decide
    /// whether to bump the read-only cache epoch on successful completion
    pub fn agent_method_invalidates_read_only_cache(&self, method_name: &str) -> bool {
        let snapshot = self.current_component.load();
        let metadata = &snapshot.metadata;
        let Some(parsed) = self.parsed_agent_id.as_ref() else {
            return true;
        };
        match read_only_cache::resolve_read_only_method(metadata, &parsed.agent_type, method_name) {
            Some(method) => method.read_only.is_none(),
            None => true,
        }
    }
}

/// Inserts `output` into the cache under `epoch`, which must be the value
/// captured at enqueue (see
/// [`Worker::enqueue_worker_invocation_with_effect`]).
///
/// Performs a final epoch recheck against `read_only_cache_epoch` before
/// inserting and drops the populate if a mutating invocation has completed
/// in the meantime. This is the populate-time guard
/// for the "epoch is bumped on mutating completion, not enqueue" semantics.
///
/// Free function so the observer task does not pin the worker.
pub(in crate::worker) async fn populate_read_only_cache(
    cache: &golem_common::cache::Cache<
        read_only_cache::ReadOnlyCacheKey,
        read_only_cache::PendingReadOnlyCacheEntry,
        Arc<read_only_cache::ReadOnlyCacheEntry>,
        WorkerExecutorError,
    >,
    read_only_cache_epoch: &AtomicU64,
    ro: &ReadOnlyContext,
    epoch: u64,
    output: AgentInvocationOutput,
) {
    // Stale-populate guard: if a mutating invocation has completed (and bumped
    // the epoch) between the read-only enqueue and now, do not store this
    // result. It would otherwise sit under the pre-mutation epoch and be
    // unreachable, but storing it would still let a future epoch wrap hit it
    // (defensive).
    if read_only_cache_epoch.load(Ordering::SeqCst) != epoch {
        return;
    }

    let principal_ref = if ro.cfg.uses_principal {
        Some(&ro.principal)
    } else {
        None
    };
    let key = read_only_cache::build_read_only_cache_key(
        &ro.method_name,
        &ro.input,
        principal_ref,
        ro.component_revision,
        epoch,
    );
    let entry = build_read_only_cache_entry(ro, output);
    // First-writer-wins.
    let _ = cache
        .get_or_insert(
            &key,
            || read_only_cache::PendingReadOnlyCacheEntry::Immediate,
            async move |_| Ok::<_, WorkerExecutorError>(entry),
        )
        .await;
}

/// Builds a [`ReadOnlyCacheEntry`] for the given [`AgentInvocationOutput`]
/// using `ro.cfg.cache_policy` to derive the optional TTL expiry. Shared by
/// the detached observer (see [`populate_read_only_cache`]) and the
/// `invoke_and_await` coalescing path so both produce identical entries.
pub(in crate::worker) fn build_read_only_cache_entry(
    ro: &ReadOnlyContext,
    output: AgentInvocationOutput,
) -> Arc<read_only_cache::ReadOnlyCacheEntry> {
    let expires_at = match &ro.cfg.cache_policy {
        CachePolicy::Ttl(ttl) => {
            tokio::time::Instant::now().checked_add(Duration::from_nanos(ttl.duration_nanos))
        }
        CachePolicy::UntilWrite(_) | CachePolicy::NoCache(_) => None,
    };
    Arc::new(read_only_cache::ReadOnlyCacheEntry { output, expires_at })
}

/// `Ttl(0)` is folded in as it is equivalent to `NoCache`.
pub(in crate::worker) fn is_no_cache(policy: &CachePolicy) -> bool {
    match policy {
        CachePolicy::NoCache(_) => true,
        CachePolicy::Ttl(ttl) => ttl.duration_nanos == 0,
        CachePolicy::UntilWrite(_) => false,
    }
}
