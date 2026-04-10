// Copyright 2024-2026 Golem Cloud
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

//! Ergonomic wrappers for the `golem:quota/host` WIT interface.
//!
//! # Typical usage
//!
//! ```rust,ignore
//! use golem_rust::quota::{QuotaToken, with_reservation};
//!
//! let token = QuotaToken::new("openai-tokens", 1000);
//! let result = with_reservation(&token, 500, |_reservation| {
//!     // ... call external API ...
//!     let actual_used = 312u64;
//!     (actual_used, my_api_result)
//! });
//! ```

use crate::bindings::golem::api::host::EnvironmentId;
use crate::bindings::golem::quota::types;
use crate::value_and_type::type_builder::TypeNodeBuilder;
use crate::value_and_type::wasi::Datetime;
use crate::value_and_type::{FromValueAndType, IntoValue};
use golem_wasm::{NodeBuilder, WitValueExtractor};
use std::time::Duration;

/// Error returned when a reservation cannot be granted because the resource's
/// enforcement policy is `reject`.
///
/// Contains an optional estimated wait time — only available for rate-limited
/// resources where a future refill is predictable.
#[derive(Debug, Clone, PartialEq)]
pub struct FailedReservation {
    /// How long the caller would likely need to wait for capacity, if known.
    pub estimated_wait: Option<Duration>,
}

impl From<types::FailedReservation> for FailedReservation {
    fn from(raw: types::FailedReservation) -> Self {
        Self {
            estimated_wait: raw.estimated_wait_nanos.map(Duration::from_nanos),
        }
    }
}

impl std::fmt::Display for FailedReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.estimated_wait {
            Some(d) => write!(f, "quota reservation failed (retry after {d:?})"),
            None => write!(f, "quota reservation failed"),
        }
    }
}

impl std::error::Error for FailedReservation {}

/// A short-lived capability representing a pending resource consumption.
///
/// Dropping a `Reservation` without calling [`commit`][Reservation::commit] is
/// equivalent to committing with the full reserved amount
#[must_use]
pub struct Reservation {
    raw: types::Reservation,
}

impl Reservation {
    /// Commit actual usage.
    ///
    /// - If `used` < reserved — unused capacity is returned to the pool.
    /// - If `used` > reserved — the excess is deducted from the token's
    ///   remaining allocation as "debt".
    pub fn commit(self, used: u64) {
        types::Reservation::commit(self.raw, used);
    }
}

/// An unforgeable capability granting the right to consume a named resource.
///
/// Dropping the token releases the underlying lease back to the executor pool.
///
/// # Example
///
/// ```rust,ignore
/// let token = QuotaToken::new("llm-tokens", 1000);
/// match token.reserve(500) {
///     Ok(reservation) => {
///         let used = call_llm_api();
///         reservation.commit(used);
///     }
///     Err(e) => eprintln!("quota unavailable: {e}"),
/// }
/// ```
pub struct QuotaToken {
    raw: types::QuotaToken,
}

impl QuotaToken {
    /// Request a quota capability for the given resource.
    ///
    /// - `resource_name`: the resource name as declared in the manifest.
    /// - `expected_use`: expected units per reservation; used to derive the
    ///   credit rate and max-credit for fair scheduling.
    pub fn new(resource_name: &str, expected_use: u64) -> Self {
        Self {
            raw: types::QuotaToken::new(resource_name, expected_use),
        }
    }

    /// Reserve `amount` units from the local allocation.
    ///
    /// Blocks internally until capacity is available or the resource's
    /// enforcement action fires.  Returns a [`Reservation`] handle that
    /// must be committed (or dropped) to release unused capacity.
    ///
    /// Returns `Err(FailedReservation)` when the enforcement policy is `reject`.
    /// For `throttle` / `terminate` policies the call suspends or terminates
    /// the agent before returning.
    pub fn reserve(&self, amount: u64) -> Result<Reservation, FailedReservation> {
        self.raw
            .reserve(amount)
            .map(|raw| Reservation { raw })
            .map_err(FailedReservation::from)
    }

