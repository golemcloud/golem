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

use bigdecimal::BigDecimal;
use chrono::{Duration, NaiveTime, TimeZone, Timelike, Utc};
use golem_common::model::oplog::payload::types::SerializableDbValue;
use golem_common::model::oplog::types::{
    SerializableDbColumn, SerializableDbColumnType, SerializableDbColumnTypeNode,
    SerializableDbValueNode,
};
use std::fmt::Display;
use uuid::Uuid;

// ── Column ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbColumn {
    pub ordinal: usize,
    pub name: String,
}

impl DbColumn {
    pub fn new(ordinal: usize, name: String) -> Self {
        Self { ordinal, name }
    }
}

impl Display for DbColumn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl From<DbColumn> for SerializableDbColumn {
    fn from(value: DbColumn) -> Self {
        SerializableDbColumn {
            ordinal: value.ordinal as u64,
            name: value.name,
            db_type: SerializableDbColumnType {
                nodes: vec![SerializableDbColumnTypeNode::Text],
            },
            db_type_name: String::new(),
        }
    }
}

impl TryFrom<SerializableDbColumn> for DbColumn {
    type Error = String;

    fn try_from(value: SerializableDbColumn) -> Result<Self, Self::Error> {
        Ok(DbColumn {
            ordinal: value.ordinal as usize,
            name: value.name,
        })
    }
}

// ── Value ─────────────────────────────────────────────────────────────────────

/// Service-layer representation of Apache Ignite value types.
/// Maps 1-to-1 to `ignite_client::IgniteValue`.
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    Null,
    Boolean(bool),
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    /// 16-bit Unicode code unit (Java `char`).
    Char(u16),
    String(String),
    Uuid(Uuid),
    /// Days since Unix epoch (UTC midnight), stored as ms in the wire format.
    Date(i64),
    /// (milliseconds since epoch, sub-millisecond nanoseconds 0..999_999).
    Timestamp(i64, i32),
    /// Milliseconds since midnight.
    Time(i64),
    Decimal(BigDecimal),
    ByteArray(Vec<u8>),
}

