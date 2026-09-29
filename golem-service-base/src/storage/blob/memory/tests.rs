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

use super::{Entry, InMemoryBlobStorage, Key};
use crate::storage::blob::{
    BlobMetadata, BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath,
    normalized_blob_path,
};
use golem_common::model::Timestamp;
use golem_common::model::environment::EnvironmentId;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use test_r::test;
use uuid::Uuid;

fn namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId(Uuid::nil()),
    }
}

fn other_namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId(Uuid::from_u128(1)),
    }
}

/// The key of the blob at `path` in `namespace`.
fn blob(namespace: BlobStorageNamespace, path: &str) -> Key {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    Key {
        namespace,
        dir: dir.to_string(),
        file: Some(file.to_string()),
    }
}

/// The key of the directory that `create_dir` made at `path` in `namespace`.
fn directory(namespace: BlobStorageNamespace, path: &str) -> Key {
    Key {
        namespace,
        dir: path.to_string(),
        file: None,
    }
}

fn file_entry(size: u64, time: u64) -> Entry {
    Entry::File {
        data: bytes::Bytes::from(vec![0; size as usize]),
        metadata: BlobMetadata {
            last_modified_at: Timestamp::from(time),
            size,
        },
    }
}

fn directory_entry(time: u64) -> Entry {
    Entry::Directory {
        created_at: Timestamp::from(time),
    }
}

/// A map with blobs and directories in and below the directory `a`, and a sibling `ab` whose
/// name starts with the name of `a`. The path `a/b` holds a blob and a directory. The other
/// namespace holds a blob at `a/x` too.
fn storage() -> BTreeMap<Key, Entry> {
    BTreeMap::from([
        (blob(namespace(), "a"), file_entry(1, 10)),
        (directory(namespace(), "a"), directory_entry(11)),
        (blob(namespace(), "a/x"), file_entry(2, 12)),
        (blob(namespace(), "a/b"), file_entry(3, 13)),
        (directory(namespace(), "a/b"), directory_entry(14)),
        (blob(namespace(), "a/b/y"), file_entry(4, 15)),
        (directory(namespace(), "a/c/d"), directory_entry(16)),
        (blob(namespace(), "ab/z"), file_entry(5, 17)),
        (blob(other_namespace(), "a/x"), file_entry(6, 18)),
    ])
}

/// The one form of the path, as the storage gives it to a backend.
fn at(path: &'static str) -> NormalizedBlobPath<'static> {
    normalized_blob_path(Path::new(path)).unwrap()
}

fn sorted<T: Ord>(mut items: Vec<T>) -> Vec<T> {
    items.sort();
    items
}

fn listed(path: &str, size: u64) -> ListedBlob {
    ListedBlob {
        path: Path::new(path).into(),
        size,
    }
}

#[test]
fn the_listing_gives_blobs_in_the_directory_and_created_directories_at_any_depth_once() {
    let data = storage();

    let listings = ["a", ""]
        .map(|path| sorted(InMemoryBlobStorage::listing(&data, &namespace(), &at(path)).unwrap()));

    assert_eq!(
        listings,
        [
            vec![
                PathBuf::from("a/b"),
                PathBuf::from("a/c/d"),
                PathBuf::from("a/x")
            ],
            vec![
                PathBuf::from("a"),
                PathBuf::from("a/b"),
                PathBuf::from("a/c/d")
            ],
        ]
    );
}

#[test]
fn blobs_below_give_each_blob_at_any_depth_and_no_directory() {
    let data = storage();

    let below = ["a", "", "a/c", "missing"].map(|path| {
        sorted(
            InMemoryBlobStorage::blobs_below(&data, &namespace(), &at(path))
                .unwrap()
                .into_vec(),
        )
    });

    assert_eq!(
        below,
        [
            vec![listed("a/b", 3), listed("a/b/y", 4), listed("a/x", 2)],
            vec![
                listed("a", 1),
                listed("a/b", 3),
                listed("a/b/y", 4),
                listed("a/x", 2),
                listed("ab/z", 5)
            ],
            vec![],
            vec![],
        ]
    );
}

#[test]
fn a_blob_at_the_path_wins_over_a_directory_and_keys_below_make_a_directory() {
    let data = storage();

    let answers = ["a/b", "a/b/y", "a/c", "a/c/d", "ab", "a/m", "b"]
        .map(|path| InMemoryBlobStorage::existence(&data, &namespace(), &at(path)).unwrap());

    assert_eq!(
        answers,
        [
            ExistsResult::File,
            ExistsResult::File,
            ExistsResult::Directory,
            ExistsResult::Directory,
            ExistsResult::Directory,
            ExistsResult::DoesNotExist,
            ExistsResult::DoesNotExist,
        ]
    );
}

#[test]
fn metadata_gives_the_blob_and_else_a_created_directory_with_a_size_of_zero() {
    let data = storage();

    let answers = ["a/b", "a/c/d", "a/c", "missing"].map(|path| {
        InMemoryBlobStorage::metadata(&data, &namespace(), &at(path))
            .unwrap()
            .map(|metadata| (metadata.size, metadata.last_modified_at))
    });

    assert_eq!(
        answers,
        [
            Some((3, Timestamp::from(13))),
            Some((0, Timestamp::from(16))),
            None,
            None,
        ]
    );
}
