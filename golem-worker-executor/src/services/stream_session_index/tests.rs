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
use crate::durable_host::durable_stream::SessionError;
use crate::services::oplog::tests::{
    create_test_entry, default_execution_status, default_last_known_status, make_agent_metadata,
};
use crate::services::oplog::{CommitLevel, DurableStreamOplogRecord, Oplog, PrimaryOplogService};
use crate::services::worker_fork::lineage::tests::prepared;
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::indexed::{IndexedStorage, IndexedStorageNamespace};
use crate::storage::keyvalue::KeyValueStorageError;
use crate::storage::keyvalue::fault_injecting::{
    FaultInjectingKeyValueStorage, KeyValueStorageFaults,
};
use golem_common::base_model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, StreamRegistrationInvocation, StreamSessionCancelRequestedRecord,
};
use golem_common::model::RetryConfig;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use test_r::test;

struct Fixture {
    oplog_service: Arc<dyn OplogService>,
    indexed: Arc<InMemoryIndexedStorage>,
    kv: Arc<InMemoryKeyValueStorage>,
    faults: KeyValueStorageFaults,
    index: StreamSessionIndexService,
    id: OwnedAgentId,
    fingerprint: AgentFingerprint,
}

async fn fixture(name: &str) -> Fixture {
    let indexed = Arc::new(InMemoryIndexedStorage::new());
    let oplog_service: Arc<dyn OplogService> = Arc::new(
        PrimaryOplogService::new(
            indexed.clone(),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            100,
            32,
            RetryConfig::default(),
        )
        .await,
    );
    let kv = Arc::new(InMemoryKeyValueStorage::new());
    let faults = KeyValueStorageFaults::default();
    let index = StreamSessionIndexService::new(
        Arc::new(FaultInjectingKeyValueStorage::new(
            kv.clone(),
            faults.clone(),
        )),
        Arc::downgrade(&oplog_service),
    );
    let id = OwnedAgentId::new(EnvironmentId::new(), &crate::test_agent_id(name));
    Fixture {
        oplog_service,
        indexed,
        kv,
        faults,
        index,
        id,
        fingerprint: AgentFingerprint(Uuid::from_u128(0x5e55_1014)),
    }
}

async fn open_oplog(fixture: &Fixture) -> Arc<dyn Oplog> {
    fixture
        .oplog_service
        .open(
            &mut fixture
                .oplog_service
                .lock_lifecycle(&fixture.id.agent_id)
                .await,
            &fixture.id,
            AgentMode::Durable,
            None,
            make_agent_metadata(
                fixture.id.agent_id,
                AccountId::new(),
                fixture.id.environment_id,
            ),
            default_last_known_status(),
            default_execution_status(AgentMode::Durable),
            None,
        )
        .await
        .unwrap()
}

async fn add_create_entry(fixture: &Fixture, oplog: &Arc<dyn Oplog>) {
    oplog
        .add(create_test_entry(
            fixture.id.agent_id,
            AgentMode::Durable,
            golem_common::model::component::ComponentRevision::INITIAL,
            fixture.id.environment_id,
            AccountId::new(),
            fixture.fingerprint.0,
        ))
        .await
        .unwrap();
}

fn unanswered_read(error: &SessionError, id: &OwnedAgentId) -> bool {
    matches!(error, SessionError::Unreadable(unreadable)
        if unreadable.agent_id == id.agent_id && unreadable.is_transient())
}

#[test]
#[test_r::timeout("30s")]
async fn an_unanswered_coverage_read_is_a_temporary_read_failure_of_the_agent() {
    let fixture = fixture("index-coverage-read-fails").await;
    fixture.faults.fail(
        "read",
        1,
        KeyValueStorageError::other("index storage did not answer"),
    );

    let error = fixture
        .index
        .lookup_producer_identity(&fixture.id, AgentMode::Durable, fixture.fingerprint)
        .await
        .expect_err("the coverage read did not answer");

    assert!(unanswered_read(&error, &fixture.id), "{error:?}");
}

#[test]
#[test_r::timeout("30s")]
async fn an_unanswered_snapshot_read_is_a_temporary_read_failure_of_the_agent() {
    let fixture = fixture("index-snapshot-read-fails").await;
    fixture.faults.fail(
        "lookup",
        1,
        KeyValueStorageError::other("index storage did not answer"),
    );

    let error = fixture
        .index
        .lookup_latest(
            &fixture.id,
            AgentMode::Durable,
            fixture.fingerprint,
            &InvocationId(Uuid::from_u128(0x0016_28c5)),
        )
        .await
        .expect_err("the snapshot read did not answer");

    assert!(unanswered_read(&error, &fixture.id), "{error:?}");
}