impl DbValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            DbValue::Null => "NULL",
            DbValue::Boolean(_) => "BOOLEAN",
            DbValue::Byte(_) => "TINYINT",
            DbValue::Short(_) => "SMALLINT",
            DbValue::Int(_) => "INT",
            DbValue::Long(_) => "BIGINT",
            DbValue::Float(_) => "FLOAT",
            DbValue::Double(_) => "DOUBLE",
            DbValue::Char(_) => "CHAR",
            DbValue::String(_) => "VARCHAR",
            DbValue::Uuid(_) => "UUID",
            DbValue::Date(_) => "DATE",
            DbValue::Timestamp(..) => "TIMESTAMP",
            DbValue::Time(_) => "TIME",
            DbValue::Decimal(_) => "DECIMAL",
            DbValue::ByteArray(_) => "BINARY",
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let DbValue::Boolean(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_byte(&self) -> Option<i8> {
        if let DbValue::Byte(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_short(&self) -> Option<i16> {
        if let DbValue::Short(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_int(&self) -> Option<i32> {
        if let DbValue::Int(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_long(&self) -> Option<i64> {
        if let DbValue::Long(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_float(&self) -> Option<f32> {
        if let DbValue::Float(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_double(&self) -> Option<f64> {
        if let DbValue::Double(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_char(&self) -> Option<u16> {
        if let DbValue::Char(v) = self {
            Some(*v)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let DbValue::String(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_uuid(&self) -> Option<&Uuid> {
        if let DbValue::Uuid(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_date_ms(&self) -> Option<i64> {
        if let DbValue::Date(ms) = self {
            Some(*ms)
        } else {
            None
        }
    }
    pub fn as_timestamp(&self) -> Option<(i64, i32)> {
        if let DbValue::Timestamp(ms, ns) = self {
            Some((*ms, *ns))
        } else {
            None
        }
    }
    pub fn as_time_ms(&self) -> Option<i64> {
        if let DbValue::Time(ms) = self {
            Some(*ms)
        } else {
            None
        }
    }
    pub fn as_decimal(&self) -> Option<&BigDecimal> {
        if let DbValue::Decimal(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let DbValue::ByteArray(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, DbValue::Null)
    }
}

impl Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbValue::Null => write!(f, "NULL"),
            DbValue::Boolean(v) => write!(f, "{v}"),
            DbValue::Byte(v) => write!(f, "{v}"),
            DbValue::Short(v) => write!(f, "{v}"),
            DbValue::Int(v) => write!(f, "{v}"),
            DbValue::Long(v) => write!(f, "{v}"),
            DbValue::Float(v) => write!(f, "{v}"),
            DbValue::Double(v) => write!(f, "{v}"),
            DbValue::Char(v) => write!(f, "{}", char::from_u32(*v as u32).unwrap_or('\u{FFFD}')),
            DbValue::String(v) => write!(f, "{v}"),
            DbValue::Uuid(v) => write!(f, "{v}"),
            DbValue::Date(ms) => {
                let date = chrono::DateTime::from_timestamp_millis(*ms)
                    .map(|dt| dt.date_naive())
                    .unwrap_or_default();
                write!(f, "{date}")
            }
            DbValue::Timestamp(ms, ns) => write!(f, "{ms}ms+{ns}ns"),
            DbValue::Time(v) => write!(f, "{v}ms"),
            DbValue::Decimal(v) => write!(f, "{v}"),
            DbValue::ByteArray(v) => write!(f, "bytes({})", v.len()),
        }
    }
}

// ── Serialization: DbValue ↔ SerializableDbValue ──────────────────────────────

impl From<DbValue> for SerializableDbValue {
    fn from(value: DbValue) -> Self {
        let node = match value {
            DbValue::Null => SerializableDbValueNode::Null,
            DbValue::Boolean(v) => SerializableDbValueNode::Boolean(v),
            DbValue::Byte(v) => SerializableDbValueNode::Tinyint(v),
            DbValue::Short(v) => SerializableDbValueNode::Smallint(v),
            DbValue::Int(v) => SerializableDbValueNode::Int(v),
            DbValue::Long(v) => SerializableDbValueNode::Bigint(v),
            DbValue::Float(v) => SerializableDbValueNode::Float(v),
            DbValue::Double(v) => SerializableDbValueNode::Double(v),
            DbValue::Char(v) => {
                let ch = char::from_u32(v as u32).unwrap_or('\u{FFFD}');
                SerializableDbValueNode::Varchar(ch.to_string())
            }
            DbValue::String(v) => SerializableDbValueNode::Text(v),
            DbValue::Uuid(v) => SerializableDbValueNode::Uuid(v),
            DbValue::Date(ms) => {
                let date = chrono::DateTime::from_timestamp_millis(ms)
                    .map(|dt| dt.date_naive())
                    .unwrap_or_default();
                SerializableDbValueNode::Date(date)
            }
            DbValue::Timestamp(ms, nanos) => {
                let dt = Utc.timestamp_millis_opt(ms).single().unwrap_or_default();
                // Combine ms-level dt with sub-ms nanos
                let dt_with_ns = dt + Duration::nanoseconds(nanos as i64);
                SerializableDbValueNode::Timestamptz(dt_with_ns)
            }
            DbValue::Time(ms) => {
                let total_nanos = ms as u64 * 1_000_000;
                let h = (total_nanos / 3_600_000_000_000) as u32;
                let m = ((total_nanos % 3_600_000_000_000) / 60_000_000_000) as u32;
                let s = ((total_nanos % 60_000_000_000) / 1_000_000_000) as u32;
                let ns = (total_nanos % 1_000_000_000) as u32;
                SerializableDbValueNode::Time(
                    NaiveTime::from_hms_nano_opt(h, m, s, ns).unwrap_or_default(),
                )
            }
            DbValue::Decimal(v) => SerializableDbValueNode::Decimal(v),
            DbValue::ByteArray(v) => SerializableDbValueNode::Bytea(v),
        };
        SerializableDbValue { nodes: vec![node] }
    }
}

impl TryFrom<SerializableDbValue> for DbValue {
    type Error = String;

    fn try_from(value: SerializableDbValue) -> Result<Self, Self::Error> {
        let node = value
            .nodes
            .into_iter()
            .next()
            .ok_or_else(|| "empty SerializableDbValue".to_string())?;
        let v = match node {
            SerializableDbValueNode::Null => DbValue::Null,
            SerializableDbValueNode::Boolean(v) => DbValue::Boolean(v),
            SerializableDbValueNode::Tinyint(v) => DbValue::Byte(v),
            SerializableDbValueNode::Smallint(v) => DbValue::Short(v),
            SerializableDbValueNode::Int(v) | SerializableDbValueNode::Mediumint(v) => {
                DbValue::Int(v)
            }
            SerializableDbValueNode::Bigint(v) => DbValue::Long(v),
            SerializableDbValueNode::Float(v) => DbValue::Float(v),
            SerializableDbValueNode::Double(v) => DbValue::Double(v),
            SerializableDbValueNode::Varchar(v) => {
                // Stored as single-char string; recover the u16 value
                let ch = v.chars().next().unwrap_or('\u{FFFD}');
                DbValue::Char(ch as u16)
            }
            SerializableDbValueNode::Text(v)
            | SerializableDbValueNode::Bpchar(v)
            | SerializableDbValueNode::Tinytext(v)
            | SerializableDbValueNode::Mediumtext(v)
            | SerializableDbValueNode::Longtext(v) => DbValue::String(v),
            SerializableDbValueNode::Uuid(v) => DbValue::Uuid(v),
            SerializableDbValueNode::Date(v) => {
                let ms = v
                    .and_hms_opt(0, 0, 0)
                    .map(|dt| dt.and_utc().timestamp_millis())
                    .unwrap_or(0);
                DbValue::Date(ms)
            }
            SerializableDbValueNode::Timestamp(v) => {
                let ms = v.and_utc().timestamp_millis();
                DbValue::Timestamp(ms, 0)
            }
            SerializableDbValueNode::Timestamptz(v) => {
                let ms = v.timestamp_millis();
                let nanos = (v.timestamp_nanos_opt().unwrap_or_default() - ms * 1_000_000) as i32;
                DbValue::Timestamp(ms, nanos)
            }
            SerializableDbValueNode::Time(v) => {
                let ns =
                    v.num_seconds_from_midnight() as i64 * 1_000_000_000 + v.nanosecond() as i64;
                let ms = ns / 1_000_000;
                DbValue::Time(ms)
            }
            SerializableDbValueNode::Decimal(v) => DbValue::Decimal(v),
            SerializableDbValueNode::Bytea(v)
            | SerializableDbValueNode::Binary(v)
            | SerializableDbValueNode::Varbinary(v)
            | SerializableDbValueNode::Blob(v)
            | SerializableDbValueNode::Tinyblob(v)
            | SerializableDbValueNode::Mediumblob(v)
            | SerializableDbValueNode::Longblob(v) => DbValue::ByteArray(v),
            other => {
                return Err(format!(
                    "unsupported SerializableDbValueNode for Ignite: {other:?}"
                ));
            }
        };
        Ok(v)
    }
}
