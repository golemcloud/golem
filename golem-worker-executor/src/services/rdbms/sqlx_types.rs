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

use golem_common::model::oplog::types::{TimeTz, ValuesRange};
use golem_service_base::repo::postgres_temporal::{validate_date, validate_timestamp};
use sqlx::error::BoxDynError;
use sqlx::postgres::PgValueRef;
use sqlx::postgres::types::{PgRange, PgTimeTz};
use sqlx::{Database, Decode, Encode, Type};

/// Keeps SQLx's native types at the database boundary.
#[derive(Clone)]
pub(super) struct SqlValue<T>(pub T);

pub(super) trait SqlType: Sized {
    type Sql;
    fn into_sql(self) -> Result<Self::Sql, BoxDynError>;
    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError>;

    fn decode_postgres(value: PgValueRef<'_>) -> Result<Self, BoxDynError>
    where
        Self::Sql: for<'r> Decode<'r, sqlx::Postgres>,
    {
        Self::from_sql(<Self::Sql as Decode<sqlx::Postgres>>::decode(value)?)
    }
}

impl SqlType for bit_vec::BitVec {
    type Sql = bit_vec_sqlx::BitVec;

    fn into_sql(self) -> Result<Self::Sql, BoxDynError> {
        Ok(self.iter().collect())
    }

    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError> {
        Ok(value.iter().collect())
    }
}

impl<T: SqlType, DB: Database> Type<DB> for SqlValue<T>
where
    T::Sql: Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        T::Sql::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        T::Sql::compatible(ty)
    }
}

impl<'q, T: SqlType + Clone, DB: Database> Encode<'q, DB> for SqlValue<T>
where
    T::Sql: Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut DB::ArgumentBuffer<'q>,
    ) -> Result<sqlx::encode::IsNull, BoxDynError> {
        self.0.clone().into_sql()?.encode(buf)
    }
}

impl<'r, T: SqlType> Decode<'r, sqlx::MySql> for SqlValue<T>
where
    T::Sql: Decode<'r, sqlx::MySql>,
{
    fn decode(value: sqlx::mysql::MySqlValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self(T::from_sql(<T::Sql as Decode<sqlx::MySql>>::decode(
            value,
        )?)?))
    }
}

impl<'r, T: SqlType> Decode<'r, sqlx::Postgres> for SqlValue<T>
where
    T::Sql: for<'a> Decode<'a, sqlx::Postgres>,
{
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        Ok(Self(T::decode_postgres(value)?))
    }
}

impl<T: SqlType> sqlx::postgres::PgHasArrayType for SqlValue<T>
where
    T::Sql: sqlx::postgres::PgHasArrayType,
{
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        T::Sql::array_type_info()
    }

    // SQLx checks a column against this, not against `array_type_info`. Some types accept
    // more than one array type: a bit vector reads both `BIT[]` and `VARBIT[]` columns.
    fn array_compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        T::Sql::array_compatible(ty)
    }
}

impl SqlType for jiff::civil::Date {
    type Sql = time::Date;

    fn decode_postgres(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        validate_date(value.clone())?;
        Self::from_sql(<Self::Sql as Decode<sqlx::Postgres>>::decode(value)?)
    }

    fn into_sql(self) -> Result<Self::Sql, BoxDynError> {
        Ok(time::Date::from_calendar_date(
            i32::from(self.year()),
            (self.month() as u8).try_into()?,
            self.day() as u8,
        )?)
    }

    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError> {
        Ok(Self::new(
            value.year().try_into()?,
            value.month() as i8,
            value.day() as i8,
        )?)
    }
}

impl SqlType for jiff::civil::Time {
    type Sql = time::Time;

    fn into_sql(self) -> Result<Self::Sql, BoxDynError> {
        Ok(time::Time::from_hms_nano(
            self.hour() as u8,
            self.minute() as u8,
            self.second() as u8,
            self.subsec_nanosecond() as u32,
        )?)
    }

    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError> {
        Ok(Self::new(
            value.hour() as i8,
            value.minute() as i8,
            value.second() as i8,
            value.nanosecond() as i32,
        )?)
    }
}