#[test]
#[test_r::timeout("30s")]
async fn an_exact_lookup_over_corrupt_coverage_reads_its_own_horizon_below_an_unreadable_entry() {
    let fixture = fixture("index-exact-below-refold").await;
    let oplog = open_oplog(&fixture).await;
    let key = StreamSessionKey {
        callee_environment_id: fixture.id.environment_id,
        callee: fixture.id.agent_id,
        callee_fingerprint: fixture.fingerprint,
        idempotency_key: InvocationId(Uuid::from_u128(0x0016_28c4)),
    };
    add_create_entry(&fixture, &oplog).await;
    let horizon = oplog
        .add(
            DurableStreamOplogRecord::Session(
                None,
                Box::new(StreamSessionRecord::Prepared(prepared(&key))),
            )
            .into_inline_entry(),
        )
        .await
        .unwrap();
    let tail = oplog.add(OplogEntry::grow_memory(7)).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    let unreadable = tail.next();
    fixture
        .indexed
        .append(
            "test",
            "corrupt",
            "entry",
            IndexedStorageNamespace::OpLog {
                owned_agent_id: fixture.id.clone(),
                agent_mode: AgentMode::Durable,
            },
            &fixture.id.storage_key(),
            u64::from(unreadable),
            vec![0xff, 0xff, 0xff],
            None,
        )
        .await
        .unwrap();
    let namespace = StreamSessionIndexService::namespace(&fixture.id, fixture.fingerprint);
    fixture
        .kv
        .set(
            "test",
            "corrupt",
            "metadata",
            namespace.clone(),
            METADATA_FIELD,
            &[0xff, 0x13],
        )
        .await
        .unwrap();

    let status = fixture
        .index
        .lookup_persisted(
            &fixture.id,
            AgentMode::Durable,
            fixture.fingerprint,
            horizon,
            &key.idempotency_key,
        )
        .await
        .expect("the exact snapshot is readable from the oplog")
        .expect("the session is prepared at the horizon");

    assert_eq!(status.first_prepared, Some(horizon));
    assert_eq!(
        fixture
            .kv
            .get("test", "check", "metadata", namespace, METADATA_FIELD)
            .await
            .unwrap()
            .as_deref(),
        Some(&[0xff, 0x13][..]),
        "the exact lookup folds in private storage and leaves the shared coverage to other reads"
    );
}

/// The coverage probe of an exact lookup is a standalone coverage read, so it has the label and
/// entity of that read, `read / metadata`: an exact lookup over a healthy index makes the same
/// reads as a latest lookup and one more `read / metadata`.
#[test]
#[test_r::timeout("30s")]
async fn the_coverage_probe_of_an_exact_lookup_is_a_coverage_read() {
    let fixture = fixture("index-exact-probe-label").await;
    let oplog = open_oplog(&fixture).await;
    add_create_entry(&fixture, &oplog).await;
    oplog.commit(CommitLevel::Always).await.unwrap();
    let horizon = oplog.current_oplog_index().await;
    let key = InvocationId(Uuid::from_u128(0x0016_28f6));
    fixture
        .index
        .lookup_latest(&fixture.id, AgentMode::Durable, fixture.fingerprint, &key)
        .await
        .unwrap();
    let counts = || {
        (
            fixture.faults.attempts(),
            fixture.faults.entity_calls("read", "metadata"),
        )
    };

    let before = counts();
    fixture
        .index
        .lookup_latest(&fixture.id, AgentMode::Durable, fixture.fingerprint, &key)
        .await
        .unwrap();
    let after = counts();
    let latest = (after.0 - before.0, after.1 - before.1);
    fixture
        .index
        .lookup_persisted(
            &fixture.id,
            AgentMode::Durable,
            fixture.fingerprint,
            horizon,
            &key,
        )
        .await
        .unwrap();
    let exact = (counts().0 - after.0, counts().1 - after.1);

    assert_eq!(exact, (latest.0 + 1, latest.1 + 1));
}

async fn prepare_horizon(
    fixture: &Fixture,
    oplog: &Arc<dyn Oplog>,
    key: &StreamSessionKey,
) -> OplogIndex {
    add_create_entry(fixture, oplog).await;
    let horizon = oplog
        .add(
            DurableStreamOplogRecord::Session(
                None,
                Box::new(StreamSessionRecord::Prepared(prepared(key))),
            )
            .into_inline_entry(),
        )
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    horizon
}

fn session_key(fixture: &Fixture, key: u128) -> StreamSessionKey {
    StreamSessionKey {
        callee_environment_id: fixture.id.environment_id,
        callee: fixture.id.agent_id,
        callee_fingerprint: fixture.fingerprint,
        idempotency_key: InvocationId(Uuid::from_u128(key)),
    }
}

