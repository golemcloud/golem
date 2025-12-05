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

use super::audit::ImmutableAuditFields;
use golem_common::error_forwarding;
use golem_common::model::domain_registration::{Domain, DomainRegistration, DomainRegistrationId};
use golem_common::model::environment::EnvironmentId;
use golem_service_base::repo::RepoError;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DomainRegistrationRepoError {
    #[error("There is already an entry for this domain")]
    DomainAlreadyExists,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(DomainRegistrationRepoError, RepoError);

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct DomainRegistrationRecord {
    pub domain_registration_id: Uuid,
    pub environment_id: Uuid,
    pub domain: String,

    #[sqlx(flatten)]
    pub audit: ImmutableAuditFields,
}

impl DomainRegistrationRecord {
    pub fn from_model(model: DomainRegistration, audit: ImmutableAuditFields) -> Self {
        Self {
            domain_registration_id: model.id.0,
            environment_id: model.environment_id.0,
            domain: model.domain.0,
            audit,
        }
    }
}

impl From<DomainRegistrationRecord> for DomainRegistration {
    fn from(value: DomainRegistrationRecord) -> Self {
        Self {
            id: DomainRegistrationId(value.domain_registration_id),
            environment_id: EnvironmentId(value.environment_id),
            domain: Domain(value.domain),
        }
    }
}
