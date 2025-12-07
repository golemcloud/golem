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

use desert_rust::{BinaryDeserializer, BinarySerializer};
use sqlx::Database;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Blob<T> {
    value: T,

    serialized: OnceLock<Vec<u8>>,
}

impl<T> Blob<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            serialized: OnceLock::new(),
        }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }

    pub fn serialize(&self) -> anyhow::Result<&Vec<u8>>
    where
        T: BinarySerializer,
    {
        if let Some(bytes) = self.serialized.get() {
            return Ok(bytes);
        }

        let computed = desert_rust::serialize_to_byte_vec(&self.value)
            .map_err(|e| anyhow::Error::from(e).context("serializing blob failed"))?;
        let _ = self.serialized.set(computed);

        Ok(self.serialized.get().expect("OnceCell was set just above"))
    }

    pub fn deserialze(data: Vec<u8>) -> anyhow::Result<Self>
    where
        T: BinaryDeserializer,
    {
        let value: T = desert_rust::deserialize(&data)
            .map_err(|e| anyhow::Error::from(e).context("deserializing blob failed"))?;

        let deserialized_blob = Self::new(value);
        let _ = deserialized_blob.serialized.set(data.to_vec());

        Ok(deserialized_blob)
    }
}

impl<T: PartialEq> PartialEq for Blob<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq> Eq for Blob<T> {}

impl<T, DB: Database> sqlx::Type<DB> for Blob<T>
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

impl<'q, T: BinarySerializer, DB: Database> sqlx::Encode<'q, DB> for Blob<T>
where
    Vec<u8>: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        let serialized = self.serialize()?;
        serialized.encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        match self.serialize() {
            Ok(bytes) => bytes.size_hint(),
            Err(_) => 0,
        }
    }
}

impl<'r, T: BinaryDeserializer, DB: Database> sqlx::Decode<'r, DB> for Blob<T>
where
    Vec<u8>: sqlx::Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        let bytes = <Vec<u8> as sqlx::Decode<DB>>::decode(value)?;
        let deserialized: Self = Self::deserialze(bytes)?;
        Ok(deserialized)
    }
}
