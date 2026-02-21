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

use crate::repo::model::datetime::SqlDateTime;
use uuid::Uuid;

/// Audit fields for resources that can only be created and deleted, not updated.
#[derive(Debug, Clone, sqlx::FromRow, PartialEq)]
pub struct ImmutableAuditFields {
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
    pub deleted_at: Option<SqlDateTime>,
    pub deleted_by: Option<Uuid>,
}

impl ImmutableAuditFields {
    pub fn new(created_by: Uuid) -> Self {
        Self {
            created_at: SqlDateTime::now(),
            created_by,
            deleted_at: None,
            deleted_by: None,
        }
    }

    pub fn into_deletion(self, deleted_by: Uuid) -> Self {
        Self {
            deleted_at: Some(SqlDateTime::now()),
            deleted_by: Some(deleted_by),
            ..self
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, PartialEq)]
pub struct AuditFields {
    pub created_at: SqlDateTime,
    pub updated_at: SqlDateTime,
    pub deleted_at: Option<SqlDateTime>,
    pub modified_by: Uuid,
}

impl AuditFields {
    pub fn new(modified_by: Uuid) -> Self {
        Self {
            created_at: SqlDateTime::now(),
            updated_at: SqlDateTime::now(),
            deleted_at: None,
            modified_by,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, PartialEq)]
pub struct DeletableRevisionAuditFields {
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
    pub deleted: bool,
}

impl DeletableRevisionAuditFields {
    pub fn new(created_by: Uuid) -> Self {
        Self {
            created_at: SqlDateTime::now(),
            created_by,
            deleted: false,
        }
    }

    pub fn deletion(created_by: Uuid) -> Self {
        Self {
            created_at: SqlDateTime::now(),
            created_by,
            deleted: true,
        }
    }

    pub fn ensure_new(self) -> Self {
        Self {
            deleted: false,
            ..self
        }
    }

    pub fn ensure_deletion(self) -> Self {
        Self {
            deleted: true,
            ..self
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, PartialEq)]
pub struct RevisionAuditFields {
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
}

impl RevisionAuditFields {
    pub fn new(created_by: Uuid) -> Self {
        Self {
            created_at: SqlDateTime::now(),
            created_by,
        }
    }
}
