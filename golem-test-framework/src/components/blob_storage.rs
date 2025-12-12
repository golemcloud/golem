// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum BlobStorageInfo {
    LocalFileSytem { root: PathBuf },
}

impl BlobStorageInfo {
    pub fn config(&self) -> BlobStorageConfig {
        match self {
            BlobStorageInfo::LocalFileSytem { root } => {
                BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
                    root: root.clone(),
                })
            }
        }
    }

    pub fn env(&self) -> HashMap<String, String> {
        match self {
            BlobStorageInfo::LocalFileSytem { root } => [
                (
                    "GOLEM__BLOB_STORAGE__TYPE".to_string(),
                    "LocalFileSystem".to_string(),
                ),
                (
                    "GOLEM__BLOB_STORAGE__CONFIG__ROOT".to_string(),
                    root.to_string_lossy().to_string(),
                ),
            ]
            .into(),
        }
    }
}
