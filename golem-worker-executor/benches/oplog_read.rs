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

//! Oplog read-path benchmarks covering primary, buffered, archived, and cross-tier reads.
//! Archived scenarios distinguish append-populated warm caches from freshly opened cold handles.
//! Cold-handle construction runs in Criterion's untimed batch setup; source dispatch counts are
//! asserted separately in oplog tests so timed reads contain no counting instrumentation.
//!
//! Run before and after reader changes with:
//!
//! ```text
//! cargo bench -p golem-worker-executor --bench oplog_read
//! ```

use arc_swap::ArcSwap;
use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use golem_common::base_model::account::{AccountEmail, AccountId};
use golem_common::base_model::agent::AgentMode;
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::oplog::{AgentError, OplogEntry, OplogIndex};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, RetryConfig,
};
use golem_common::{model::Timestamp, read_only_lock};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_worker_executor::model::ExecutionStatus;
use golem_worker_executor::services::oplog::{
    BlobOplogArchiveService, CommitLevel, CompressedOplogArchiveService, MultiLayerOplog,
    MultiLayerOplogService, Oplog, OplogArchiveService, OplogService, PrimaryOplogService,
};
use golem_worker_executor::storage::indexed::memory::InMemoryIndexedStorage;
use nonempty_collections::nev;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use tokio::runtime::Runtime;

const ENTRY_COUNT: u64 = 8193;
const ARCHIVE_BOUNDARY: u64 = 4096;
const READ_SIZES: [u64; 7] = [1, 1023, 1024, 1025, 4095, 4096, 4097];

struct Fixture {
    oplog: Arc<dyn Oplog>,
    indexed_storage: Arc<InMemoryIndexedStorage>,
    blob_storage: Arc<InMemoryBlobStorage>,
    owned_agent_id: OwnedAgentId,
    initial_metadata: AgentMetadata,
}

impl Fixture {
    async fn reopen(&self) -> Arc<dyn Oplog> {
        build_service(self.indexed_storage.clone(), self.blob_storage.clone())
            .await
            .open(
                &self.owned_agent_id,
                AgentMode::Durable,
                None,
                self.initial_metadata.clone(),
                last_known_status(),
                execution_status(),
            )
            .await
    }
}

fn metadata(
    agent_id: AgentId,
    account_id: AccountId,
    environment_id: EnvironmentId,
) -> AgentMetadata {
    AgentMetadata {
        agent_id,
        env: Vec::new(),
        environment_id,
        created_by: account_id,
        created_by_email: AccountEmail::new("oplog-read-benchmark@golem.cloud"),
        config: Vec::new(),
        created_at: Timestamp::now_utc(),
        parent: None,
        last_known_status: AgentStatusRecord::default(),
        original_phantom_id: None,
        fingerprint: AgentFingerprint::new(),
        agent_mode: AgentMode::Durable,
    }
}

fn last_known_status() -> read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord> {
    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(ArcSwap::from_pointee(
        AgentStatusRecord::default(),
    )))
}

fn execution_status() -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
        agent_mode: AgentMode::Durable,
        timestamp: Timestamp::now_utc(),
    })))
}

fn entry(value: u64) -> OplogEntry {
    OplogEntry::Error {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        error: AgentError::Unknown(value.to_string()),
        retry_from: OplogIndex::NONE,
        inside_atomic_region: false,
        retry_policy_state: None,
    }
}

async fn build_service(
    indexed_storage: Arc<InMemoryIndexedStorage>,
    blob_storage: Arc<InMemoryBlobStorage>,
) -> Arc<MultiLayerOplogService> {
    let primary = Arc::new(
        PrimaryOplogService::new(
            indexed_storage.clone(),
            blob_storage.clone(),
            ENTRY_COUNT + 1,
            ENTRY_COUNT + 1,
            1024 * 1024,
            RetryConfig::default(),
        )
        .await,
    );
    let compressed: Arc<dyn OplogArchiveService> = Arc::new(CompressedOplogArchiveService::new(
        indexed_storage,
        1,
        RetryConfig::default(),
    ));
    let blob: Arc<dyn OplogArchiveService> =
        Arc::new(BlobOplogArchiveService::new(blob_storage, 2));

    Arc::new(MultiLayerOplogService::new(
        primary,
        nev![compressed, blob],
        ENTRY_COUNT + 1,
        ENTRY_COUNT + 1,
    ))
}

async fn open_fixture(initial_entries: u64) -> Fixture {
    let indexed_storage = Arc::new(InMemoryIndexedStorage::new());
    let blob_storage = Arc::new(InMemoryBlobStorage::new());
    let service = build_service(indexed_storage.clone(), blob_storage.clone()).await;
    let account_id = AccountId::new();
    let environment_id = EnvironmentId::new();
    let agent_id = AgentId {
        component_id: ComponentId::new(),
        agent_id: "oplog-read-benchmark".to_string(),
    };
    let owned_agent_id = OwnedAgentId::new(environment_id, &agent_id);
    let initial_metadata = metadata(agent_id, account_id, environment_id);
    let oplog = service
        .create(
            &owned_agent_id,
            AgentMode::Durable,
            entry(0),
            initial_metadata.clone(),
            last_known_status(),
            execution_status(),
        )
        .await;

    for value in 1..initial_entries {
        oplog.add(entry(value)).await;
    }
    oplog.commit(CommitLevel::Always).await;

    Fixture {
        oplog,
        indexed_storage,
        blob_storage,
        owned_agent_id,
        initial_metadata,
    }
}

