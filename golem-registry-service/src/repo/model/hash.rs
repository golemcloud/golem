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

use golem_common::model::diff;
use sqlx::Database;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use std::fmt::Display;
use std::ops::Deref;

// TODO: we might want to change this to "SqlModelHash" with internal versioning
/// SqlBlake3Hash is a wrapper around blake3::Hash which allows storing it as BLOB, while
/// making it easy to still use methods of blake3::Hash (e.g., as_hex()) directly
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlBlake3Hash {
    hash: blake3::Hash,
}

impl SqlBlake3Hash {
    pub fn new(hash: blake3::Hash) -> Self {
        Self { hash }
    }

    pub fn empty() -> Self {
        Self {
            // TODO: const?
            hash: blake3::hash(&[]),
        }
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
        std::fmt::Display::fmt(&self.hash, f)
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

impl From<SqlBlake3Hash> for diff::Hash {
    fn from(hash: SqlBlake3Hash) -> Self {
        blake3::Hash::from(hash).into()
    }
}

impl From<diff::Hash> for SqlBlake3Hash {
    fn from(hash: diff::Hash) -> Self {
        hash.into_blake3().into()
    }
}

impl<DB: Database> sqlx::Type<DB> for SqlBlake3Hash
where
    Vec<u8>: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <Vec<u8>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <Vec<u8>>::compatible(ty)
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
