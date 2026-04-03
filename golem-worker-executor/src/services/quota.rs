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

use std::sync::Arc;

/// Executor-local quota enforcement service.
///
/// Manages quota leases obtained from the shard manager and enforces
/// resource limits for workers running on this executor.
pub trait QuotaService: Send + Sync {}

/// Quota service backed by the shard manager's quota lease RPCs.
pub struct GrpcQuotaService {
    _client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>,
}

impl GrpcQuotaService {
    pub fn new(client: Arc<dyn golem_service_base::clients::shard_manager::ShardManager>) -> Self {
        Self { _client: client }
    }
}

impl QuotaService for GrpcQuotaService {}

/// Quota service that grants unlimited leases.  Used for local development
/// and the debugging service where no real shard manager is available.
pub struct UnlimitedQuotaService;

impl QuotaService for UnlimitedQuotaService {}
