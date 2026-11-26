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

use sqlx::error::BoxDynError;
use sqlx::postgres::{PgValueFormat, PgValueRef};
use time::{Date, Duration, Month};

/// Rejects binary dates that would overflow SQLx's unchecked epoch arithmetic.
pub fn validate_date(value: PgValueRef<'_>) -> Result<(), BoxDynError> {
    if value.format() == PgValueFormat::Binary {
        let days = i32::from_be_bytes(value.as_bytes()?.try_into()?);
        postgres_epoch()
            .checked_add(Duration::days(i64::from(days)))
            .ok_or("PostgreSQL DATE is outside the supported date range")?;
    }
    Ok(())
}

/// Rejects binary timestamps that would overflow SQLx's unchecked epoch arithmetic.
pub fn validate_timestamp(value: PgValueRef<'_>) -> Result<(), BoxDynError> {
    if value.format() == PgValueFormat::Binary {
        let micros = i64::from_be_bytes(value.as_bytes()?.try_into()?);
        postgres_epoch()
            .midnight()
            .checked_add(Duration::microseconds(micros))
            .ok_or("PostgreSQL timestamp is outside the supported date range")?;
    }
    Ok(())
}

fn postgres_epoch() -> Date {
    Date::from_calendar_date(2000, Month::January, 1).unwrap()
}
