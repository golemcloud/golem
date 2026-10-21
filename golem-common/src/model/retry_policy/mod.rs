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

mod api;
mod context;
mod continuation;
mod evaluation;
#[cfg(feature = "full")]
mod protobuf;
mod serde;
#[cfg(test)]
mod tests;
mod wit;

use std::collections::BTreeMap;
use std::fmt::Write;
use std::ops::Range;
use std::time::Duration;

use ::serde::{Deserialize, Serialize};
use desert_rust::BinaryCodec;

use crate::SafeDisplay;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
pub struct RetryConfig {
    pub max_attempts: u32,
    #[serde(with = "humantime_serde")]
    pub min_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    pub multiplier: f64,
    pub max_jitter_factor: Option<f64>,
}

impl SafeDisplay for RetryConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();

        let _ = writeln!(&mut result, "max attempts: {}", self.max_attempts);
        let _ = writeln!(&mut result, "min delay: {:?}", self.min_delay);
        let _ = writeln!(&mut result, "max delay: {:?}", self.max_delay);
        let _ = writeln!(&mut result, "multiplier: {}", self.multiplier);
        if let Some(max_jitter_factor) = &self.max_jitter_factor {
            let _ = writeln!(&mut result, "max jitter factor: {max_jitter_factor:?}");
        }

        result
    }
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
#[schema(named = "predicate-value")]
/// A typed value used in retry predicate comparisons.
pub enum PredicateValue {
    /// A UTF-8 string value.
    Text(String),
    /// A 64-bit signed integer value.
    Integer(i64),
    /// A boolean value.
    Boolean(bool),
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
/// A boolean predicate evaluated against [`RetryProperties`] to decide whether a
/// semantic retry policy applies to a given error context.
pub enum Predicate {
    /// True when the property equals the given value.
    PropEq {
        property: String,
        value: PredicateValue,
    },
    /// True when the property does not equal the given value.
    PropNeq {
        property: String,
        value: PredicateValue,
    },
    /// True when the property is strictly greater than the given value.
    PropGt {
        property: String,
        value: PredicateValue,
    },
    /// True when the property is greater than or equal to the given value.
    PropGte {
        property: String,
        value: PredicateValue,
    },
    /// True when the property is strictly less than the given value.
    PropLt {
        property: String,
        value: PredicateValue,
    },
    /// True when the property is less than or equal to the given value.
    PropLte {
        property: String,
        value: PredicateValue,
    },
    /// True when the named property exists in the retry context.
    PropExists(String),
    /// True when the property's value is contained in the given set.
    PropIn {
        property: String,
        values: Vec<PredicateValue>,
    },
    /// True when the property's text representation matches the glob pattern.
    PropMatches { property: String, pattern: String },
    /// True when the property's text representation starts with the given prefix.
    PropStartsWith { property: String, prefix: String },
    /// True when the property's text representation contains the given substring.
    PropContains { property: String, substring: String },
    /// Logical conjunction of two predicates.
    And(Box<Predicate>, Box<Predicate>),
    /// Logical disjunction of two predicates.
    Or(Box<Predicate>, Box<Predicate>),
    /// Logical negation of a predicate.
    Not(Box<Predicate>),
    /// A predicate that always evaluates to true.
    True,
    /// A predicate that always evaluates to false.
    False,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
/// A composable semantic retry policy that determines delay schedules and
/// termination conditions for in-function and background-task retries.
pub enum RetryPolicy {
    /// Retries with a fixed delay between each attempt.
    Periodic(Duration),
    /// Retries with exponentially growing delays: `base_delay * factor^attempt`.
    Exponential { base_delay: Duration, factor: f64 },
    /// Retries following the Fibonacci sequence of delays starting from `first` and `second`.
    Fibonacci { first: Duration, second: Duration },
    /// Retries immediately with zero delay.
    Immediate,
    /// Never retries; gives up on the first failure.
    Never,
    /// Limits the total number of retry attempts, delegating delay calculation to `inner`.
    CountBox {
        max_retries: u32,
        inner: Box<RetryPolicy>,
    },
    /// Limits retries to a wall-clock duration, delegating delay calculation to `inner`.
    TimeBox {
        limit: Duration,
        inner: Box<RetryPolicy>,
    },
    /// Clamps the delay produced by `inner` to the `[min_delay, max_delay]` range.
    Clamp {
        min_delay: Duration,
        max_delay: Duration,
        inner: Box<RetryPolicy>,
    },
    /// Adds a constant delay on top of whatever `inner` produces.
    AddDelay {
        delay: Duration,
        inner: Box<RetryPolicy>,
    },
    /// Adds random jitter (up to `factor` fraction of the base delay) to `inner`'s delay.
    Jitter {
        factor: f64,
        inner: Box<RetryPolicy>,
    },
    /// Applies `inner` only when `predicate` matches the error context; otherwise gives up.
    FilteredOn {
        predicate: Predicate,
        inner: Box<RetryPolicy>,
    },
    /// Runs the first policy until it gives up, then continues with the second.
    AndThen(Box<RetryPolicy>, Box<RetryPolicy>),
    /// Retries as long as *either* sub-policy wants to retry, picking the shorter delay.
    Union(Box<RetryPolicy>, Box<RetryPolicy>),
    /// Retries only while *both* sub-policies want to retry, picking the longer delay.
    Intersect(Box<RetryPolicy>, Box<RetryPolicy>),
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
#[schema(named = "named-retry-policy")]
/// A retry policy paired with a name, priority, and a predicate that determines
/// when it applies. The highest-priority matching policy is selected at retry time.
pub struct NamedRetryPolicy {
    /// Human-readable identifier for this policy.
    pub name: String,
    /// Selection priority — higher values are evaluated first.
    pub priority: u32,
    /// Predicate evaluated against the error context to decide if this policy applies.
    pub predicate: Predicate,
    /// The retry policy to use when the predicate matches.
    pub policy: RetryPolicy,
}

/// Persistent state of a [`RetryPolicy`] across retry attempts.
///
/// Each variant mirrors the structure of the corresponding [`RetryPolicy`] variant
/// and carries the mutable counters / sub-states needed to compute the next delay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
pub enum RetryPolicyState {
    /// Tracks the number of attempts for counter-based policies (e.g. `Periodic`, `Exponential`).
    Counter(u32),
    /// Indicates the policy has permanently given up.
    Terminal,
    /// Wraps the inner state of a decorator policy (e.g. `TimeBox`, `Clamp`, `Jitter`).
    Wrapper(Box<RetryPolicyState>),
    /// Tracks both an attempt count and the inner state for [`RetryPolicy::CountBox`].
    CountBox {
        attempts: u32,
        inner: Box<RetryPolicyState>,
    },
    /// Preserves the elapsed-time origin and monotonic high-water mark for a retry sequence
    /// whose policy contains a [`RetryPolicy::TimeBox`].
    TimeBox {
        started_at_millis: u64,
        elapsed_millis: u64,
        inner: Box<RetryPolicyState>,
    },
    /// Tracks left/right sub-states and whether execution has moved to the right policy
    /// for [`RetryPolicy::AndThen`].
    AndThen {
        left: Box<RetryPolicyState>,
        right: Box<RetryPolicyState>,
        on_right: bool,
    },
    /// Tracks two independent sub-states for [`RetryPolicy::Union`] and [`RetryPolicy::Intersect`].
    Pair(Box<RetryPolicyState>, Box<RetryPolicyState>),
}

/// A bag of key-value properties describing the error context (HTTP status, verb, URI, etc.)
/// that retry predicates are evaluated against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetryProperties {
    entries: BTreeMap<String, PredicateValue>,
}

/// The outcome of a single [`RetryPolicy::step`] evaluation.
#[derive(Clone, Debug, PartialEq)]
pub enum RetryVerdict {
    /// The policy decided to retry after the given delay.
    Retry(Duration),
    /// The policy decided to give up (retries exhausted or not applicable).
    GiveUp,
    /// An error occurred while evaluating the policy (e.g. missing property).
    Error(RetryEvaluationError),
}

/// Discriminant for [`PredicateValue`] types, used in coercion error messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub enum PredicateValueType {
    /// Corresponds to [`PredicateValue::Text`].
    Text,
    /// Corresponds to [`PredicateValue::Integer`].
    Integer,
    /// Corresponds to [`PredicateValue::Boolean`].
    Boolean,
}

/// Errors that can occur while evaluating a [`Predicate`] or stepping a [`RetryPolicy`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec, thiserror::Error)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub enum RetryEvaluationError {
    #[error("Property not found: {property}")]
    PropertyNotFound { property: String },
    #[error("Cannot coerce property '{property}' value {value:?} to target type {target_type:?}")]
    CoercionFailed {
        property: String,
        value: PredicateValue,
        target_type: PredicateValueType,
    },
    #[error("Invalid retry policy state: {details}")]
    InvalidState { details: String },
}

/// Source of randomness used by [`RetryPolicy::Jitter`] to add random noise to delays.
pub trait RngSource: Send {
    /// Returns a random `f64` within the given range.
    fn random_f64(&mut self, range: Range<f64>) -> f64;
}

/// [`RngSource`] backed by the thread-local random number generator.
pub struct ThreadRng;

/// A deterministic [`RngSource`] that always returns a fixed value. Useful for testing.
pub struct FixedRng(pub f64);

/// Factory for building [`RetryProperties`] for various operation types (HTTP, RPC, KV, etc.).
pub struct RetryContext;

pub fn duration_to_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

pub use crate::base_model::retry_policy::{
    RetryPolicyCreation, RetryPolicyDto, RetryPolicyId, RetryPolicyRevision, RetryPolicyUpdate,
};
