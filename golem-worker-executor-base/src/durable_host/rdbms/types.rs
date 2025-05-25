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

use crate::preview2::golem::rdbms::types::{
    Date, IpAddress, MacAddress, Time, Timestamp, Timestamptz, Uuid,
};
use chrono::{Datelike, Offset, Timelike};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

impl From<uuid::Uuid> for Uuid {
    fn from(value: uuid::Uuid) -> Self {
        let (high_bits, low_bits) = value.as_u64_pair();
        Self {
            high_bits,
            low_bits,
        }
    }
}

impl From<Uuid> for uuid::Uuid {
    fn from(value: Uuid) -> Self {
        Self::from_u64_pair(value.high_bits, value.low_bits)
    }
}

impl From<mac_address::MacAddress> for MacAddress {
    fn from(value: mac_address::MacAddress) -> Self {
        Self {
            octets: value.bytes().into(),
        }
    }
}

impl From<MacAddress> for mac_address::MacAddress {
    fn from(value: MacAddress) -> Self {
        mac_address::MacAddress::new(value.octets.into())
    }
}

impl From<IpAddr> for IpAddress {
    fn from(value: IpAddr) -> Self {
        match value {
            IpAddr::V4(v) => Self::Ipv4(v.octets().into()),
            IpAddr::V6(v) => Self::Ipv6(v.segments().into()),
        }
    }
}

impl From<IpAddress> for IpAddr {
    fn from(value: IpAddress) -> Self {
        match value {
            IpAddress::Ipv4((a, b, c, d)) => {
                let v = Ipv4Addr::new(a, b, c, d);
                IpAddr::V4(v)
            }
            IpAddress::Ipv6((a, b, c, d, e, f, g, h)) => {
                let v = Ipv6Addr::new(a, b, c, d, e, f, g, h);
                IpAddr::V6(v)
            }
        }
    }
}

impl TryFrom<Timestamp> for chrono::DateTime<chrono::Utc> {
    type Error = String;

    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let v: chrono::NaiveDateTime = value.try_into()?;
        Ok(v.and_utc())
    }
}

impl TryFrom<Timestamp> for chrono::NaiveDateTime {
    type Error = String;

    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let date = value.date.try_into()?;
        let time = value.time.try_into()?;
        Ok(chrono::naive::NaiveDateTime::new(date, time))
    }
}

impl From<chrono::DateTime<chrono::Utc>> for Timestamp {
    fn from(v: chrono::DateTime<chrono::Utc>) -> Self {
        v.naive_utc().into()
    }
}

impl From<chrono::NaiveDateTime> for Timestamp {
    fn from(v: chrono::NaiveDateTime) -> Self {
        let date = v.date().into();
        let time = v.time().into();
        Timestamp { date, time }
    }
}

impl From<chrono::NaiveTime> for Time {
    fn from(v: chrono::NaiveTime) -> Self {
        let hour = v.hour() as u8;
        let minute = v.minute() as u8;
        let second = v.second() as u8;
        let nanosecond = v.nanosecond();
        Time {
            hour,
            minute,
            second,
            nanosecond,
        }
    }
}

impl TryFrom<Time> for chrono::NaiveTime {
    type Error = String;

    fn try_from(value: Time) -> Result<Self, Self::Error> {
        let time = chrono::NaiveTime::from_hms_nano_opt(
            value.hour as u32,
            value.minute as u32,
            value.second as u32,
            value.nanosecond,
        )
        .ok_or("Time value is not valid")?;
        Ok(time)
    }
}

impl From<chrono::NaiveDate> for Date {
    fn from(v: chrono::NaiveDate) -> Self {
        let year = v.year();
        let month = v.month() as u8;
        let day = v.day() as u8;
        Date { year, month, day }
    }
}

impl TryFrom<Date> for chrono::NaiveDate {
    type Error = String;

    fn try_from(value: Date) -> Result<Self, Self::Error> {
        let date = chrono::naive::NaiveDate::from_ymd_opt(
            value.year,
            value.month as u32,
            value.day as u32,
        )
        .ok_or("Date value is not valid")?;
        Ok(date)
    }
}

impl TryFrom<Timestamptz> for chrono::DateTime<chrono::Utc> {
    type Error = String;

    fn try_from(value: Timestamptz) -> Result<Self, Self::Error> {
        let datetime: chrono::NaiveDateTime = value.timestamp.try_into()?;
        let offset = chrono::offset::FixedOffset::west_opt(value.offset)
            .ok_or("Offset value is not valid")?;
        let datetime = datetime
            .checked_add_offset(offset)
            .ok_or("Offset value is not valid")?;
        Ok(datetime.and_utc())
    }
}

impl From<chrono::DateTime<chrono::Utc>> for Timestamptz {
    fn from(v: chrono::DateTime<chrono::Utc>) -> Self {
        let timestamp = v.naive_utc().into();
        let offset = v.offset().fix().local_minus_utc();
        Timestamptz { timestamp, offset }
    }
}
