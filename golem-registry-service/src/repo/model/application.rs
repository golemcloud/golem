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
use golem_common::model::account::AccountId;
use golem_common::model::application::{
    Application, ApplicationId, ApplicationName, NewApplicationData,
};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ApplicationRecord {
    pub application_id: Uuid,
    pub name: String,
    pub account_id: Uuid,
    #[sqlx(flatten)]
    pub audit: AuditFields,
}

impl ApplicationRecord {
    pub fn from_model(
        account_id: AccountId,
        application_id: ApplicationId,
        model: NewApplicationData,
        actor: AccountId,
    ) -> Self {
        ApplicationRecord {
            account_id: account_id.0,
            application_id: application_id.0,
            name: model.name.0,
            audit: AuditFields::new(actor.0),
        }
    }
}

impl From<ApplicationRecord> for Application {
    fn from(value: ApplicationRecord) -> Self {
        Self {
            id: ApplicationId(value.application_id),
            account_id: AccountId(value.account_id),
            name: ApplicationName(value.name),
        }
    }
}

#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct ApplicationRevisionRecord {
    pub application_id: Uuid,
    pub revision_id: i64,
    pub name: String,
    pub account_id: Uuid,
    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}
