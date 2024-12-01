// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod mysql;
pub mod postgres;

pub(crate) mod utils {
    use chrono::{Datelike, Offset, Timelike};

    type Date = (i32, u8, u8); // year, month, day
    type Time = (u8, u8, u8, u32); // hour, minute, second, nanosecond
    type Timetz = (u8, u8, u8, u32, i32); // hour, minute, second, nanosecond, timezone offset in seconds
    type Timestamp = (i32, u8, u8, u8, u8, u8, u32); // year, month, day, hour, minute, second, nanosecond
    type Timestamptz = (i32, u8, u8, u8, u8, u8, u32, i32); // year, month, day, hour, minute, second, nanosecond, timezone offset in seconds

    pub(crate) fn time_to_nativetime(value: Time) -> Result<chrono::NaiveTime, String> {
        let (hour, minute, second, nanosecond) = value;

        let time = chrono::NaiveTime::from_hms_nano_opt(
            hour as u32,
            minute as u32,
            second as u32,
            nanosecond,
        )
        .ok_or("Time value is not valid")?;
        Ok(time)
    }

    pub(crate) fn timetz_to_nativetime_and_offset(
        value: Timetz,
    ) -> Result<(chrono::NaiveTime, chrono::FixedOffset), String> {
        let (hour, minute, second, nanosecond, offset) = value;
        let time = chrono::NaiveTime::from_hms_nano_opt(
            hour as u32,
            minute as u32,
            second as u32,
            nanosecond,
        )
        .ok_or("Time value is not valid")?;
        let offset =
            chrono::offset::FixedOffset::west_opt(offset).ok_or("Offset value is not valid")?;
        Ok((time, offset))
    }

    pub(crate) fn date_to_nativedate(value: Date) -> Result<chrono::NaiveDate, String> {
        let (year, month, day) = value;
        let date = chrono::naive::NaiveDate::from_ymd_opt(year, month as u32, day as u32)
            .ok_or("Date value is not valid")?;
        Ok(date)
    }

    pub(crate) fn timestamp_to_datetime(
        value: Timestamp,
    ) -> Result<chrono::DateTime<chrono::Utc>, String> {
        let (year, month, day, hour, minute, second, nanosecond) = value;
        let date = chrono::naive::NaiveDate::from_ymd_opt(year, month as u32, day as u32)
            .ok_or("Date value is not valid")?;
        let time = chrono::NaiveTime::from_hms_nano_opt(
            hour as u32,
            minute as u32,
            second as u32,
            nanosecond,
        )
        .ok_or("Time value is not valid")?;
        Ok(chrono::naive::NaiveDateTime::new(date, time).and_utc())
    }

    pub(crate) fn timestamptz_to_datetime(
        value: Timestamptz,
    ) -> Result<chrono::DateTime<chrono::Utc>, String> {
        let (year, month, day, hour, minute, second, nanosecond, offset) = value;
        let date = chrono::naive::NaiveDate::from_ymd_opt(year, month as u32, day as u32)
            .ok_or("Date value is not valid")?;
        let time = chrono::NaiveTime::from_hms_nano_opt(
            hour as u32,
            minute as u32,
            second as u32,
            nanosecond,
        )
        .ok_or("Time value is not valid")?;
        let offset =
            chrono::offset::FixedOffset::west_opt(offset).ok_or("Offset value is not valid")?;
        let datetime = chrono::naive::NaiveDateTime::new(date, time)
            .checked_add_offset(offset)
            .ok_or("Offset value is not valid")?;
        Ok(datetime.and_utc())
    }

    pub(crate) fn naivetime_and_offset_to_time(v: chrono::NaiveTime, o: chrono::FixedOffset) -> Timetz {
        let hour = v.hour() as u8;
        let minute = v.minute() as u8;
        let second = v.second() as u8;
        let nanosecond = v.nanosecond();
        let offset = o.local_minus_utc();
        (hour, minute, second, nanosecond, offset)
    }

    pub(crate) fn naivetime_to_time(v: chrono::NaiveTime) -> Time {
        let hour = v.hour() as u8;
        let minute = v.minute() as u8;
        let second = v.second() as u8;
        let nanosecond = v.nanosecond();
        (hour, minute, second, nanosecond)
    }

    pub(crate) fn naivedate_to_date(v: chrono::NaiveDate) -> Date {
        let year = v.year();
        let month = v.month() as u8;
        let day = v.day() as u8;
        (year, month, day)
    }

    pub(crate) fn datetime_to_timestamp(v: chrono::DateTime<chrono::Utc>) -> Timestamp {
        let year = v.date_naive().year();
        let month = v.date_naive().month() as u8;
        let day = v.date_naive().day() as u8;
        let hour = v.time().hour() as u8;
        let minute = v.time().minute() as u8;
        let second = v.time().second() as u8;
        let nanosecond = v.time().nanosecond();
        (year, month, day, hour, minute, second, nanosecond)
    }

    pub(crate) fn datetime_to_timestamptz(v: chrono::DateTime<chrono::Utc>) -> Timestamptz {
        let year = v.date_naive().year();
        let month = v.date_naive().month() as u8;
        let day = v.date_naive().day() as u8;
        let hour = v.time().hour() as u8;
        let minute = v.time().minute() as u8;
        let second = v.time().second() as u8;
        let nanosecond = v.time().nanosecond();
        let offset = v.offset().fix().local_minus_utc();
        (year, month, day, hour, minute, second, nanosecond, offset)
    }
}
