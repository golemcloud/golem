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

use super::FileSystemBlobStorage;
use crate::storage::blob::{BlobRangeError, BlobStorage, BlobStorageNamespace};
use golem_common::model::environment::EnvironmentId;
use pretty_assertions::assert_eq;
use std::io::ErrorKind;
use std::path::Path;
use test_r::test;
use uuid::Uuid;

fn namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId(Uuid::nil()),
    }
}

/// Makes a blob storage in a new temporary directory with the blob `ranges/blob`, which holds
/// `abcdef`, and the empty blob `ranges/empty`.
async fn storage_with_blobs() -> (tempfile::TempDir, FileSystemBlobStorage) {
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();
    storage
        .put_raw(
            "test",
            "put-raw",
            namespace(),
            Path::new("ranges/blob"),
            b"abcdef",
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "test",
            "put-raw",
            namespace(),
            Path::new("ranges/empty"),
            b"",
        )
        .await
        .unwrap();
    (root, storage)
}

#[test]
async fn get_raw_slice_reads_the_inclusive_range_of_the_file() {
    let (_root, storage) = storage_with_blobs().await;
    let read = |path: &'static str, start, end| {
        storage.get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new(path),
            start,
            end,
        )
    };

    let inside = read("ranges/blob", 1, 3).await.unwrap();
    let first_byte = read("ranges/blob", 0, 0).await.unwrap();
    let whole = read("ranges/blob", 0, 5).await.unwrap();
    let last_byte = read("ranges/blob", 5, 5).await.unwrap();
    let missing = read("ranges/missing", 0, 0).await.unwrap();
    let below_a_file = read("ranges/blob/below", 0, 0).await.unwrap();

    assert_eq!(
        (inside, first_byte, whole, last_byte, missing, below_a_file),
        (
            Some(b"bcd".to_vec()),
            Some(b"a".to_vec()),
            Some(b"abcdef".to_vec()),
            Some(b"f".to_vec()),
            None,
            None
        )
    );
}

#[test]
async fn get_raw_slice_gives_a_range_error_for_a_range_that_is_not_in_the_file() {
    let (_root, storage) = storage_with_blobs().await;
    let storage = &storage;
    let outside = [
        ("ranges/blob", 0, 6),
        ("ranges/blob", 6, 6),
        ("ranges/blob", 3, 2),
        ("ranges/blob", u64::MAX, 2),
        ("ranges/blob", u64::MAX, u64::MAX),
        ("ranges/empty", 0, 0),
        // A start after the end gives the error before the backend looks for the file.
        ("ranges/missing", 3, 2),
    ];

    let errors = futures::future::join_all(outside.map(|(path, start, end)| async move {
        storage
            .get_raw_slice(
                "test",
                "get-raw-slice",
                namespace(),
                Path::new(path),
                start,
                end,
            )
            .await
            .map_err(|error| error.downcast_ref::<BlobRangeError>().copied())
    }))
    .await;

    assert_eq!(
        errors,
        outside
            .map(|(_, start, end)| Err(Some(BlobRangeError { start, end })))
            .to_vec()
    );
}

#[test]
async fn get_raw_slice_gives_the_error_of_get_raw_for_a_directory() {
    let (_root, storage) = storage_with_blobs().await;
    let io_error_kind = |error: anyhow::Error| {
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind)
    };

    let slice = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("ranges"),
            0,
            0,
        )
        .await;
    let raw = storage
        .get_raw("test", "get-raw", namespace(), Path::new("ranges"))
        .await;

    assert_eq!(
        (slice.map_err(io_error_kind), raw.map_err(io_error_kind)),
        (
            Err(Some(ErrorKind::IsADirectory)),
            Err(Some(ErrorKind::IsADirectory))
        )
    );
}
