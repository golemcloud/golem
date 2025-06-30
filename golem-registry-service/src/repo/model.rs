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

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::query::QueryAs;
use sqlx::Database;
use std::fmt::Display;
use std::ops::{Deref, Sub};
use uuid::Uuid;

/// SqlDateTime is a wrapper around DateTime<Utc> which allows storing it as NaiveDateTime
/// (e.g. TIMESTAMP WITHOUT TIME ZONE for Postgres). Doing so allows us to use the same queries
/// for Sqlite and Postgres without any custom casting for the specific database, while still
/// using UTC times in our Repo Record types.
///
/// Another feature provided by SqlDateTime is that it defines PartialEq with some small difference
/// allowed. This is useful for writing repo tests in cases where the resolution
/// of timestamps are different between the OS (used by Rust) and the DB timestamp type.
///
/// Note that above means that SqlDateTime MUST NOT implement Eq.
#[derive(Debug, Clone, PartialOrd)]
pub struct SqlDateTime {
    utc: DateTime<Utc>,
}

impl SqlDateTime {
    pub fn new(utc: DateTime<Utc>) -> Self {
        Self { utc }
    }

    pub fn now() -> Self {
        Utc::now().into()
    }

    pub fn as_utc(&self) -> &DateTime<Utc> {
        &self.utc
    }

    pub fn into_utc(self) -> DateTime<Utc> {
        self.utc
    }
}

impl PartialEq<SqlDateTime> for SqlDateTime {
    fn eq(&self, other: &SqlDateTime) -> bool {
        self.utc.sub(other.utc).abs() < TimeDelta::milliseconds(1)
    }
}

impl Display for SqlDateTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.utc.fmt(f)
    }
}

impl Deref for SqlDateTime {
    type Target = DateTime<Utc>;

    fn deref(&self) -> &Self::Target {
        self.as_utc()
    }
}

impl From<DateTime<Utc>> for SqlDateTime {
    fn from(utc: DateTime<Utc>) -> Self {
        Self::new(utc)
    }
}

impl From<SqlDateTime> for DateTime<Utc> {
    fn from(sql_dt: SqlDateTime) -> Self {
        sql_dt.utc
    }
}

impl<DB: Database> sqlx::Type<DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <NaiveDateTime as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <NaiveDateTime as sqlx::Type<DB>>::compatible(ty)
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Encode<'q, DB>,
{
    fn encode(self, buf: &mut <DB as Database>::ArgumentBuffer<'q>) -> Result<IsNull, BoxDynError>
    where
        Self: Sized,
    {
        self.utc.naive_utc().encode(buf)
    }

    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        self.utc.naive_utc().encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        self.utc.naive_utc().size_hint()
    }
}

impl<'r, DB: Database> sqlx::Decode<'r, DB> for SqlDateTime
where
    NaiveDateTime: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self {
            utc: NaiveDateTime::decode(value)?.and_utc(),
        })
    }
}

// TODO: we might want to change this to "SqlModelHash" with internal versioning
/// SqlBlake3Hash is a wrapper around blake3::Hash which allows storing it as BLOB, while
/// making it easy to still use directly methods of blake3::Hash (e.g. as_hex())
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlBlake3Hash {
    hash: blake3::Hash,
}

impl SqlBlake3Hash {
    pub fn new(hash: blake3::Hash) -> Self {
        Self { hash }
    }

    pub fn as_blake3_hash(&self) -> &blake3::Hash {
        &self.hash
    }

    pub fn into_blake3_hash(self) -> blake3::Hash {
        self.hash
    }
}

impl Display for SqlBlake3Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.hash.fmt(f)
    }
}

impl Deref for SqlBlake3Hash {
    type Target = blake3::Hash;

    fn deref(&self) -> &Self::Target {
        &self.hash
    }
}

impl From<SqlBlake3Hash> for blake3::Hash {
    fn from(hash: SqlBlake3Hash) -> Self {
        hash.hash
    }
}

impl From<blake3::Hash> for SqlBlake3Hash {
    fn from(hash: blake3::Hash) -> Self {
        Self { hash }
    }
}

impl<DB: Database> sqlx::Type<DB> for SqlBlake3Hash
where
    Vec<u8>: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <Vec<u8> as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <Vec<u8> as sqlx::Type<DB>>::compatible(ty)
    }
}

impl<'q, DB: Database> sqlx::Encode<'q, DB> for SqlBlake3Hash
where
    Vec<u8>: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        self.hash.as_bytes().to_vec().encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        blake3::OUT_LEN
    }
}

impl<'r, DB: Database> sqlx::Decode<'r, DB> for SqlBlake3Hash
where
    Vec<u8>: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self {
            hash: blake3::Hash::from_slice(<Vec<u8>>::decode(value)?.as_slice())?,
        })
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
pub struct RevisionAuditFields {
    pub created_at: SqlDateTime,
    pub created_by: Uuid,
    pub deleted: bool,
}

impl RevisionAuditFields {
    pub fn new(created_by: Uuid) -> Self {
        Self {
            created_at: SqlDateTime::now(),
            created_by,
            deleted: false,
        }
    }
}

/// BindFields is used to extract binding of common field sets
pub trait BindFields {
    fn bind_audit_fields(self, entity_audit_fields: AuditFields) -> Self;
    fn bind_revision_audit_fields(self, entity_revision_audit_fields: RevisionAuditFields) -> Self;
}

impl<'q, DB: Database, O> BindFields for QueryAs<'q, DB, O, <DB as Database>::Arguments<'q>>
where
    NaiveDateTime: sqlx::Encode<'q, DB>,
    NaiveDateTime: sqlx::Type<DB>,
    Option<SqlDateTime>: sqlx::Encode<'q, DB>,
    Uuid: sqlx::Encode<'q, DB>,
    Uuid: sqlx::Type<DB>,
    bool: sqlx::Encode<'q, DB>,
    bool: sqlx::Type<DB>,
{
    fn bind_audit_fields(self, entity_audit_fields: AuditFields) -> Self {
        self.bind(entity_audit_fields.created_at)
            .bind(entity_audit_fields.updated_at)
            .bind(entity_audit_fields.deleted_at)
            .bind(entity_audit_fields.modified_by)
    }

    fn bind_revision_audit_fields(self, entity_revision_audit_fields: RevisionAuditFields) -> Self {
        self.bind(entity_revision_audit_fields.created_at)
            .bind(entity_revision_audit_fields.created_by)
            .bind(entity_revision_audit_fields.deleted)
    }
}