async fn primary_fixture() -> Fixture {
    open_fixture(ENTRY_COUNT).await
}

async fn buffered_fixture() -> Fixture {
    let fixture = open_fixture(ENTRY_COUNT - 8).await;
    for value in ENTRY_COUNT - 8..ENTRY_COUNT {
        fixture.oplog.add(entry(value)).await;
    }
    fixture
}

async fn compressed_fixture() -> Fixture {
    let fixture = open_fixture(ENTRY_COUNT).await;
    assert_eq!(
        MultiLayerOplog::try_archive_blocking(&fixture.oplog).await,
        Some(true)
    );
    fixture
}

async fn blob_fixture() -> Fixture {
    let fixture = compressed_fixture().await;
    assert_eq!(
        MultiLayerOplog::try_archive_blocking(&fixture.oplog).await,
        Some(false)
    );
    fixture
}

async fn cross_tier_fixture() -> Fixture {
    let fixture = open_fixture(ARCHIVE_BOUNDARY).await;
    assert_eq!(
        MultiLayerOplog::try_archive_blocking(&fixture.oplog).await,
        Some(true)
    );
    for value in ARCHIVE_BOUNDARY..ENTRY_COUNT {
        fixture.oplog.add(entry(value)).await;
    }
    fixture.oplog.commit(CommitLevel::Always).await;
    fixture
}

fn benchmark_fixture(
    criterion: &mut Criterion,
    runtime: &Runtime,
    name: &str,
    fixture: &Fixture,
    start_for: impl Fn(u64) -> OplogIndex,
) {
    let mut group = criterion.benchmark_group(name);
    for count in READ_SIZES {
        let start = start_for(count);
        group.throughput(Throughput::Elements(count));
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |bencher, &count| {
                bencher.iter(|| {
                    let entries: BTreeMap<OplogIndex, OplogEntry> = runtime
                        .block_on(fixture.oplog.read_exact(black_box(start), black_box(count)));
                    black_box(entries);
                });
            },
        );
    }
    group.finish();
}

fn benchmark_cold_fixture(
    criterion: &mut Criterion,
    runtime: &Runtime,
    name: &str,
    fixture: &Fixture,
    start_for: impl Fn(u64) -> OplogIndex,
) {
    let mut group = criterion.benchmark_group(name);
    for count in READ_SIZES {
        let start = start_for(count);
        group.throughput(Throughput::Elements(count));
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |bencher, &count| {
                bencher.iter_batched(
                    || runtime.block_on(fixture.reopen()),
                    |oplog| {
                        let entries: BTreeMap<OplogIndex, OplogEntry> =
                            runtime.block_on(oplog.read_exact(black_box(start), black_box(count)));
                        black_box(entries);
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_oplog_reads(criterion: &mut Criterion) {
    let runtime = Runtime::new().expect("failed to create benchmark runtime");
    let primary = runtime.block_on(primary_fixture());
    let buffered = runtime.block_on(buffered_fixture());
    let compressed = runtime.block_on(compressed_fixture());
    let blob = runtime.block_on(blob_fixture());
    let cross_tier = runtime.block_on(cross_tier_fixture());

    benchmark_fixture(criterion, &runtime, "primary/start", &primary, |_| {
        OplogIndex::INITIAL
    });
    benchmark_fixture(criterion, &runtime, "primary/tail", &primary, |count| {
        OplogIndex::from_u64(ENTRY_COUNT - count + 1)
    });
    benchmark_fixture(criterion, &runtime, "buffered/tail", &buffered, |count| {
        OplogIndex::from_u64(ENTRY_COUNT - count + 1)
    });
    benchmark_fixture(
        criterion,
        &runtime,
        "compressed/warm-start",
        &compressed,
        |_| OplogIndex::INITIAL,
    );
    benchmark_cold_fixture(
        criterion,
        &runtime,
        "compressed/cold-start",
        &compressed,
        |_| OplogIndex::INITIAL,
    );
    benchmark_fixture(criterion, &runtime, "blob/warm-start", &blob, |_| {
        OplogIndex::INITIAL
    });
    benchmark_cold_fixture(criterion, &runtime, "blob/cold-start", &blob, |_| {
        OplogIndex::INITIAL
    });
    benchmark_fixture(
        criterion,
        &runtime,
        "cross-tier/warm-boundary",
        &cross_tier,
        |count| OplogIndex::from_u64(ARCHIVE_BOUNDARY.saturating_sub(count / 2).max(1)),
    );
    benchmark_cold_fixture(
        criterion,
        &runtime,
        "cross-tier/cold-boundary",
        &cross_tier,
        |count| OplogIndex::from_u64(ARCHIVE_BOUNDARY.saturating_sub(count / 2).max(1)),
    );
}

criterion_group!(benches, bench_oplog_reads);
criterion_main!(benches);
