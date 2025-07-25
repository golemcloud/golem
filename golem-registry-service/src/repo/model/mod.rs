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

pub mod account;
pub mod application;
pub mod audit;
pub mod component;
pub mod datetime;
pub mod deployment;
pub mod environment;
pub mod hash;
pub mod http_api_definition;
pub mod http_api_deployment;
pub mod plan;

use crate::repo::model::audit::{AuditFields, DeletableRevisionAuditFields, RevisionAuditFields};
use crate::repo::model::datetime::SqlDateTime;
use chrono::NaiveDateTime;
use sqlx::query::{Query, QueryAs};
use sqlx::Database;
use uuid::Uuid;

// TODO: typed revision_id and entity UUIDs

/// BindFields is used to extract binding of common field sets, e.g., audit fields
pub trait BindFields {
    fn bind_audit(self, entity_audit_fields: AuditFields) -> Self;
    fn bind_revision_audit(self, field: RevisionAuditFields) -> Self;
    fn bind_deletable_revision_audit(
        self,
        entity_revision_audit_fields: DeletableRevisionAuditFields,
    ) -> Self;
}

impl<'q, DB: Database, O> BindFields for QueryAs<'q, DB, O, <DB as Database>::Arguments<'q>>
where
    NaiveDateTime: sqlx::Type<DB> + sqlx::Encode<'q, DB>,
    Uuid: sqlx::Type<DB> + sqlx::Encode<'q, DB>,
    bool: sqlx::Type<DB> + sqlx::Encode<'q, DB>,
    Option<SqlDateTime>: sqlx::Encode<'q, DB>,
{
    fn bind_audit(self, entity_audit_fields: AuditFields) -> Self {
        self.bind(entity_audit_fields.created_at)
            .bind(entity_audit_fields.updated_at)
            .bind(entity_audit_fields.deleted_at)
            .bind(entity_audit_fields.modified_by)
    }

    fn bind_revision_audit(self, field: RevisionAuditFields) -> Self {
        self.bind(field.created_at).bind(field.created_by)
    }

    fn bind_deletable_revision_audit(
        self,
        entity_revision_audit_fields: DeletableRevisionAuditFields,
    ) -> Self {
        self.bind(entity_revision_audit_fields.created_at)
            .bind(entity_revision_audit_fields.created_by)
            .bind(entity_revision_audit_fields.deleted)
    }
}

impl<'q, DB: Database> BindFields for Query<'q, DB, <DB as Database>::Arguments<'q>>
where
    NaiveDateTime: sqlx::Type<DB> + sqlx::Encode<'q, DB>,
    Uuid: sqlx::Type<DB> + sqlx::Encode<'q, DB>,
    bool: sqlx::Type<DB> + sqlx::Encode<'q, DB>,
    Option<SqlDateTime>: sqlx::Encode<'q, DB>,
{
    fn bind_audit(self, fields: AuditFields) -> Self {
        self.bind(fields.created_at)
            .bind(fields.updated_at)
            .bind(fields.deleted_at)
            .bind(fields.modified_by)
    }

    fn bind_revision_audit(self, field: RevisionAuditFields) -> Self {
        self.bind(field.created_at).bind(field.created_by)
    }

    fn bind_deletable_revision_audit(self, fields: DeletableRevisionAuditFields) -> Self {
        self.bind(fields.created_at)
            .bind(fields.created_by)
            .bind(fields.deleted)
    }
}
