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

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::WireSchema for ::jiff::SignedDuration {
    fn wire_type_id() -> String {
        <Self as IntoSchema>::type_id().0
    }

    fn append_schema(builder: &mut crate::schema::wit::direct::WireSchemaBuilder) -> i32 {
        builder.push(crate::schema::wit::wire::SchemaTypeBody::DurationType)
    }
}

#[cfg(feature = "guest")]
fn jiff_duration_nanoseconds(
    duration: &::jiff::SignedDuration,
) -> Result<i64, crate::schema::wit::direct::WireError> {
    i64::try_from(duration.as_nanos()).map_err(|_| {
        crate::schema::wit::direct::WireError::Shape("duration within the i64 nanosecond range")
    })
}

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::IntoWire for ::jiff::SignedDuration {
    const NEEDS_PREPARATION: bool = false;

    fn preflight(
        &self,
        _resources: &mut crate::schema::wit::direct::WirePreflight,
    ) -> Result<(), crate::schema::wit::direct::WireError> {
        jiff_duration_nanoseconds(self).map(|_| ())
    }

    fn write_wire(
        &self,
        writer: &mut crate::schema::wit::direct::WireWriter,
    ) -> Result<i32, crate::schema::wit::direct::WireError> {
        Ok(
            writer.push(crate::schema::wit::wire::SchemaValueNode::DurationValue(
                crate::schema::wit::wire::DurationValuePayload {
                    nanoseconds: jiff_duration_nanoseconds(self)?,
                },
            )),
        )
    }
}

#[cfg(feature = "guest")]
impl crate::schema::wit::direct::FromWire for ::jiff::SignedDuration {
    fn read_wire(
        reader: &mut crate::schema::wit::direct::WireReader,
        index: i32,
    ) -> Result<Self, crate::schema::wit::direct::WireError> {
        match reader.take(index)? {
            crate::schema::wit::wire::SchemaValueNode::DurationValue(value) => {
                Ok(Self::from_nanos(value.nanoseconds))
            }
            _ => Err(crate::schema::wit::direct::WireError::Shape("duration")),
        }
    }
}

impl IntoSchema for ::jiff::SignedDuration {
    fn type_id() -> TypeId {
        TypeId::new("jiff.SignedDuration")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::duration()
    }
    // `to_value` cannot fail, so a duration outside the i64 nanosecond range
    // saturates, like `std::time::Duration`. The wire encoder rejects it.
    fn to_value(&self) -> SchemaValue {
        let nanoseconds = i64::try_from(self.as_nanos()).unwrap_or(if self.is_negative() {
            i64::MIN
        } else {
            i64::MAX
        });
        SchemaValue::Duration(DurationValuePayload { nanoseconds })
    }
}

impl FromSchema for ::jiff::SignedDuration {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Duration(p) => Ok(::jiff::SignedDuration::from_nanos(p.nanoseconds)),
            other => Err(FromSchemaError::shape_mismatch(
                "duration",
                value_kind(other),
                "jiff::SignedDuration",
            )),
        }
    }
}

fn parse_date(value: &str) -> Result<::jiff::civil::Date, String> {
    let date = value
        .parse::<::jiff::civil::Date>()
        .map_err(|error| error.to_string())?;
    if date.to_string() != value {
        return Err("expected a date without time, offset or annotations".into());
    }
    Ok(date)
}

fn parse_time(value: &str) -> Result<::jiff::civil::Time, String> {
    let bytes = value.as_bytes();
    let valid_seconds = bytes.len() >= 8
        && bytes[..2].iter().all(u8::is_ascii_digit)
        && bytes[2] == b':'
        && bytes[3..5].iter().all(u8::is_ascii_digit)
        && bytes[5] == b':'
        && bytes[6..8].iter().all(u8::is_ascii_digit)
        && &bytes[6..8] != b"60";
    let valid_fraction = bytes.len() == 8
        || ((10..=18).contains(&bytes.len())
            && bytes[8] == b'.'
            && bytes[9..].iter().all(u8::is_ascii_digit));
    if !valid_seconds || !valid_fraction {
        return Err("expected HH:MM:SS with optional nanoseconds and no leap second".into());
    }
    value
        .parse::<::jiff::civil::Time>()
        .map_err(|error| error.to_string())
}

fn parse_datetime(value: &str) -> Result<::jiff::civil::DateTime, String> {
    let (date, time) = value
        .split_once('T')
        .ok_or_else(|| "expected a date and time separated by T".to_string())?;
    Ok(parse_date(date)?.to_datetime(parse_time(time)?))
}

#[cfg(feature = "guest")]
crate::schema::wit::direct::wire_string!(
    ::jiff::civil::Date,
    |value: &::jiff::civil::Date| value.to_string(),
    parse_date
);
#[cfg(feature = "guest")]
crate::schema::wit::direct::wire_string!(
    ::jiff::civil::DateTime,
    |value: &::jiff::civil::DateTime| format!("{value:.9}"),
    parse_datetime
);
#[cfg(feature = "guest")]
crate::schema::wit::direct::wire_string!(
    ::jiff::civil::Time,
    |value: &::jiff::civil::Time| format!("{value:.9}"),
    parse_time
);

impl IntoSchema for ::jiff::civil::Date {
    fn type_id() -> TypeId {
        TypeId::new("jiff.civil.Date")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.to_string())
    }
}

impl FromSchema for ::jiff::civil::Date {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => {
                parse_date(s).map_err(|e| FromSchemaError::custom(format!("invalid Date: {e}")))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "Date",
            )),
        }
    }
}

impl IntoSchema for ::jiff::civil::DateTime {
    fn type_id() -> TypeId {
        TypeId::new("jiff.civil.DateTime")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(format!("{self:.9}"))
    }
}

impl FromSchema for ::jiff::civil::DateTime {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => parse_datetime(s)
                .map_err(|e| FromSchemaError::custom(format!("invalid DateTime: {e}"))),
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "DateTime",
            )),
        }
    }
}

impl IntoSchema for ::jiff::civil::Time {
    fn type_id() -> TypeId {
        TypeId::new("jiff.civil.Time")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(format!("{self:.9}"))
    }
}

impl FromSchema for ::jiff::civil::Time {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => {
                parse_time(s).map_err(|e| FromSchemaError::custom(format!("invalid Time: {e}")))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "Time",
            )),
        }
    }
}
