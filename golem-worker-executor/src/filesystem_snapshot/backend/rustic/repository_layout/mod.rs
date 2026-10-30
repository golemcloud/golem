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

//! Stable repository and prune-protocol paths and rustic file-type mapping.

use rustic_core::{ErrorKind, FileType, Id, RusticError, RusticResult};
use std::path::{Path, PathBuf};

/// The path of the config file of a repository.
pub(super) const CONFIG_PATH: &str = "config";

/// The directory of the index files of a repository.
pub(super) const INDEX_PATH: &str = "index";

/// The directory of the key files of a repository.
pub(super) const KEYS_PATH: &str = "keys";

/// The directory of a file type, or the config file's path for `Config`.
pub(super) fn type_path(tpe: FileType) -> &'static str {
    match tpe {
        FileType::Config => CONFIG_PATH,
        FileType::Pack => DATA_PATH,
        FileType::Index => INDEX_PATH,
        FileType::Key => KEYS_PATH,
        FileType::Snapshot => SNAPSHOTS_PATH,
    }
}

/// Gives the path of a file of the repository, relative to the root of the namespace.
pub(super) fn file_path(tpe: FileType, id: &Id) -> RusticResult<Box<Path>> {
    let hex = id.to_hex();
    let name = hex.as_str();
    let directory = type_path(tpe);
    let path = match tpe {
        FileType::Config => CONFIG_PATH.to_string(),
        FileType::Pack => {
            let prefix = name.get(..2).ok_or_else(|| {
                RusticError::new(
                    ErrorKind::Internal,
                    "The id `{id}` has fewer than two hex digits.",
                )
                .attach_context("id", name)
            })?;
            format!("{directory}/{prefix}/{name}")
        }
        FileType::Index | FileType::Key | FileType::Snapshot => {
            format!("{directory}/{name}")
        }
    };
    Ok(PathBuf::from(path).into_boxed_path())
}

/// The directory of all prune protocol records, relative to the repository namespace.
pub(super) const PRUNE_PATH: &str = "golem";

/// The directory of the ledger entries, relative to the root of the namespace of the scope.
pub(super) const LEDGERS_PATH: &str = "golem/prune-ledgers";

/// The directory of the prune claims, relative to the root of the namespace of the scope.
pub(super) const CLAIMS_PATH: &str = "golem/prune-claims";

/// The directory of the records of freed bytes, relative to the root of the namespace of the scope.
pub(super) const FREED_PATH: &str = "golem/prune-freed";

/// The path of the packs of a repository, relative to the root of the namespace of the scope.
pub(super) const DATA_PATH: &str = "data";

/// The directory of the snapshot files of a repository.
pub(super) const SNAPSHOTS_PATH: &str = "snapshots";