impl SqlType for jiff::civil::DateTime {
    type Sql = time::PrimitiveDateTime;

    fn decode_postgres(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        validate_timestamp(value.clone())?;
        Self::from_sql(<Self::Sql as Decode<sqlx::Postgres>>::decode(value)?)
    }

    fn into_sql(self) -> Result<Self::Sql, BoxDynError> {
        Ok(time::PrimitiveDateTime::new(
            self.date().into_sql()?,
            self.time().into_sql()?,
        ))
    }

    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError> {
        Ok(Self::from_parts(
            jiff::civil::Date::from_sql(value.date())?,
            jiff::civil::Time::from_sql(value.time())?,
        ))
    }
}

impl SqlType for jiff::Timestamp {
    type Sql = time::OffsetDateTime;

    fn decode_postgres(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        validate_timestamp(value.clone())?;
        Self::from_sql(<Self::Sql as Decode<sqlx::Postgres>>::decode(value)?)
    }

    fn into_sql(self) -> Result<Self::Sql, BoxDynError> {
        Ok(time::OffsetDateTime::from_unix_timestamp_nanos(
            self.as_nanosecond(),
        )?)
    }

    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError> {
        Ok(Self::from_nanosecond(value.unix_timestamp_nanos())?)
    }
}

impl<T: SqlType> SqlType for ValuesRange<T>
where
    T::Sql: Type<sqlx::Postgres> + for<'r> Decode<'r, sqlx::Postgres>,
{
    type Sql = PgRange<T::Sql>;

    fn decode_postgres(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        // Decode each bound through the scalar adapter before SQLx can perform temporal arithmetic.
        let range = PgRange::<SqlValue<T>>::decode(value)?;
        Ok(Self {
            start: range.start.map(|v| v.0),
            end: range.end.map(|v| v.0),
        })
    }

    fn into_sql(self) -> Result<Self::Sql, BoxDynError> {
        Ok(self
            .try_map(|v| v.into_sql().map_err(|e| e.to_string()))?
            .into())
    }

    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError> {
        Ok(ValuesRange::from(value).try_map(|v| T::from_sql(v).map_err(|e| e.to_string()))?)
    }
}

impl SqlType for TimeTz {
    type Sql = PgTimeTz<time::Time, time::UtcOffset>;

    fn into_sql(self) -> Result<Self::Sql, BoxDynError> {
        Ok(PgTimeTz {
            time: self.time.into_sql()?,
            offset: time::UtcOffset::from_whole_seconds(
                self.offset
                    .checked_neg()
                    .ok_or("invalid time zone offset")?,
            )?,
        })
    }

