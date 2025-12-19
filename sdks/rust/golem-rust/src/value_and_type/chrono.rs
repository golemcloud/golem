// Copyright 2024-2025 Golem Cloud
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

use super::type_builder::TypeNodeBuilder;
use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_wasm::{NodeBuilder, WitValueExtractor};

impl IntoValue for chrono::DateTime<chrono::Utc> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder.string(&self.to_rfc3339()).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("DateTime".to_string()), Some("chrono".to_string()));
        let builder = builder.field("timestamp");
        builder.string().finish()
    }
}

impl FromValueAndType for chrono::DateTime<chrono::Utc> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing timestamp field".to_string())?;
        value
            .string()
            .ok_or_else(|| "Expected string for DateTime<Utc>".to_string())
            .and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .map_err(|_| "Failed to parse DateTime from RFC3339 string".to_string())
            })
    }
}

impl IntoValue for chrono::NaiveDate {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder
            .string(&self.format("%Y-%m-%d").to_string())
            .finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("NaiveDate".to_string()), Some("chrono".to_string()));
        let builder = builder.field("date");
        builder.string().finish()
    }
}

impl FromValueAndType for chrono::NaiveDate {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing date field".to_string())?;
        value
            .string()
            .ok_or_else(|| "Expected string for NaiveDate".to_string())
            .and_then(|s| {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .map_err(|_| "Failed to parse NaiveDate from YYYY-MM-DD format".to_string())
            })
    }
}

impl IntoValue for chrono::NaiveTime {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder
            .string(&self.format("%H:%M:%S").to_string())
            .finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("NaiveTime".to_string()), Some("chrono".to_string()));
        let builder = builder.field("time");
        builder.string().finish()
    }
}

impl FromValueAndType for chrono::NaiveTime {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing time field".to_string())?;
        value
            .string()
            .ok_or_else(|| "Expected string for NaiveTime".to_string())
            .and_then(|s| {
                chrono::NaiveTime::parse_from_str(s, "%H:%M:%S")
                    .map_err(|_| "Failed to parse NaiveTime from HH:MM:SS format".to_string())
            })
    }
}

impl IntoValue for chrono::NaiveDateTime {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder.string(&self.to_string()).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("NaiveDateTime".to_string()),
            Some("chrono".to_string()),
        );
        let builder = builder.field("datetime");
        builder.string().finish()
    }
}

impl FromValueAndType for chrono::NaiveDateTime {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing datetime field".to_string())?;
        value
            .string()
            .ok_or_else(|| "Expected string for NaiveDateTime".to_string())
            .and_then(|s| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map_err(|_| {
                    "Failed to parse NaiveDateTime from YYYY-MM-DD HH:MM:SS format".to_string()
                })
            })
    }
}

impl IntoValue for chrono::FixedOffset {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder.s32(self.local_minus_utc()).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("FixedOffset".to_string()), Some("chrono".to_string()));
        let builder = builder.field("seconds");
        builder.s32().finish()
    }
}

impl FromValueAndType for chrono::FixedOffset {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing seconds field".to_string())?;
        value
            .s32()
            .ok_or_else(|| "Expected i32 for FixedOffset".to_string())
            .and_then(|seconds| {
                chrono::FixedOffset::east_opt(seconds)
                    .ok_or_else(|| "Invalid FixedOffset seconds value".to_string())
            })
    }
}

impl IntoValue for chrono::Month {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder.u32(self.number_from_month()).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("Month".to_string()), Some("chrono".to_string()));
        let builder = builder.field("number");
        builder.u32().finish()
    }
}

impl FromValueAndType for chrono::Month {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing number field".to_string())?;
        value
            .u32()
            .ok_or_else(|| "Expected u32 for Month".to_string())
            .and_then(|month_num| {
                chrono::Month::try_from(month_num as u8)
                    .map_err(|_| "Invalid month number (must be 1-12)".to_string())
            })
    }
}

impl IntoValue for chrono::Weekday {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let variant_idx = match self {
            chrono::Weekday::Sun => 0,
            chrono::Weekday::Mon => 1,
            chrono::Weekday::Tue => 2,
            chrono::Weekday::Wed => 3,
            chrono::Weekday::Thu => 4,
            chrono::Weekday::Fri => 5,
            chrono::Weekday::Sat => 6,
        };
        builder.variant_unit(variant_idx)
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let mut builder = builder.variant(Some("Weekday".to_string()), Some("chrono".to_string()));
        builder = builder.unit_case("sunday");
        builder = builder.unit_case("monday");
        builder = builder.unit_case("tuesday");
        builder = builder.unit_case("wednesday");
        builder = builder.unit_case("thursday");
        builder = builder.unit_case("friday");
        builder = builder.unit_case("saturday");
        builder.finish()
    }
}

