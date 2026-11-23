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

use std::collections::HashMap;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Bound;
use std::str::FromStr;

use mac_address::MacAddress;
use sqlx::ValueRef;
use sqlx::postgres::PgTypeKind;
use sqlx::postgres::types::{Oid, PgInterval, PgRange, PgTimeTz};

use super::{
    Enumeration, EnumerationType, Interval, SerializableMacAddress, SparseVec, TimeTz, ValuesRange,
};
use crate::schema::conversion::{FromSchemaError, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;

// Schema-native representation mirroring the legacy schema
// above: the MAC address rendered as a string.
impl crate::schema::conversion::IntoSchema for SerializableMacAddress {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.SerializableMacAddress")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.0.to_string())
    }
}

impl crate::schema::conversion::FromSchema for SerializableMacAddress {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => {
                let macaddr = MacAddress::from_str(s)
                    .map_err(|err| FromSchemaError::custom(err.to_string()))?;
                Ok(SerializableMacAddress(macaddr))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "SerializableMacAddress",
            )),
        }
    }
}

impl From<MacAddress> for SerializableMacAddress {
    fn from(value: MacAddress) -> Self {
        SerializableMacAddress(value)
    }
}

impl From<SerializableMacAddress> for MacAddress {
    fn from(value: SerializableMacAddress) -> Self {
        value.0
    }
}

impl<T> ValuesRange<T> {
    pub fn new(start: Bound<T>, end: Bound<T>) -> Self {
        ValuesRange { start, end }
    }

    pub fn start_value(&self) -> Option<&T> {
        match &self.start {
            Bound::Included(v) => Some(v),
            Bound::Excluded(v) => Some(v),
            Bound::Unbounded => None,
        }
    }

    pub fn end_value(&self) -> Option<&T> {
        match &self.end {
            Bound::Included(v) => Some(v),
            Bound::Excluded(v) => Some(v),
            Bound::Unbounded => None,
        }
    }

    pub fn map<U>(self, f: impl Fn(T) -> U + Clone) -> ValuesRange<U> {
        let start: Bound<U> = self.start.map(f.clone());
        let end: Bound<U> = self.end.map(f.clone());
        ValuesRange::new(start, end)
    }

    pub fn try_map<U>(
        self,
        f: impl Fn(T) -> Result<U, String> + Clone,
    ) -> Result<ValuesRange<U>, String> {
        fn to_bound<T, U>(
            v: Bound<T>,
            f: impl Fn(T) -> Result<U, String>,
        ) -> Result<Bound<U>, String> {
            match v {
                Bound::Included(v) => Ok(Bound::Included(f(v)?)),
                Bound::Excluded(v) => Ok(Bound::Excluded(f(v)?)),
                Bound::Unbounded => Ok(Bound::Unbounded),
            }
        }
        let start: Bound<U> = to_bound(self.start, f.clone())?;
        let end: Bound<U> = to_bound(self.end, f.clone())?;

        Ok(ValuesRange::new(start, end))
    }
}

impl<T: Debug> Display for ValuesRange<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} {:?}", self.start, self.end)
    }
}

impl Interval {
    pub fn new(months: i32, days: i32, microseconds: i64) -> Self {
        Interval {
            months,
            days,
            microseconds,
        }
    }
}

impl Display for Interval {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}m {}d {}us", self.months, self.days, self.microseconds)
    }
}

impl TimeTz {
    pub fn new(time: jiff::civil::Time, offset: jiff::tz::Offset) -> Self {
        TimeTz {
            time,
            offset: -offset.seconds(),
        }
    }
}

impl Display for TimeTz {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.time, self.offset)
    }
}

impl Enumeration {
    pub fn new(name: String, value: String) -> Self {
        Enumeration { name, value }
    }
}

impl Display for Enumeration {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
    }
}

impl SparseVec {
    pub fn try_new(dim: i32, indices: Vec<i32>, values: Vec<f32>) -> Result<Self, String> {
        if indices.len() != values.len() {
            Err("Indices and values must have the same length".to_string())
        } else if indices.len() > dim as usize {
            Err("Indices must be less than or equal to dim".to_string())
        } else {
            Ok(SparseVec {
                dim,
                indices,
                values,
            })
        }
    }

    pub fn to_map(&self) -> HashMap<&i32, &f32> {
        let mut map = HashMap::new();
        for (idx, val) in self.indices.iter().zip(self.values.iter()) {
            map.insert(idx, val);
        }
        map
    }
}

