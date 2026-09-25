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

//! Measures how in-memory blob `exists` scales with keys in the target namespace and with
//! unrelated namespaces.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p golem-service-base --bench blob_storage_exists
//! ```

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::path::Path;
use tokio::runtime::{Builder, Runtime};
use uuid::Uuid;

const TARGET_NAMESPACE_ID: u128 = 1;

fn namespace(id: u128) -> BlobStorageNamespace {
    BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId::from(Uuid::from_u128(id)),
    }
}

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

fn put(
    runtime: &Runtime,
    storage: &InMemoryBlobStorage,
    namespace: BlobStorageNamespace,
    path: &str,
) {
    runtime
        .block_on(storage.put_raw(
            "blob_storage_exists",
            "setup",
            namespace,
            Path::new(path),
            b"x",
        ))
        .unwrap();
}

fn benchmark_exists(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    runtime: &Runtime,
    storage: InMemoryBlobStorage,
    id: BenchmarkId,
) {
    group.bench_function(id, |bencher| {
        bencher.iter(|| {
            black_box(
                runtime
                    .block_on(storage.exists(
                        "blob_storage_exists",
                        "exists",
                        namespace(TARGET_NAMESPACE_ID),
                        Path::new("middle"),
                    ))
                    .unwrap(),
            )
        });
    });
}

fn bench_exists(c: &mut Criterion) {
    let runtime = runtime();
    let mut group = c.benchmark_group("in_memory_blob_exists_missing_path");

    for foreign_namespace_count in [0, 128, 4096] {
        let storage = InMemoryBlobStorage::new();
        put(
            &runtime,
            &storage,
            namespace(TARGET_NAMESPACE_ID),
            "zzz-target/blob",
        );
        for id in 0..foreign_namespace_count {
            put(
                &runtime,
                &storage,
                namespace(TARGET_NAMESPACE_ID + 1 + id as u128),
                "unrelated/blob",
            );
        }
        benchmark_exists(
            &mut group,
            &runtime,
            storage,
            BenchmarkId::new("foreign_namespaces", foreign_namespace_count),
        );
    }

    for target_namespace_key_count in [1, 128, 4096] {
        let storage = InMemoryBlobStorage::new();
        for id in 0..target_namespace_key_count {
            let side = if id % 2 == 0 { "aaa" } else { "zzz" };
            put(
                &runtime,
                &storage,
                namespace(TARGET_NAMESPACE_ID),
                &format!("{side}-{id:04}/blob"),
            );
        }
        benchmark_exists(
            &mut group,
            &runtime,
            storage,
            BenchmarkId::new("target_namespace_keys", target_namespace_key_count),
        );
    }

    group.finish();
}

criterion_group!(benches, bench_exists);
criterion_main!(benches);