impl FromValueAndType for chrono::Weekday {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let (idx, inner) = extractor
            .variant()
            .ok_or_else(|| "Expected Weekday to be a variant".to_string())?;
        if inner.is_some() {
            return Err("Weekday variants should not have values".to_string());
        }
        match idx {
            0 => Ok(chrono::Weekday::Sun),
            1 => Ok(chrono::Weekday::Mon),
            2 => Ok(chrono::Weekday::Tue),
            3 => Ok(chrono::Weekday::Wed),
            4 => Ok(chrono::Weekday::Thu),
            5 => Ok(chrono::Weekday::Fri),
            6 => Ok(chrono::Weekday::Sat),
            _ => Err(format!("Invalid Weekday variant index: {}", idx)),
        }
    }
}

impl IntoValue for chrono::DateTime<chrono::FixedOffset> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder.string(&self.to_rfc3339()).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("DateTime".to_string()), Some("chrono".to_string()));
        let builder = builder.field("timestamp");
        builder.string().finish()
    }
}

impl FromValueAndType for chrono::DateTime<chrono::FixedOffset> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing timestamp field".to_string())?;
        value
            .string()
            .ok_or_else(|| "Expected string for DateTime<FixedOffset>".to_string())
            .and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s).map_err(|_| {
                    "Failed to parse DateTime<FixedOffset> from RFC3339 string".to_string()
                })
            })
    }
}

impl IntoValue for chrono::DateTime<chrono::Local> {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let builder = builder.record();
        let builder = builder.item();
        builder.string(&self.to_rfc3339()).finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(Some("DateTime".to_string()), Some("chrono".to_string()));
        let builder = builder.field("timestamp");
        builder.string().finish()
    }
}

impl FromValueAndType for chrono::DateTime<chrono::Local> {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let value = extractor
            .field(0usize)
            .ok_or_else(|| "Missing timestamp field".to_string())?;
        value
            .string()
            .ok_or_else(|| "Expected string for DateTime<Local>".to_string())
            .and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .map(|dt| dt.with_timezone(&chrono::Local))
                    .map_err(|_| "Failed to parse DateTime<Local> from RFC3339 string".to_string())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roundtrip_test;
    use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;
    use proptest::prop_assert_eq;
    use proptest::proptest;
    use proptest::strategy::Strategy;
    use test_r::test;

    roundtrip_test!(
        prop_roundtrip_datetime_utc,
        chrono::DateTime<chrono::Utc>,
        (-30610224000i64..=253402300799i64)
            .prop_map(|secs| chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).unwrap())
    );
    roundtrip_test!(
        prop_roundtrip_datetime_fixedoffset,
        chrono::DateTime<chrono::FixedOffset>,
        (-30610224000i64..=253402300799i64).prop_map(|secs| {
            let utc = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).unwrap();
            let fo = chrono::FixedOffset::east_opt(3600).unwrap();
            utc.with_timezone(&fo)
        })
    );
    roundtrip_test!(
        prop_roundtrip_naivedate,
        chrono::NaiveDate,
        (1i32..=365243)
            .prop_map(|days| chrono::NaiveDate::from_num_days_from_ce_opt(days).unwrap())
    );
    roundtrip_test!(
        prop_roundtrip_naivetime,
        chrono::NaiveTime,
        (0u32..86400).prop_map(
            |secs| chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, 0).unwrap()
        )
    );
    roundtrip_test!(
        prop_roundtrip_naivedatetime,
        chrono::NaiveDateTime,
        (1i32..=365243).prop_flat_map(|days| {
            let date = chrono::NaiveDate::from_num_days_from_ce_opt(days).unwrap();
            (0u32..86400).prop_map(move |secs| {
                let time = chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, 0).unwrap();
                chrono::NaiveDateTime::new(date, time)
            })
        })
    );
    roundtrip_test!(
        prop_roundtrip_month,
        chrono::Month,
        (1u8..=12).prop_map(|m| chrono::Month::try_from(m).unwrap())
    );
    #[test]
    fn prop_roundtrip_weekday() {
        proptest!(|(d in 0u32..=6)| {
            let value = match d {
                0 => chrono::Weekday::Sun,
                1 => chrono::Weekday::Mon,
                2 => chrono::Weekday::Tue,
                3 => chrono::Weekday::Wed,
                4 => chrono::Weekday::Thu,
                5 => chrono::Weekday::Fri,
                _ => chrono::Weekday::Sat,
            };
            let typ = chrono::Weekday::get_type();
            let value_and_type = ValueAndType {
                value: value.clone().into_value(),
                typ,
            };
            let recovered = chrono::Weekday::from_value_and_type(value_and_type)
                .expect("roundtrip conversion should succeed");
            prop_assert_eq!(recovered, value);
        });
    }
}
