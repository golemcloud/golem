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

use super::{FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::{DurationValuePayload, SchemaValue};

impl IntoSchema for ::chrono::Duration {
    fn type_id() -> TypeId {
        TypeId::new("chrono.Duration")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::duration()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::Duration(DurationValuePayload {
            nanoseconds: self
                .num_nanoseconds()
                .unwrap_or(self.num_seconds() * 1_000_000_000),
        })
    }
}

impl FromSchema for ::chrono::Duration {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Duration(p) => Ok(::chrono::Duration::nanoseconds(p.nanoseconds)),
            other => Err(FromSchemaError::shape_mismatch(
                "duration",
                value_kind(other),
                "chrono::Duration",
            )),
        }
    }
}

const NAIVE_DATE_FMT: &str = "%Y-%m-%d";
const NAIVE_DATE_TIME_FMT: &str = "%Y-%m-%dT%H:%M:%S%.9f";
const NAIVE_TIME_FMT: &str = "%H:%M:%S%.9f";

impl IntoSchema for ::chrono::NaiveDate {
    fn type_id() -> TypeId {
        TypeId::new("chrono.NaiveDate")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.format(NAIVE_DATE_FMT).to_string())
    }
}

impl FromSchema for ::chrono::NaiveDate {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => ::chrono::NaiveDate::parse_from_str(s, NAIVE_DATE_FMT)
                .map_err(|e| FromSchemaError::custom(format!("invalid NaiveDate: {e}"))),
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "NaiveDate",
            )),
        }
    }
}

impl IntoSchema for ::chrono::NaiveDateTime {
    fn type_id() -> TypeId {
        TypeId::new("chrono.NaiveDateTime")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.format(NAIVE_DATE_TIME_FMT).to_string())
    }
}

impl FromSchema for ::chrono::NaiveDateTime {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => {
                ::chrono::NaiveDateTime::parse_from_str(s, NAIVE_DATE_TIME_FMT)
                    .map_err(|e| FromSchemaError::custom(format!("invalid NaiveDateTime: {e}")))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "NaiveDateTime",
            )),
        }
    }
}

impl IntoSchema for ::chrono::NaiveTime {
    fn type_id() -> TypeId {
        TypeId::new("chrono.NaiveTime")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.format(NAIVE_TIME_FMT).to_string())
    }
}

impl FromSchema for ::chrono::NaiveTime {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => ::chrono::NaiveTime::parse_from_str(s, NAIVE_TIME_FMT)
                .map_err(|e| FromSchemaError::custom(format!("invalid NaiveTime: {e}"))),
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "NaiveTime",
            )),
        }
    }
}
