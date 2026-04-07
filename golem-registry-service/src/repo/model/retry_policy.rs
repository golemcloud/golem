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

use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::retry_policy::{RetryPolicyId, RetryPolicyRevision};
use golem_service_base::model::retry_policy::StoredRetryPolicy;
use golem_service_base::repo::RepoError;
use golem_service_base::repo::SqlDateTime;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RetryPolicyRepoError {
    #[error("There is already a retry policy with this name in this environment")]
    NameViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(RetryPolicyRepoError, RepoError);

#[derive(Debug, Clone)]
pub struct RetryPolicyCreationRecord {
    pub environment_id: Uuid,
    pub name: String,
    pub revision: RetryPolicyRevisionRecord,
}

impl RetryPolicyCreationRecord {
    pub fn new(
        id: RetryPolicyId,
        environment_id: EnvironmentId,
        name: String,
        priority: u32,
        predicate_json: String,
        policy_json: String,
        actor: AccountId,
    ) -> Self {
        Self {
            environment_id: environment_id.0,
            name,
            revision: RetryPolicyRevisionRecord {
                retry_policy_id: id.0,
                revision_id: RetryPolicyRevision::INITIAL.into(),
                priority: priority as i64,
                predicate_json,
                policy_json,
                audit: DeletableRevisionAuditFields::new(actor.0),
            },
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct RetryPolicyRecord {
    pub retry_policy_id: Uuid,
    pub environment_id: Uuid,
    pub name: String,

    #[sqlx(flatten)]
    pub audit: AuditFields,

    pub current_revision_id: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct RetryPolicyRevisionRecord {
    pub retry_policy_id: Uuid,
    pub revision_id: i64,

    pub priority: i64,
    pub predicate_json: String,
    pub policy_json: String,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl RetryPolicyRevisionRecord {
    pub fn from_model(value: StoredRetryPolicy, audit: DeletableRevisionAuditFields) -> Self {
        Self {
            retry_policy_id: value.id.0,
            revision_id: value.revision.into(),
            priority: value.priority as i64,
            predicate_json: value.predicate_json,
            policy_json: value.policy_json,
            audit,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct RetryPolicyExtRevisionRecord {
    pub environment_id: Uuid,
    pub name: String,

    pub entity_created_at: SqlDateTime,

    #[sqlx(flatten)]
    pub revision: RetryPolicyRevisionRecord,
}

impl TryFrom<RetryPolicyExtRevisionRecord> for StoredRetryPolicy {
    type Error = RetryPolicyRepoError;
    fn try_from(value: RetryPolicyExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: RetryPolicyId(value.revision.retry_policy_id),
            environment_id: EnvironmentId(value.environment_id),
            name: value.name,
            revision: value.revision.revision_id.try_into()?,
            priority: value.revision.priority as u32,
            predicate_json: value.revision.predicate_json,
            policy_json: value.revision.policy_json,
        })
    }
}
