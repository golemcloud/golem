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

use anyhow::anyhow;
use bigdecimal::ToPrimitive;
use sqlx::Database;
use sqlx::TypeInfo;
use sqlx::ValueRef;
use sqlx::decode::Decode;
use sqlx::encode::{Encode, IsNull};
use sqlx::error::BoxDynError;
use sqlx::sqlite::SqliteValueRef;
use sqlx::types::BigDecimal;
use std::fmt;
use std::str::FromStr;

/// A `u64` that can be stored in NUMERIC columns, full range allowed
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NumericU64(u64);

impl NumericU64 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn get(&self) -> u64 {
        self.0
    }
}

impl From<u64> for NumericU64 {
    fn from(v: u64) -> Self {
        Self::new(v)
    }
}

impl From<NumericU64> for u64 {
    fn from(v: NumericU64) -> Self {
        v.0
    }
}

impl fmt::Display for NumericU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl sqlx::Type<sqlx::Postgres> for NumericU64 {
    fn type_info() -> <sqlx::Postgres as Database>::TypeInfo {
        <BigDecimal as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &<sqlx::Postgres as Database>::TypeInfo) -> bool {
        <BigDecimal as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'q> Encode<'q, sqlx::Postgres> for NumericU64
where
    BigDecimal: sqlx::Encode<'q, sqlx::Postgres>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <sqlx::Postgres as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        let bd = BigDecimal::from(self.0);
        bd.encode_by_ref(buf)
    }

    fn size_hint(&self) -> usize {
        8
    }
}

impl<'r> Decode<'r, sqlx::Postgres> for NumericU64 {
    fn decode(value: <sqlx::Postgres as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        let bd: BigDecimal = Decode::<sqlx::Postgres>::decode(value)?;
        let u = bd.to_u64().ok_or("value out of u64 range")?;
        Ok(NumericU64::new(u))
    }
}

impl sqlx::Type<sqlx::Sqlite> for NumericU64 {
    fn type_info() -> <sqlx::Sqlite as Database>::TypeInfo {
        <String as sqlx::Type<sqlx::Sqlite>>::type_info()
    }

    fn compatible(ty: &<sqlx::Sqlite as Database>::TypeInfo) -> bool {
        <f64 as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            || <i64 as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            || <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
    }
}

impl<'q> Encode<'q, sqlx::Sqlite> for NumericU64
where
    i64: sqlx::Encode<'q, sqlx::Sqlite>,
    String: sqlx::Encode<'q, sqlx::Sqlite>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <sqlx::Sqlite as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, Box<dyn std::error::Error + Send + Sync>> {
        if self.0 <= i64::MAX as u64 {
            let encoded = self.0 as i64;
            encoded.encode_by_ref(buf)
        } else {
            let encoded = self.0.to_string();
            encoded.encode_by_ref(buf)
        }
    }

    fn size_hint(&self) -> usize {
        if self.0 <= i64::MAX as u64 {
            8
        } else {
            self.0.to_string().len()
        }
    }
}

impl<'r> Decode<'r, sqlx::Sqlite> for NumericU64
where
    i64: sqlx::Decode<'r, sqlx::Sqlite>,
    String: sqlx::Decode<'r, sqlx::Sqlite>,
{
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match value.type_info().name() {
            "REAL" => {
                let f: f64 = Decode::<sqlx::Sqlite>::decode(value)?;

                if f < 0.0 {
                    Err(anyhow!("Negative values not supported for NumericU64: {f}"))?
                }

                if f.fract() != 0.0 {
                    Err(anyhow!(
                        "Fractional REAL values cannot be decoded into NumericU64: {f}"
                    ))?
                }

                let u = f as u64;

                if (u as f64) != f {
                    Err(anyhow!(
                        "REAL value {f} cannot be safely represented as u64"
                    ))?
                }

                Ok(NumericU64(u))
            }
            "INTEGER" => {
                let i: i64 = Decode::<sqlx::Sqlite>::decode(value)?;
                if i < 0 {
                    Err(anyhow!("Negative values not supported for NumericU64: {i}"))?
                }
                Ok(NumericU64(i as u64))
            }
            "TEXT" => {
                let s: String = Decode::<sqlx::Sqlite>::decode(value)?;
                let u = u64::from_str(&s)?;
                Ok(NumericU64(u))
            }
            other => Err(anyhow!("Unsupported type for NumericU64: {other}"))?,
        }
    }
}
