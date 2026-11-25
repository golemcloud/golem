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

use crate::preview2::golem::rdbms::types::{
    Date, IpAddress, MacAddress, Time, Timestamp, Timestamptz, Uuid,
};

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

impl TryFrom<Timestamp> for jiff::Timestamp {
    type Error = String;

    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let v: jiff::civil::DateTime = value.try_into()?;
        v.to_zoned_datetime(jiff::tz::TimeZone::UTC)
            .map(|v| v.timestamp())
            .map_err(|e| e.to_string())
    }
}

impl TryFrom<Timestamp> for jiff::civil::DateTime {
    type Error = String;

    fn try_from(value: Timestamp) -> Result<Self, Self::Error> {
        let date = value.date.try_into()?;
        let time = value.time.try_into()?;
        Ok(jiff::civil::DateTime::from_parts(date, time))
    }
}

impl From<jiff::Timestamp> for Timestamp {
    fn from(v: jiff::Timestamp) -> Self {
        v.to_zoned_datetime(jiff::tz::TimeZone::UTC)
            .datetime()
            .into()
    }
}

impl From<jiff::civil::DateTime> for Timestamp {
    fn from(v: jiff::civil::DateTime) -> Self {
        let date = v.date().into();
        let time = v.time().into();
        Timestamp { date, time }
    }
}

impl From<jiff::civil::Time> for Time {
    fn from(v: jiff::civil::Time) -> Self {
        let hour = v.hour() as u8;
        let minute = v.minute() as u8;
        let second = v.second() as u8;
        let nanosecond = v.subsec_nanosecond() as u32;
        Time {
            hour,
            minute,
            second,
            nanosecond,
        }
    }
}

impl TryFrom<Time> for jiff::civil::Time {
    type Error = String;

    fn try_from(value: Time) -> Result<Self, Self::Error> {
        let time = jiff::civil::Time::new(
            value
                .hour
                .try_into()
                .map_err(|_| "Time value is not valid")?,
            value
                .minute
                .try_into()
                .map_err(|_| "Time value is not valid")?,
            value
                .second
                .try_into()
                .map_err(|_| "Time value is not valid")?,
            value
                .nanosecond
                .try_into()
                .map_err(|_| "Time value is not valid")?,
        )
        .map_err(|e| e.to_string())?;
        Ok(time)
    }
}

impl From<jiff::civil::Date> for Date {
    fn from(v: jiff::civil::Date) -> Self {
        let year = i32::from(v.year());
        let month = v.month() as u8;
        let day = v.day() as u8;
        Date { year, month, day }
    }
}

impl TryFrom<Date> for jiff::civil::Date {
    type Error = String;

    fn try_from(value: Date) -> Result<Self, Self::Error> {
        let date = jiff::civil::Date::new(
            value
                .year
                .try_into()
                .map_err(|_| "Date value is not valid")?,
            value
                .month
                .try_into()
                .map_err(|_| "Date value is not valid")?,
            value
                .day
                .try_into()
                .map_err(|_| "Date value is not valid")?,
        )
        .map_err(|e| e.to_string())?;
        Ok(date)
    }
}

impl TryFrom<Timestamptz> for jiff::Timestamp {
    type Error = String;

    fn try_from(value: Timestamptz) -> Result<Self, Self::Error> {
        let datetime: jiff::civil::DateTime = value.timestamp.try_into()?;
        if !(-86_399..=86_399).contains(&value.offset) {
            return Err("Offset value is not valid".to_string());
        }
        datetime
            .checked_add(jiff::SignedDuration::from_secs(-i64::from(value.offset)))
            .and_then(|v| v.to_zoned_datetime(jiff::tz::TimeZone::UTC))
            .map(|v| v.timestamp())
            .map_err(|e| e.to_string())
    }
}

impl From<jiff::Timestamp> for Timestamptz {
    fn from(v: jiff::Timestamp) -> Self {
        let timestamp = v
            .to_zoned_datetime(jiff::tz::TimeZone::UTC)
            .datetime()
            .into();
        let offset = 0;
        Timestamptz { timestamp, offset }
    }
}
