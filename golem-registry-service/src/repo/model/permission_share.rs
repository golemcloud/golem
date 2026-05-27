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

use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields};
use golem_common::error_forwarding;
use golem_common::model::account::AccountId;
use golem_common::model::permission_share::{
    PermissionShare, PermissionShareData, PermissionShareId, PermissionShareRevision,
};
use golem_service_base::repo::{Blob, RepoError};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PermissionShareRepoError {
    #[error("There is already a permission share with this name")]
    ShareViolatesUniqueness,
    #[error("Concurrent modification")]
    ConcurrentModification,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(PermissionShareRepoError, RepoError);

#[derive(Debug, Clone, PartialEq, Eq, desert_rust::BinaryCodec)]
pub struct PermissionShareDataRecord {
    pub lower_positive: Vec<String>,
    pub lower_negative: Vec<String>,
    pub upper_positive: Vec<String>,
    pub upper_negative: Vec<String>,
}

impl From<PermissionShareData> for PermissionShareDataRecord {
    fn from(value: PermissionShareData) -> Self {
        Self {
            lower_positive: value.lower_positive,
            lower_negative: value.lower_negative,
            upper_positive: value.upper_positive,
            upper_negative: value.upper_negative,
        }
    }
}

impl From<PermissionShareDataRecord> for PermissionShareData {
    fn from(value: PermissionShareDataRecord) -> Self {
        Self {
            lower_positive: value.lower_positive,
            lower_negative: value.lower_negative,
            upper_positive: value.upper_positive,
            upper_negative: value.upper_negative,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PermissionShareRecord {
    pub permission_share_id: Uuid,
    pub owner_account_id: Uuid,
    pub target_account_id: Uuid,
    pub name: String,
    pub current_card_id: Option<Uuid>,

    #[sqlx(flatten)]
    pub audit: AuditFields,

    pub current_revision_id: i64,
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PermissionShareRevisionRecord {
    pub permission_share_id: Uuid,
    pub revision_id: i64,
    pub name: String,
    pub card_id: Option<Uuid>,
    pub data: Blob<PermissionShareDataRecord>,

    #[sqlx(flatten)]
    pub audit: DeletableRevisionAuditFields,
}

impl PermissionShareRevisionRecord {
    pub fn creation(
        id: PermissionShareId,
        name: String,
        data: PermissionShareData,
        actor: AccountId,
    ) -> Self {
        Self {
            permission_share_id: id.0,
            revision_id: PermissionShareRevision::INITIAL.into(),
            name,
            card_id: None,
            data: Blob::new(data.into()),
            audit: DeletableRevisionAuditFields::new(actor.0),
        }
    }

    pub fn from_model(value: PermissionShare, audit: DeletableRevisionAuditFields) -> Self {
        Self {
            permission_share_id: value.id.0,
            revision_id: value.revision.into(),
            name: value.name,
            card_id: value.current_card_id,
            data: Blob::new(value.data.into()),
            audit,
        }
    }
}

#[derive(FromRow, Debug, Clone, PartialEq)]
pub struct PermissionShareExtRevisionRecord {
    pub owner_account_id: Uuid,
    pub target_account_id: Uuid,
    pub current_card_id: Option<Uuid>,

    #[sqlx(flatten)]
    pub revision: PermissionShareRevisionRecord,
}

impl TryFrom<PermissionShareExtRevisionRecord> for PermissionShare {
    type Error = PermissionShareRepoError;

    fn try_from(value: PermissionShareExtRevisionRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: PermissionShareId(value.revision.permission_share_id),
            revision: value.revision.revision_id.try_into()?,
            owner_account_id: AccountId(value.owner_account_id),
            target_account_id: AccountId(value.target_account_id),
            name: value.revision.name,
            current_card_id: value.current_card_id,
            data: value.revision.data.into_value().into(),
        })
    }
}