    /// Split off a child token with `child_expected_use` units.
    ///
    /// - The parent's expected-use is reduced by `child_expected_use`.
    /// - Credits are divided proportionally between parent and child.
    ///
    /// # Panics
    ///
    /// Traps if `child_expected_use` exceeds the parent's current expected-use.
    pub fn split(&mut self, child_expected_use: u64) -> QuotaToken {
        QuotaToken {
            raw: self.raw.split(child_expected_use),
        }
    }

    /// Merge `other` into this token, combining expected-use and credits.
    ///
    /// Both tokens must refer to the same resource (same resource-name and
    /// environment).  `other` is consumed.
    ///
    /// # Panics
    ///
    /// Traps if the tokens refer to different resources.
    pub fn merge(&mut self, other: QuotaToken) {
        self.raw.merge(other.raw);
    }

    fn to_record(&self) -> types::QuotaTokenRecord {
        self.raw.to_record()
    }

    fn from_record(record: &types::QuotaTokenRecord) -> QuotaToken {
        QuotaToken {
            raw: types::QuotaToken::from_record(record),
        }
    }
}

/// Reserve `amount` units, run `f`, then commit the actual usage returned by `f`.
///
/// `f` receives a shared reference to the [`Reservation`] (for inspection) and
/// must return `(used, value)`.
///
/// Returns `Err(FailedReservation)` if the reservation could not be granted.
///
/// # Example
///
/// ```rust,ignore
/// let result = with_reservation(&token, 500, |_res| {
///     let data = call_external_api();
///     (data.tokens_used, data)
/// });
/// ```
pub fn with_reservation<T, F>(token: &QuotaToken, amount: u64, f: F) -> Result<T, FailedReservation>
where
    F: FnOnce(&Reservation) -> (u64, T),
{
    let reservation = token.reserve(amount)?;
    let (used, value) = f(&reservation);
    reservation.commit(used);
    Ok(value)
}

impl IntoValue for QuotaToken {
    fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
        let record = self.to_record();
        let builder = builder.record();
        let builder = record.environment_id.add_to_builder(builder.item());
        let builder = record.resource_name.add_to_builder(builder.item());
        let builder = record.expected_use.add_to_builder(builder.item());
        let builder = record.last_credit.add_to_builder(builder.item());
        let builder = record.last_credit_at.add_to_builder(builder.item());
        builder.finish()
    }

    fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
        let builder = builder.record(
            Some("QuotaTokenRecord".to_string()),
            Some("golem:quota".to_string()),
        );
        let builder = <EnvironmentId>::add_to_type_builder(builder.field("environment-id"));
        let builder = <String>::add_to_type_builder(builder.field("resource-name"));
        let builder = <u64>::add_to_type_builder(builder.field("expected-use"));
        let builder = <i64>::add_to_type_builder(builder.field("last-credit"));
        let builder = <Datetime>::add_to_type_builder(builder.field("last-credit-at"));
        builder.finish()
    }
}

impl FromValueAndType for QuotaToken {
    fn from_extractor<'a, 'b>(
        extractor: &'a impl WitValueExtractor<'a, 'b>,
    ) -> Result<Self, String> {
        let environment_id = <EnvironmentId>::from_extractor(
            &extractor
                .field(0)
                .ok_or_else(|| "Missing environment-id".to_string())?,
        )?;
        let resource_name = <String>::from_extractor(
            &extractor
                .field(1)
                .ok_or_else(|| "Missing resource-name".to_string())?,
        )?;
        let expected_use = <u64>::from_extractor(
            &extractor
                .field(2)
                .ok_or_else(|| "Missing expected-use".to_string())?,
        )?;
        let last_credit = <i64>::from_extractor(
            &extractor
                .field(3)
                .ok_or_else(|| "Missing last-credit".to_string())?,
        )?;
        let last_credit_at = <Datetime>::from_extractor(
            &extractor
                .field(4)
                .ok_or_else(|| "Missing last-credit-at".to_string())?,
        )?;
        let record = types::QuotaTokenRecord {
            environment_id,
            resource_name,
            expected_use,
            last_credit,
            last_credit_at,
        };
        Ok(QuotaToken::from_record(&record))
    }
}
