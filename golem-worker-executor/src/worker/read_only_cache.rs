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
mod tests {
    #[test]
    async fn pending_admission_preserves_queued_signal() {
        let pending = PendingReadOnlyInvocation::new();
        pending.publish(AdmissionSignal::Queued);
        pending.publish(AdmissionSignal::Failure(
            golem_service_base::error::worker_executor::WorkerExecutorError::runtime("later"),
        ));
        assert!(matches!(pending.admission().await, AdmissionSignal::Queued));
    }

    use super::*;
    use golem_common::base_model::Empty;
    use golem_common::base_model::component_metadata::KnownExports;
    use golem_common::model::agent::{
        AgentMode, AgentPrincipal, AgentTypeName, CachePolicy, ReadOnlyConfig, Snapshotting,
    };
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::oplog::RawSnapshotData;
    use golem_common::model::{AgentId, IdempotencyKey};
    use golem_common::schema::UnionValuePayload;
    use golem_common::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, OutputSchema,
    };
    use golem_common::schema::graph::SchemaGraph;
    use std::collections::BTreeMap;
    use test_r::test;
    use uuid::Uuid;

    fn rev(n: u64) -> ComponentRevision {
        ComponentRevision::new(n).unwrap()
    }

    fn read_only_method(name: &str, ro: Option<ReadOnlyConfig>) -> AgentMethodSchema {
        AgentMethodSchema {
            name: name.to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: ro,
        }
    }

    fn metadata_with_one_agent_type(
        agent_type: AgentTypeName,
        methods: Vec<AgentMethodSchema>,
    ) -> ComponentMetadata {
        let at = AgentTypeSchema {
            kind: golem_common::schema::agent::AgentTypeKind::Regular,
            type_name: agent_type,
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            methods,
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        };
        ComponentMetadata::from_parts(
            KnownExports::default(),
            vec![],
            None,
            None,
            vec![at],
            BTreeMap::new(),
        )
    }

    fn principal(seed: u128) -> Principal {
        let agent_id = AgentId {
            component_id: ComponentId(Uuid::from_u128(seed)),
            agent_id: format!("agent-{seed}"),
        };
        Principal::Agent(AgentPrincipal { agent_id })
    }

    fn tuple(values: Vec<SchemaValue>) -> SchemaValue {
        SchemaValue::Record { fields: values }
    }

    fn multimodal(values: Vec<(&str, SchemaValue)>) -> SchemaValue {
        SchemaValue::List {
            elements: values
                .into_iter()
                .map(|(name, value)| {
                    SchemaValue::Union(UnionValuePayload {
                        tag: name.to_string(),
                        body: Box::new(value),
                    })
                })
                .collect(),
        }
    }

    struct HashOutput(blake3::Hasher);

    impl desert_rust::BinaryOutput for HashOutput {
        fn write_u8(&mut self, value: u8) {
            self.0.update(&[value]);
        }

        fn write_bytes(&mut self, bytes: &[u8]) {
            self.0.update(bytes);
        }
    }

    fn streamed_digest<T: desert_rust::BinarySerializer>(value: &T) -> [u8; 32] {
        let output = desert_rust::serialize(value, HashOutput(blake3::Hasher::new())).unwrap();
        *output.0.finalize().as_bytes()
    }

    #[test]
    fn streamed_cache_digest_matches_materialized_bytes() {
        for value in [
            SchemaValue::U64(u64::MAX),
            SchemaValue::String("quotes: \"; nul: \0; unicode: árvíz".to_string()),
            multimodal(vec![
                ("a", SchemaValue::F64(-0.0)),
                ("b", SchemaValue::S64(i64::MIN)),
            ]),
            tuple(vec![SchemaValue::List {
                elements: (0..10_000)
                    .map(|i| SchemaValue::U8((i % 251) as u8))
                    .collect(),
            }]),
        ] {
            assert_eq!(
                streamed_digest(&value),
                digest_bytes(&canonicalize_schema_value(&value))
            );
        }
        for p in [Principal::anonymous(), principal(17), principal(23)] {
            assert_eq!(streamed_digest(&p), digest_bytes(&principal_bytes(&p)));
        }
    }

    #[test]
    #[ignore]
    fn profile_invocation_payload_passes() {
        use std::hint::black_box;
        use std::time::Instant;

        fn sample(mut f: impl FnMut(), iterations: u32) -> Vec<u128> {
            for _ in 0..10 {
                f();
            }
            let mut samples = Vec::new();
            for _ in 0..7 {
                let start = Instant::now();
                for _ in 0..iterations {
                    f();
                }
                samples.push(start.elapsed().as_nanos() / u128::from(iterations));
            }
            samples.sort_unstable();
            samples
        }

        for size in [100, 10_000] {
            let value = tuple(vec![SchemaValue::List {
                elements: (0..size)
                    .map(|i| SchemaValue::U8((i % 251) as u8))
                    .collect(),
            }]);
            let shared = std::sync::Arc::new(value.clone());
            let proto: golem_schema::proto::golem::schema::SchemaValue =
                value.clone().try_into().unwrap();
            let bytes = canonicalize_schema_value(&value);
            println!(
                "size={size} serialized_bytes={} protobuf_bytes={}",
                bytes.len(),
                prost::Message::encoded_len(&proto)
            );
            for (label, samples) in [
                (
                    "cache_vec_hash",
                    sample(
                        || {
                            black_box(digest_bytes(&canonicalize_schema_value(black_box(&value))));
                        },
                        100,
                    ),
                ),
                (
                    "cache_stream_hash",
                    sample(
                        || {
                            black_box(streamed_digest(black_box(&value)));
                        },
                        100,
                    ),
                ),
                (
                    "native_clone",
                    sample(
                        || {
                            black_box(black_box(&value).clone());
                        },
                        100,
                    ),
                ),
                (
                    "shared_clone",
                    sample(
                        || {
                            black_box(black_box(&shared).clone());
                        },
                        100,
                    ),
                ),
                (
                    "protobuf_clone_decode",
                    sample(
                        || {
                            black_box(SchemaValue::try_from(black_box(&proto).clone()).unwrap());
                        },
                        100,
                    ),
                ),
                (
                    "protobuf_encoded_len",
                    sample(
                        || {
                            black_box(prost::Message::encoded_len(black_box(&proto)));
                        },
                        100,
                    ),
                ),
                (
                    "output_clone_convert",
                    sample(
                        || {
                            black_box(
                                golem_schema::proto::golem::schema::SchemaValue::try_from(
                                    black_box(&value).clone(),
                                )
                                .unwrap(),
                            );
                        },
                        100,
                    ),
                ),
            ] {
                println!("{label} ns median={} samples={samples:?}", samples[3]);
            }
        }
    }

    #[test]
    fn equal_inputs_produce_equal_keys() {
        let a = tuple(vec![SchemaValue::U32(1), SchemaValue::U32(2)]);
        let b = tuple(vec![SchemaValue::U32(1), SchemaValue::U32(2)]);
        let ka = build_read_only_cache_key("m", &a, None, rev(7), 3);
        let kb = build_read_only_cache_key("m", &b, None, rev(7), 3);
        assert_eq!(ka, kb);
    }

    #[test]
    fn different_inputs_produce_different_keys() {
        let a = tuple(vec![SchemaValue::U32(1)]);
        let b = tuple(vec![SchemaValue::U32(2)]);
        let ka = build_read_only_cache_key("m", &a, None, rev(7), 3);
        let kb = build_read_only_cache_key("m", &b, None, rev(7), 3);
        assert_ne!(ka, kb);
    }

    #[test]
    fn swapped_tuple_positions_produce_different_keys() {
        let a = tuple(vec![SchemaValue::U32(1), SchemaValue::U32(2)]);
        let b = tuple(vec![SchemaValue::U32(2), SchemaValue::U32(1)]);
        let ka = build_read_only_cache_key("m", &a, None, rev(7), 3);
        let kb = build_read_only_cache_key("m", &b, None, rev(7), 3);
        assert_ne!(ka, kb);
    }

    #[test]
    fn multimodal_field_renames_produce_different_keys() {
        let a = multimodal(vec![("x", SchemaValue::U32(1))]);
        let b = multimodal(vec![("y", SchemaValue::U32(1))]);
        let ka = build_read_only_cache_key("m", &a, None, rev(7), 3);
        let kb = build_read_only_cache_key("m", &b, None, rev(7), 3);
        assert_ne!(ka, kb);
    }

    #[test]
    fn epoch_in_key_changes_key_when_epoch_bumps() {
        let a = tuple(vec![SchemaValue::U32(1)]);
        let k1 = build_read_only_cache_key("m", &a, None, rev(1), 1);
        let k2 = build_read_only_cache_key("m", &a, None, rev(1), 2);
        assert_ne!(k1, k2);
    }

    #[test]
    fn component_revision_in_key_changes_key_after_update() {
        let a = tuple(vec![SchemaValue::U32(1)]);
        let k1 = build_read_only_cache_key("m", &a, None, rev(1), 1);
        let k2 = build_read_only_cache_key("m", &a, None, rev(2), 1);
        assert_ne!(k1, k2);
    }

    #[test]
    fn principal_off_means_principal_digest_is_none() {
        let a = tuple(vec![SchemaValue::U32(1)]);
        let key = build_read_only_cache_key("m", &a, None, rev(1), 1);
        assert!(key.principal_digest.is_none());
    }

    #[test]
    fn principal_on_distinguishes_principals() {
        let a = tuple(vec![SchemaValue::U32(1)]);
        let p1 = principal(1);
        let p2 = principal(2);
        let k1 = build_read_only_cache_key("m", &a, Some(&p1), rev(1), 1);
        let k2 = build_read_only_cache_key("m", &a, Some(&p2), rev(1), 1);
        assert_ne!(k1, k2);
        assert!(k1.principal_digest.is_some());
    }

    #[test]
    fn method_name_changes_the_key() {
        let a = tuple(vec![SchemaValue::U32(1)]);
        let k1 = build_read_only_cache_key("foo", &a, None, rev(1), 1);
        let k2 = build_read_only_cache_key("bar", &a, None, rev(1), 1);
        assert_ne!(k1, k2);
    }

    #[test]
    fn classify_non_method_variants_are_mutating() {
        let m = AgentInvocation::ManualUpdate {
            target_revision: ComponentRevision::new(2).unwrap(),
        };
        assert_eq!(
            classify_invocation(None, None, &m),
            InvocationEffect::Mutating
        );
    }

    #[test]
    fn classify_load_snapshot_is_mutating() {
        let invocation = AgentInvocation::LoadSnapshot {
            idempotency_key: IdempotencyKey::new("load-snapshot".to_string()),
            snapshot: RawSnapshotData {
                data: vec![1, 2, 3],
                mime_type: "application/octet-stream".to_string(),
            },
        };

        assert_eq!(
            classify_invocation(None, None, &invocation),
            InvocationEffect::Mutating
        );
    }

    // -----------------------------------------------------------------------
    // resolve_read_only_method: lookup that gates read-only strictness in
    // `Worker::invoke`. Verifies that the lookup returns the right method
    // value (including its `read_only` config), discriminates between
    // read-only and non-read-only methods, and handles unknown method /
    // unknown agent-type inputs cleanly.
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_read_only_method_returns_method_with_read_only_config() {
        let agent_type = AgentTypeName("TestAgent".to_string());
        let cfg = ReadOnlyConfig {
            cache_policy: CachePolicy::UntilWrite(Empty {}),
            uses_principal: false,
        };
        let metadata = metadata_with_one_agent_type(
            agent_type.clone(),
            vec![
                read_only_method("get", Some(cfg.clone())),
                read_only_method("set", None),
            ],
        );

        let got =
            resolve_read_only_method(&metadata, &agent_type, "get").expect("get must resolve");
        assert_eq!(got.read_only, Some(cfg));
    }

    #[test]
    fn resolve_read_only_method_returns_non_read_only_method_as_is() {
        let agent_type = AgentTypeName("TestAgent".to_string());
        let metadata =
            metadata_with_one_agent_type(agent_type.clone(), vec![read_only_method("set", None)]);

        let got =
            resolve_read_only_method(&metadata, &agent_type, "set").expect("set must resolve");
        assert!(
            got.read_only.is_none(),
            "non-read-only method must round-trip with read_only == None"
        );
    }

    #[test]
    fn resolve_read_only_method_returns_none_for_unknown_method_name() {
        let agent_type = AgentTypeName("TestAgent".to_string());
        let metadata =
            metadata_with_one_agent_type(agent_type.clone(), vec![read_only_method("get", None)]);
        assert!(resolve_read_only_method(&metadata, &agent_type, "missing").is_none());
    }

    #[test]
    fn resolve_read_only_method_returns_none_for_unknown_agent_type() {
        let agent_type = AgentTypeName("TestAgent".to_string());
        let metadata =
            metadata_with_one_agent_type(agent_type, vec![read_only_method("get", None)]);
        let wrong = AgentTypeName("OtherAgent".to_string());
        assert!(resolve_read_only_method(&metadata, &wrong, "get").is_none());
    }

    #[test]
    fn classify_unknown_metadata_is_unknown_assume_mutating() {
        use golem_common::model::IdempotencyKey;
        use golem_common::model::invocation_context::InvocationContextStack;

        let m = AgentInvocation::AgentMethod {
            idempotency_key: IdempotencyKey::new("k".into()),
            method_name: "m".into(),
            input: tuple(vec![]),
            invocation_context: InvocationContextStack::fresh(),
            principal: Principal::anonymous(),
            scope_card: None,
        };
        assert_eq!(
            classify_invocation(None, None, &m),
            InvocationEffect::UnknownAssumeMutating
        );
    }
}
