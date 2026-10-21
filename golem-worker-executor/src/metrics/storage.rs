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

// Re-export shared storage metrics from golem-service-base so all services
// can use the same metric definitions (same Prometheus global registry).
pub use golem_service_base::metrics::storage::*;

pub const STORAGE_TYPE_BLOB_STORE: &str = "blob_store";
pub const STORAGE_TYPE_KV: &str = "kv";
pub const STORAGE_TYPE_OPLOG: &str = "oplog";
pub const STORAGE_TYPE_OPLOG_ARCHIVE: &str = "oplog_archive";

/// Re-exported from [`crate::identity`], which owns the process identity.
/// Kept here so existing metric-recording call sites can keep using
/// `crate::metrics::storage::executor_id()`.
pub use crate::identity::executor_id;