    fn from_sql(value: Self::Sql) -> Result<Self, BoxDynError> {
        Ok(Self {
            time: jiff::civil::Time::from_sql(value.time)?,
            offset: -value.offset.whole_seconds(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Bound;
    use test_r::test;

    #[test]
    fn temporal_sql_conversions_preserve_nanoseconds_and_pre_epoch_instants() {
        let date = jiff::civil::date(1969, 12, 31);
        let time = jiff::civil::Time::new(23, 59, 59, 999_999_999).unwrap();
        let datetime = date.to_datetime(time);
        let timestamp = jiff::Timestamp::from_nanosecond(-1).unwrap();
        assert_eq!(
            jiff::civil::Date::from_sql(date.into_sql().unwrap()).unwrap(),
            date
        );
        assert_eq!(
            jiff::civil::Time::from_sql(time.into_sql().unwrap()).unwrap(),
            time
        );
        assert_eq!(
            jiff::civil::DateTime::from_sql(datetime.into_sql().unwrap()).unwrap(),
            datetime
        );
        assert_eq!(
            jiff::Timestamp::from_sql(timestamp.into_sql().unwrap()).unwrap(),
            timestamp
        );
    }

    #[test]
    fn timestamp_sql_conversion_rejects_end_of_time_sentinel() {
        let maximum = time::Date::from_calendar_date(9999, time::Month::December, 30)
            .unwrap()
            .with_hms_nano(22, 0, 0, 999_999_999)
            .unwrap()
            .assume_utc();
        assert_eq!(
            jiff::Timestamp::from_sql(maximum).unwrap(),
            jiff::Timestamp::MAX
        );

        let sentinel = time::Date::from_calendar_date(9999, time::Month::December, 31)
            .unwrap()
            .with_hms(23, 59, 59)
            .unwrap()
            .assume_utc();
        for value in [maximum + time::Duration::nanoseconds(1), sentinel] {
            assert!(
                jiff::Timestamp::from_sql(value).is_err(),
                "out-of-range instant {value} must be rejected"
            );
        }
    }

    #[test]
    fn temporal_sql_ranges_preserve_open_and_unbounded_bounds() {
        let date = jiff::civil::date(2026, 9, 17);
        let range = ValuesRange {
            start: Bound::Excluded(date),
            end: Bound::Unbounded,
        };
        let sql = range.clone().into_sql().unwrap();
        assert!(matches!(sql.start, Bound::Excluded(_)));
        assert!(matches!(sql.end, Bound::Unbounded));
        assert_eq!(
            ValuesRange::<jiff::civil::Date>::from_sql(sql).unwrap(),
            range
        );
    }

    #[test]
    fn temporal_sql_timetz_keeps_the_offset_sign() {
        for offset in [-18_000, 10_800] {
            let value = TimeTz {
                time: jiff::civil::Time::new(10, 20, 30, 123_456_789).unwrap(),
                offset,
            };
            let sql = value.clone().into_sql().unwrap();
            assert_eq!(sql.offset.whole_seconds(), -offset);
            assert_eq!(TimeTz::from_sql(sql).unwrap(), value);
        }
    }

    #[test]
    fn sql_bits_keep_non_byte_aligned_lengths() {
        for bits in [
            vec![],
            vec![true],
            vec![true, false, true, false, false, true, true, false, true],
        ] {
            let value: bit_vec::BitVec = bits.iter().copied().collect();
            let sql = value.clone().into_sql().unwrap();
            assert_eq!(sql.len(), bits.len());
            assert_eq!(sql.iter().collect::<Vec<_>>(), bits);
            assert_eq!(bit_vec::BitVec::from_sql(sql).unwrap(), value);
        }
    }

    #[test]
    fn sql_adapters_keep_postgres_array_and_range_types() {
        assert_eq!(
            <Vec<SqlValue<bit_vec::BitVec>> as Type<sqlx::Postgres>>::type_info(),
            <Vec<bit_vec_sqlx::BitVec> as Type<sqlx::Postgres>>::type_info(),
        );
        assert_eq!(
            <SqlValue<ValuesRange<jiff::civil::Date>> as Type<sqlx::Postgres>>::type_info(),
            <PgRange<time::Date> as Type<sqlx::Postgres>>::type_info(),
        );
        assert_eq!(
            <Vec<SqlValue<ValuesRange<jiff::Timestamp>>> as Type<sqlx::Postgres>>::type_info(),
            <Vec<PgRange<time::OffsetDateTime>> as Type<sqlx::Postgres>>::type_info(),
        );
    }

    #[test]
    fn sql_bit_arrays_accept_bit_and_varbit_columns() {
        // The OIDs of BIT[] and VARBIT[].
        for oid in [1561, 1563] {
            let column = sqlx::postgres::PgTypeInfo::with_oid(sqlx::postgres::types::Oid(oid));
            assert!(
                <Vec<bit_vec_sqlx::BitVec> as Type<sqlx::Postgres>>::compatible(&column),
                "SQLx reads a column with OID {oid} as a bit vector array"
            );
            assert!(
                <Vec<SqlValue<bit_vec::BitVec>> as Type<sqlx::Postgres>>::compatible(&column),
                "the adapter must read a column with OID {oid} as SQLx does"
            );
        }
    }
}