impl Display for SparseVec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{")?;
        for (i, (idx, val)) in self.indices.iter().zip(self.values.iter()).enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}:{}", idx, val)?;
        }
        write!(f, "}}/{}", self.dim)
    }
}

impl<T> From<ValuesRange<T>> for PgRange<T> {
    fn from(range: ValuesRange<T>) -> Self {
        PgRange {
            start: range.start,
            end: range.end,
        }
    }
}

impl<T> From<PgRange<T>> for ValuesRange<T> {
    fn from(range: PgRange<T>) -> Self {
        ValuesRange {
            start: range.start,
            end: range.end,
        }
    }
}

impl From<PgInterval> for Interval {
    fn from(interval: PgInterval) -> Self {
        Self {
            months: interval.months,
            days: interval.days,
            microseconds: interval.microseconds,
        }
    }
}

impl From<Interval> for PgInterval {
    fn from(interval: Interval) -> Self {
        Self {
            months: interval.months,
            days: interval.days,
            microseconds: interval.microseconds,
        }
    }
}

impl From<PgTimeTz<time::Time, time::UtcOffset>> for TimeTz {
    fn from(value: PgTimeTz<time::Time, time::UtcOffset>) -> Self {
        Self {
            time: jiff::civil::Time::new(
                value.time.hour() as i8,
                value.time.minute() as i8,
                value.time.second() as i8,
                value.time.nanosecond() as i32,
            )
            .expect("SQL time is representable in Jiff"),
            offset: -value.offset.whole_seconds(),
        }
    }
}

impl TryFrom<TimeTz> for PgTimeTz<time::Time, time::UtcOffset> {
    type Error = String;
    fn try_from(value: TimeTz) -> Result<Self, Self::Error> {
        let offset = value
            .offset
            .checked_neg()
            .filter(|offset| (-86_399..=86_399).contains(offset))
            .and_then(|offset| time::UtcOffset::from_whole_seconds(offset).ok())
            .ok_or("Offset value is not valid")?;
        Ok(Self {
            time: time::Time::from_hms_nano(
                value.time.hour() as u8,
                value.time.minute() as u8,
                value.time.second() as u8,
                value.time.subsec_nanosecond() as u32,
            )
            .map_err(|error| error.to_string())?,
            offset,
        })
    }
}

impl sqlx::types::Type<sqlx::Postgres> for Enumeration {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as sqlx::types::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Enum(_))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Enumeration {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        use sqlx::TypeInfo;

        let type_info = &value.type_info();
        let name = type_info.name().to_string();
        if matches!(type_info.kind(), PgTypeKind::Enum(_)) {
            let v = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
            Ok(Enumeration::new(name, v))
        } else {
            Err(format!("Type '{name}' is not supported").into())
        }
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Enumeration {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.value, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        Some(sqlx::postgres::PgTypeInfo::with_name(
            self.name.clone().leak(),
        ))
    }
}

impl sqlx::postgres::PgHasArrayType for Enumeration {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2277)) // pseudo type array
    }

    fn array_compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Array(ty) if <Enumeration as sqlx::types::Type<sqlx::Postgres>>::compatible(ty))
    }
}

impl EnumerationType {
    pub fn new(name: String) -> Self {
        EnumerationType { name }
    }
}

impl Display for EnumerationType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[cfg(test)]
mod time_tests {
    use super::*;
    use test_r::test;

    #[test]
    fn sql_time_with_offset_preserves_nanoseconds_and_offset_direction() {
        let time = jiff::civil::Time::new(23, 59, 58, 123_456_789).unwrap();
        for seconds in [-86_399, -3_600, 0, 3_600, 86_399] {
            let value = TimeTz::new(time, jiff::tz::Offset::from_seconds(seconds).unwrap());
            assert_eq!(value.offset, -seconds);
            let sql: PgTimeTz<time::Time, time::UtcOffset> = value.clone().try_into().unwrap();
            assert_eq!(sql.offset.whole_seconds(), seconds);
            assert_eq!(TimeTz::from(sql), value);
        }
    }

    #[test]
    fn sql_time_with_offset_rejects_unrepresentable_offset() {
        for offset in [i32::MIN, -86_400, 86_400, i32::MAX] {
            let value = TimeTz {
                time: jiff::civil::Time::MIN,
                offset,
            };
            assert!(PgTimeTz::<time::Time, time::UtcOffset>::try_from(value).is_err());
        }
    }
}