/// Runs an exact lookup at `horizon` and runs `interleave` after its coverage probe answered and
/// before its catch-up reads the coverage.
async fn exact_lookup_with_change_after_probe(
    fixture: &Fixture,
    horizon: OplogIndex,
    key: &InvocationId,
    interleave: impl AsyncFnOnce(),
) -> Result<Option<DurableStreamSessionStatus>, SessionError> {
    let probe = fixture.faults.pause_next("read");
    let lookup = fixture.index.lookup_persisted(
        &fixture.id,
        AgentMode::Durable,
        fixture.fingerprint,
        horizon,
        key,
    );
    let drive = async {
        probe.entered().await;
        let catch_up = fixture.faults.pause_next("read");
        probe.release();
        catch_up.entered().await;
        interleave().await;
        catch_up.release();
    };
    let (result, ()) = tokio::join!(lookup, drive);
    result
}

/// Another writer can publish coverage above the horizon of an exact lookup after its probe saw
/// no coverage. The lookup still answers at its horizon.
#[test]
#[test_r::timeout("30s")]
async fn an_exact_lookup_answers_its_horizon_when_another_writer_advances_absent_coverage() {
    let fixture = fixture("index-exact-absent-advanced").await;
    let oplog = open_oplog(&fixture).await;
    let key = session_key(&fixture, 0x0016_28a1);
    let horizon = prepare_horizon(&fixture, &oplog, &key).await;
    // A record above the horizon that changes the session, so the state at the horizon differs
    // from the newer state that the other writer publishes.
    oplog
        .add(
            DurableStreamOplogRecord::Session(
                None,
                Box::new(StreamSessionRecord::CancelRequested(
                    StreamSessionCancelRequestedRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: StreamRegistrationInvocation::Local(key.idempotency_key),
                    },
                )),
            )
            .into_inline_entry(),
        )
        .await
        .unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    let writer =
        StreamSessionIndexService::new(fixture.kv.clone(), Arc::downgrade(&fixture.oplog_service));

    let status =
        exact_lookup_with_change_after_probe(&fixture, horizon, &key.idempotency_key, async || {
            writer
                .lookup_latest(
                    &fixture.id,
                    AgentMode::Durable,
                    fixture.fingerprint,
                    &key.idempotency_key,
                )
                .await
                .unwrap();
        })
        .await
        .expect("the exact lookup answers at its horizon")
        .expect("the session is prepared at the horizon");

    assert_eq!(status.first_prepared, Some(horizon));
    assert!(
        !status.cancellation_requested,
        "the lookup answers with the session at its horizon, not with the newer session"
    );
}

/// The coverage can become unreadable after the probe of an exact lookup read it within the
/// horizon. The repair of the shared index then reads an unreadable entry above the horizon, and
/// the lookup still answers at its horizon.
#[test]
#[test_r::timeout("30s")]
async fn an_exact_lookup_answers_its_horizon_when_coverage_becomes_corrupt_after_the_probe() {
    let fixture = fixture("index-exact-corrupt-after-probe").await;
    let oplog = open_oplog(&fixture).await;
    let key = session_key(&fixture, 0x0016_28a2);
    let horizon = prepare_horizon(&fixture, &oplog, &key).await;
    fixture
        .index
        .lookup_latest(
            &fixture.id,
            AgentMode::Durable,
            fixture.fingerprint,
            &key.idempotency_key,
        )
        .await
        .unwrap();
    let tail = oplog.add(OplogEntry::grow_memory(7)).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    fixture
        .indexed
        .append(
            "test",
            "corrupt",
            "entry",
            IndexedStorageNamespace::OpLog {
                owned_agent_id: fixture.id.clone(),
                agent_mode: AgentMode::Durable,
            },
            &fixture.id.storage_key(),
            u64::from(tail.next()),
            vec![0xff, 0xff, 0xff],
            None,
        )
        .await
        .unwrap();
    let namespace = StreamSessionIndexService::namespace(&fixture.id, fixture.fingerprint);

    let status =
        exact_lookup_with_change_after_probe(&fixture, horizon, &key.idempotency_key, async || {
            fixture
                .kv
                .set(
                    "test",
                    "corrupt",
                    "metadata",
                    namespace.clone(),
                    METADATA_FIELD,
                    &[0xff, 0x13],
                )
                .await
                .unwrap();
        })
        .await
        .expect("the exact lookup answers at its horizon")
        .expect("the session is prepared at the horizon");

    assert_eq!(status.first_prepared, Some(horizon));
}
