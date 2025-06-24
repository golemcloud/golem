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

use async_trait::async_trait;
use chrono::NaiveDateTime;
use golem_service_base::repo;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct EnvironmentRecord {
    pub environment_id: Uuid,
    pub name: String,
    pub application_id: Uuid,
    pub created_at: NaiveDateTime,
    pub created_by: Uuid,
    pub compatibility_check: bool,
    pub version_check: bool,
    pub security_overrides: bool,
    pub hash: blake3::Hash,
}

#[async_trait]
pub trait EnvironmentRepo: Send + Sync {
    async fn get_by_name(application_id: &Uuid, name: &str) -> repo::Result<EnvironmentRecord>;

    async fn get_by_id(environment_id: &Uuid) -> repo::Result<EnvironmentRecord>;

    async fn ensure(&self) -> EnvironmentRecord;
}
