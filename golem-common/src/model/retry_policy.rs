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

use crate::model::agent::{parse_agent_id_parts, DataValue, ElementValue, NamedElementValue};
use crate::model::{OwnedAgentId, RdbmsPoolKey, RetryConfig};
use desert_rust::BinaryCodec;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{FromValue, IntoValue, Value};
use golem_wasm_derive::{FromValue, IntoValue};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::time::Duration;

const RETRY_WIT_OWNER: &str = "golem:api@1.5.0/retry";

#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BinaryCodec, IntoValue, FromValue,
)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
#[wit(name = "predicate-value", owner = "golem:api@1.5.0/retry")]
/// A typed value used in retry predicate comparisons.
pub enum PredicateValue {
    /// A UTF-8 string value.
    Text(String),
    /// A 64-bit signed integer value.
    Integer(i64),
    /// A boolean value.
    Boolean(bool),
}

impl PredicateValue {
    fn value_type(&self) -> PredicateValueType {
        match self {
            PredicateValue::Text(_) => PredicateValueType::Text,
            PredicateValue::Integer(_) => PredicateValueType::Integer,
            PredicateValue::Boolean(_) => PredicateValueType::Boolean,
        }
    }

    fn as_text(&self, property: &str) -> Result<String, RetryEvaluationError> {
        match self {
            PredicateValue::Text(value) => Ok(value.clone()),
            PredicateValue::Integer(value) => Ok(value.to_string()),
            PredicateValue::Boolean(_) => Err(RetryEvaluationError::CoercionFailed {
                property: property.to_string(),
                value: self.clone(),
                target_type: PredicateValueType::Text,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
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
    PropMatches {
        property: String,
        pattern: String,
    },
    /// True when the property's text representation starts with the given prefix.
    PropStartsWith {
        property: String,
        prefix: String,
    },
    /// True when the property's text representation contains the given substring.
    PropContains {
        property: String,
        substring: String,
    },
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
/// A composable semantic retry policy that determines delay schedules and
/// termination conditions for in-function and background-task retries.
pub enum RetryPolicy {
    /// Retries with a fixed delay between each attempt.
    Periodic(Duration),
    /// Retries with exponentially growing delays: `base_delay * factor^attempt`.
    Exponential {
        base_delay: Duration,
        factor: f64,
    },
    /// Retries following the Fibonacci sequence of delays starting from `first` and `second`.
    Fibonacci {
        first: Duration,
        second: Duration,
    },
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
#[wit(name = "named-retry-policy", owner = "golem:api@1.5.0/retry")]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec, IntoValue, FromValue)]
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

impl RetryPolicyState {
    /// Returns the total number of retry attempts tracked by this state.
    pub fn retry_count(&self) -> u32 {
        match self {
            RetryPolicyState::Counter(n) => *n,
            RetryPolicyState::Terminal => 0,
            RetryPolicyState::Wrapper(inner) => inner.retry_count(),
            RetryPolicyState::CountBox { attempts, .. } => *attempts,
            RetryPolicyState::AndThen {
                left,
                right,
                on_right,
            } => {
                if *on_right {
                    left.retry_count() + right.retry_count()
                } else {
                    left.retry_count()
                }
            }
            RetryPolicyState::Pair(left, right) => left.retry_count().max(right.retry_count()),
        }
    }
}

/// A bag of key-value properties describing the error context (HTTP status, verb, URI, etc.)
/// that retry predicates are evaluated against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetryProperties {
    entries: BTreeMap<String, PredicateValue>,
}

impl RetryProperties {
    /// Creates an empty property bag.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Inserts or updates a property. Returns `&mut Self` for chaining.
    pub fn set(&mut self, key: impl Into<String>, value: PredicateValue) -> &mut Self {
        self.entries.insert(key.into(), value);
        self
    }

    /// Returns the value associated with the given key, if present.
    pub fn get(&self, key: &str) -> Option<&PredicateValue> {
        self.entries.get(key)
    }

    /// Returns `true` if the property bag contains the given key.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Iterates over all key-value pairs in the property bag.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PredicateValue)> {
        self.entries.iter()
    }
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

impl RngSource for ThreadRng {
    fn random_f64(&mut self, range: Range<f64>) -> f64 {
        rand::rng().random_range(range)
    }
}

/// A deterministic [`RngSource`] that always returns a fixed value. Useful for testing.
pub struct FixedRng(pub f64);

impl RngSource for FixedRng {
    fn random_f64(&mut self, _range: Range<f64>) -> f64 {
        self.0
    }
}

/// Factory for building [`RetryProperties`] for various operation types (HTTP, RPC, KV, etc.).
pub struct RetryContext;

impl RetryContext {
    /// Builds retry properties for an outgoing HTTP request.
    pub fn http(method: &str, uri: &str) -> RetryProperties {
        let mut props = Self::base(method, uri);
        decompose_uri(&mut props, uri);
        props
    }

    /// Builds retry properties for an HTTP request that received a response or error.
    pub fn http_with_response(
        method: &str,
        uri: &str,
        status_code: Option<u16>,
        error_type: &str,
    ) -> RetryProperties {
        let mut props = Self::http(method, uri);
        if let Some(status_code) = status_code {
            props.set("status-code", PredicateValue::Integer(status_code as i64));
        }
        props.set("error-type", PredicateValue::Text(error_type.to_string()));
        props
    }

    /// Builds retry properties for a worker-to-worker RPC call.
    pub fn rpc(verb: &str, target: &OwnedAgentId, function: &str) -> RetryProperties {
        let noun_uri = format!("worker://{}/{}", target.component_id(), target.agent_name());
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
            .set("function", PredicateValue::Text(function.to_string()))
            .set(
                "target-component-id",
                PredicateValue::Text(target.component_id().to_string()),
            );

        if let Ok((agent_type_name, _, _)) = parse_agent_id_parts(&target.agent_name()) {
            props.set(
                "target-agent-type",
                PredicateValue::Text(agent_type_name.to_string()),
            );
        }

        props
    }

    /// Builds retry properties for a key-value store operation.
    pub fn kv(verb: &str, bucket: &str) -> RetryProperties {
        let noun_uri = format!("kv://{bucket}");
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
    }

    /// Builds retry properties for a blob storage operation.
    pub fn blobstore(verb: &str, container: &str) -> RetryProperties {
        let noun_uri = format!("blobstore://{container}");
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
    }

    /// Builds retry properties for an RDBMS operation.
    pub fn rdbms(verb: &str, pool_key: &RdbmsPoolKey) -> RetryProperties {
        let noun_uri = pool_key.masked_address();
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);

        if let Ok(url) = url::Url::parse(&noun_uri) {
            props.set(
                "db-type",
                PredicateValue::Text(url.scheme().to_ascii_lowercase()),
            );
        }

        props
    }

    /// Builds retry properties for a DNS resolution attempt.
    pub fn dns(hostname: &str) -> RetryProperties {
        let noun_uri = format!("dns://{hostname}");
        let mut props = Self::base("resolve", &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
    }

    /// Builds retry properties for a Golem platform API call.
    pub fn golem_api(verb: &str) -> RetryProperties {
        let noun_uri = "golem://api";
        let mut props = Self::base(verb, noun_uri);
        decompose_uri(&mut props, noun_uri);
        props
    }

    /// Builds retry properties for a WASM trap (deterministic or transient).
    pub fn trap(trap_type: &str, function: Option<&str>) -> RetryProperties {
        let function = function.unwrap_or("unknown");
        let noun_uri = format!("wasm://{function}");
        let mut props = Self::base("trap", &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props.set("trap-type", PredicateValue::Text(trap_type.to_string()));
        props
    }

    /// Builds retry properties for a custom operation with a verb and noun URI.
    pub fn custom(verb: &str, noun_uri: &str) -> RetryProperties {
        let mut props = Self::base(verb, noun_uri);
        decompose_uri(&mut props, noun_uri);
        props
    }

    fn base(verb: &str, noun_uri: &str) -> RetryProperties {
        let mut props = RetryProperties::new();
        props
            .set("verb", PredicateValue::Text(verb.to_string()))
            .set("noun-uri", PredicateValue::Text(noun_uri.to_string()));
        props
    }
}

/// Flattens an agent's invocation parameters into retry properties using dot-separated keys.
fn flatten_agent_params(props: &mut RetryProperties, prefix: &str, value: &DataValue) {
    match value {
        DataValue::Tuple(elements) => {
            for (index, element) in elements.elements.iter().enumerate() {
                flatten_element_value(props, &format!("{prefix}.{index}"), element);
            }
        }
        DataValue::Multimodal(elements) => {
            for NamedElementValue {
                name,
                value,
                schema_index: _,
            } in &elements.elements
            {
                flatten_element_value(props, &format!("{prefix}.{name}"), value);
            }
        }
    }
}

/// Parses a URI and adds its scheme, host, port, and path as individual retry properties.
fn decompose_uri(props: &mut RetryProperties, uri: &str) {
    if let Ok(parsed) = url::Url::parse(uri) {
        props.set(
            "uri-scheme",
            PredicateValue::Text(parsed.scheme().to_string()),
        );
        if let Some(host) = parsed.host_str() {
            props.set("uri-host", PredicateValue::Text(host.to_string()));
        }
        if let Some(port) = parsed.port() {
            props.set("uri-port", PredicateValue::Integer(port as i64));
        }
        props.set("uri-path", PredicateValue::Text(parsed.path().to_string()));
    }
}

impl Predicate {
    /// Evaluates this predicate against the given retry properties, returning `true` if matched.
    pub fn matches(&self, props: &RetryProperties) -> Result<bool, RetryEvaluationError> {
        match self {
            Predicate::PropEq { property, value } => {
                let actual = get_required_property(props, property)?;
                Ok(coerce_eq(actual, value, property)?)
            }
            Predicate::PropNeq { property, value } => {
                let actual = get_required_property(props, property)?;
                Ok(!coerce_eq(actual, value, property)?)
            }
            Predicate::PropGt { property, value } => {
                let actual = get_required_property(props, property)?;
                Ok(coerce_cmp(actual, value, property)? == Ordering::Greater)
            }
            Predicate::PropGte { property, value } => {
                let actual = get_required_property(props, property)?;
                Ok(matches!(
                    coerce_cmp(actual, value, property)?,
                    Ordering::Greater | Ordering::Equal
                ))
            }
            Predicate::PropLt { property, value } => {
                let actual = get_required_property(props, property)?;
                Ok(coerce_cmp(actual, value, property)? == Ordering::Less)
            }
            Predicate::PropLte { property, value } => {
                let actual = get_required_property(props, property)?;
                Ok(matches!(
                    coerce_cmp(actual, value, property)?,
                    Ordering::Less | Ordering::Equal
                ))
            }
            Predicate::PropExists(property) => Ok(props.contains(property)),
            Predicate::PropIn { property, values } => {
                let actual = get_required_property(props, property)?;
                let mut first_error = None;

                for candidate in values {
                    match coerce_eq(actual, candidate, property) {
                        Ok(true) => return Ok(true),
                        Ok(false) => {}
                        Err(error) if first_error.is_none() => first_error = Some(error),
                        Err(_) => {}
                    }
                }

                if let Some(error) = first_error {
                    Err(error)
                } else {
                    Ok(false)
                }
            }
            Predicate::PropMatches { property, pattern } => {
                let actual = get_required_property(props, property)?.as_text(property)?;
                Ok(glob_match::glob_match(pattern, &actual))
            }
            Predicate::PropStartsWith { property, prefix } => {
                let actual = get_required_property(props, property)?.as_text(property)?;
                Ok(actual.starts_with(prefix))
            }
            Predicate::PropContains {
                property,
                substring,
            } => {
                let actual = get_required_property(props, property)?.as_text(property)?;
                Ok(actual.contains(substring))
            }
            Predicate::And(left, right) => Ok(left.matches(props)? && right.matches(props)?),
            Predicate::Or(left, right) => Ok(left.matches(props)? || right.matches(props)?),
            Predicate::Not(inner) => Ok(!inner.matches(props)?),
            Predicate::True => Ok(true),
            Predicate::False => Ok(false),
        }
    }
}

impl RetryPolicy {
    /// Returns the initial [`RetryPolicyState`] for this policy (zero attempts, not yet started).
    pub fn initial_state(&self) -> RetryPolicyState {
        match self {
            RetryPolicy::Periodic(_)
            | RetryPolicy::Exponential { .. }
            | RetryPolicy::Fibonacci { .. }
            | RetryPolicy::Immediate => RetryPolicyState::Counter(0),
            RetryPolicy::Never => RetryPolicyState::Terminal,
            RetryPolicy::CountBox { inner, .. } => RetryPolicyState::CountBox {
                attempts: 0,
                inner: Box::new(inner.initial_state()),
            },
            RetryPolicy::TimeBox { inner, .. }
            | RetryPolicy::Clamp { inner, .. }
            | RetryPolicy::AddDelay { inner, .. }
            | RetryPolicy::Jitter { inner, .. }
            | RetryPolicy::FilteredOn { inner, .. } => {
                RetryPolicyState::Wrapper(Box::new(inner.initial_state()))
            }
            RetryPolicy::AndThen(left, right) => RetryPolicyState::AndThen {
                left: Box::new(left.initial_state()),
                right: Box::new(right.initial_state()),
                on_right: false,
            },
            RetryPolicy::Union(left, right) | RetryPolicy::Intersect(left, right) => {
                RetryPolicyState::Pair(
                    Box::new(left.initial_state()),
                    Box::new(right.initial_state()),
                )
            }
        }
    }

    /// Advances the policy by one retry attempt, returning the updated state and a verdict
    /// indicating whether to retry (and after what delay) or give up.
    pub fn step(
        &self,
        state: &RetryPolicyState,
        elapsed: Duration,
        properties: &RetryProperties,
        rng: &mut dyn RngSource,
    ) -> (RetryPolicyState, RetryVerdict) {
        match (self, state) {
            (RetryPolicy::Periodic(delay), RetryPolicyState::Counter(counter)) => (
                RetryPolicyState::Counter(counter.saturating_add(1)),
                RetryVerdict::Retry(*delay),
            ),
            (
                RetryPolicy::Exponential { base_delay, factor },
                RetryPolicyState::Counter(counter),
            ) => {
                let scaled = scale_duration(*base_delay, factor.powf(*counter as f64));
                (
                    RetryPolicyState::Counter(counter.saturating_add(1)),
                    RetryVerdict::Retry(scaled),
                )
            }
            (RetryPolicy::Fibonacci { first, second }, RetryPolicyState::Counter(counter)) => {
                let nth = counter.saturating_add(1);
                let delay = fibonacci_delay(*first, *second, nth);
                (
                    RetryPolicyState::Counter(counter.saturating_add(1)),
                    RetryVerdict::Retry(delay),
                )
            }
            (RetryPolicy::Immediate, RetryPolicyState::Counter(counter)) => (
                RetryPolicyState::Counter(counter.saturating_add(1)),
                RetryVerdict::Retry(Duration::ZERO),
            ),
            (RetryPolicy::Never, RetryPolicyState::Terminal) => {
                (RetryPolicyState::Terminal, RetryVerdict::GiveUp)
            }
            (
                RetryPolicy::CountBox {
                    max_retries,
                    inner,
                },
                RetryPolicyState::CountBox { attempts, inner: s },
            ) => {
                if attempts >= max_retries {
                    (
                        RetryPolicyState::CountBox {
                            attempts: *attempts,
                            inner: s.clone(),
                        },
                        RetryVerdict::GiveUp,
                    )
                } else {
                    let (new_inner, verdict) = inner.step(s, elapsed, properties, rng);
                    (
                        RetryPolicyState::CountBox {
                            attempts: attempts.saturating_add(1),
                            inner: Box::new(new_inner),
                        },
                        verdict,
                    )
                }
            }
            (
                RetryPolicy::TimeBox { limit, inner },
                RetryPolicyState::Wrapper(inner_state),
            ) => {
                if elapsed >= *limit {
                    (
                        RetryPolicyState::Wrapper(inner_state.clone()),
                        RetryVerdict::GiveUp,
                    )
                } else {
                    let (new_inner, verdict) = inner.step(inner_state, elapsed, properties, rng);
                    (RetryPolicyState::Wrapper(Box::new(new_inner)), verdict)
                }
            }
            (
                RetryPolicy::Clamp {
                    min_delay,
                    max_delay,
                    inner,
                },
                RetryPolicyState::Wrapper(inner_state),
            ) => {
                let (new_inner, verdict) = inner.step(inner_state, elapsed, properties, rng);
                let verdict = match verdict {
                    RetryVerdict::Retry(delay) => {
                        RetryVerdict::Retry(delay.clamp(*min_delay, *max_delay))
                    }
                    other => other,
                };
                (RetryPolicyState::Wrapper(Box::new(new_inner)), verdict)
            }
            (
                RetryPolicy::AddDelay { delay, inner },
                RetryPolicyState::Wrapper(inner_state),
            ) => {
                let (new_inner, verdict) = inner.step(inner_state, elapsed, properties, rng);
                let verdict = match verdict {
                    RetryVerdict::Retry(current) => {
                        RetryVerdict::Retry(saturating_add_duration(current, *delay))
                    }
                    other => other,
                };
                (RetryPolicyState::Wrapper(Box::new(new_inner)), verdict)
            }
            (
                RetryPolicy::Jitter { factor, inner },
                RetryPolicyState::Wrapper(inner_state),
            ) => {
                let (new_inner, verdict) = inner.step(inner_state, elapsed, properties, rng);
                let verdict = match verdict {
                    RetryVerdict::Retry(current) if *factor > 0.0 => {
                        let random = rng.random_f64(0.0..*factor);
                        let jitter = scale_duration(current, random);
                        RetryVerdict::Retry(saturating_add_duration(current, jitter))
                    }
                    other => other,
                };
                (RetryPolicyState::Wrapper(Box::new(new_inner)), verdict)
            }
            (
                RetryPolicy::FilteredOn { predicate, inner },
                RetryPolicyState::Wrapper(inner_state),
            ) => match predicate.matches(properties) {
                Ok(true) => {
                    let (new_inner, verdict) = inner.step(inner_state, elapsed, properties, rng);
                    (RetryPolicyState::Wrapper(Box::new(new_inner)), verdict)
                }
                Ok(false) => (
                    RetryPolicyState::Wrapper(inner_state.clone()),
                    RetryVerdict::GiveUp,
                ),
                Err(error) => (
                    RetryPolicyState::Wrapper(inner_state.clone()),
                    RetryVerdict::Error(error),
                ),
            },
            (
                RetryPolicy::AndThen(left_policy, right_policy),
                RetryPolicyState::AndThen {
                    left,
                    right,
                    on_right,
                },
            ) => {
                if !on_right {
                    let (new_left, left_verdict) =
                        left_policy.step(left, elapsed, properties, rng);
                    match left_verdict {
                        RetryVerdict::Retry(delay) => (
                            RetryPolicyState::AndThen {
                                left: Box::new(new_left),
                                right: right.clone(),
                                on_right: false,
                            },
                            RetryVerdict::Retry(delay),
                        ),
                        RetryVerdict::GiveUp => {
                            let (new_right, right_verdict) =
                                right_policy.step(right, elapsed, properties, rng);
                            (
                                RetryPolicyState::AndThen {
                                    left: Box::new(new_left),
                                    right: Box::new(new_right),
                                    on_right: true,
                                },
                                right_verdict,
                            )
                        }
                        RetryVerdict::Error(error) => (
                            RetryPolicyState::AndThen {
                                left: Box::new(new_left),
                                right: right.clone(),
                                on_right: false,
                            },
                            RetryVerdict::Error(error),
                        ),
                    }
                } else {
                    let (new_right, verdict) = right_policy.step(right, elapsed, properties, rng);
                    (
                        RetryPolicyState::AndThen {
                            left: left.clone(),
                            right: Box::new(new_right),
                            on_right: true,
                        },
                        verdict,
                    )
                }
            }
            (
                RetryPolicy::Union(left_policy, right_policy),
                RetryPolicyState::Pair(left_state, right_state),
            ) => {
                let (new_left, left_verdict) =
                    left_policy.step(left_state, elapsed, properties, rng);
                let (new_right, right_verdict) =
                    right_policy.step(right_state, elapsed, properties, rng);

                let verdict = match (left_verdict, right_verdict) {
                    (RetryVerdict::Error(error), _) | (_, RetryVerdict::Error(error)) => {
                        RetryVerdict::Error(error)
                    }
                    (RetryVerdict::Retry(left_delay), RetryVerdict::Retry(right_delay)) => {
                        RetryVerdict::Retry(left_delay.min(right_delay))
                    }
                    (RetryVerdict::Retry(delay), RetryVerdict::GiveUp)
                    | (RetryVerdict::GiveUp, RetryVerdict::Retry(delay)) => {
                        RetryVerdict::Retry(delay)
                    }
                    (RetryVerdict::GiveUp, RetryVerdict::GiveUp) => RetryVerdict::GiveUp,
                };

                (
                    RetryPolicyState::Pair(Box::new(new_left), Box::new(new_right)),
                    verdict,
                )
            }
            (
                RetryPolicy::Intersect(left_policy, right_policy),
                RetryPolicyState::Pair(left_state, right_state),
            ) => {
                let (new_left, left_verdict) =
                    left_policy.step(left_state, elapsed, properties, rng);
                let (new_right, right_verdict) =
                    right_policy.step(right_state, elapsed, properties, rng);

                let verdict = match (left_verdict, right_verdict) {
                    (RetryVerdict::Error(error), _) | (_, RetryVerdict::Error(error)) => {
                        RetryVerdict::Error(error)
                    }
                    (RetryVerdict::Retry(left_delay), RetryVerdict::Retry(right_delay)) => {
                        RetryVerdict::Retry(left_delay.max(right_delay))
                    }
                    (RetryVerdict::GiveUp, _) | (_, RetryVerdict::GiveUp) => RetryVerdict::GiveUp,
                };

                (
                    RetryPolicyState::Pair(Box::new(new_left), Box::new(new_right)),
                    verdict,
                )
            }
            _ => (
                self.initial_state(),
                RetryVerdict::Error(RetryEvaluationError::InvalidState {
                    details: format!(
                        "State shape does not match policy variant: policy={self:?}, state={state:?}"
                    ),
                }),
            ),
        }
    }

    /// Rebuilds a [`RetryPolicyState`] by replaying `count` steps from the initial state.
    ///
    /// Used when migrating from a legacy retry counter to the semantic retry model.
    pub fn reconstruct_state_from_count(
        &self,
        count: u32,
        elapsed: Duration,
        properties: &RetryProperties,
        rng: &mut dyn RngSource,
    ) -> Result<RetryPolicyState, RetryEvaluationError> {
        let mut state = self.initial_state();
        for _ in 0..count {
            let (next_state, verdict) = self.step(&state, elapsed, properties, rng);
            state = next_state;
            match verdict {
                RetryVerdict::Retry(_) => {}
                RetryVerdict::GiveUp => break,
                RetryVerdict::Error(error) => return Err(error),
            }
        }
        Ok(state)
    }
}

impl NamedRetryPolicy {
    /// Selects the highest-priority named policy whose predicate matches the given properties.
    ///
    /// Returns `None` if no policy matches.
    pub fn resolve<'a>(
        policies: &'a [NamedRetryPolicy],
        properties: &RetryProperties,
    ) -> Result<Option<&'a NamedRetryPolicy>, RetryEvaluationError> {
        let mut ordered = policies.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| right.priority.cmp(&left.priority));

        for policy in ordered {
            if policy.predicate.matches(properties)? {
                return Ok(Some(policy));
            }
        }

        Ok(None)
    }
}

impl From<RetryConfig> for RetryPolicy {
    fn from(config: RetryConfig) -> Self {
        let inner = RetryPolicy::Exponential {
            base_delay: config.min_delay,
            factor: config.multiplier,
        };

        let inner = RetryPolicy::Clamp {
            min_delay: config.min_delay,
            max_delay: config.max_delay,
            inner: Box::new(inner),
        };

        let inner = match config.max_jitter_factor {
            Some(factor) => RetryPolicy::Jitter {
                factor,
                inner: Box::new(inner),
            },
            None => inner,
        };

        RetryPolicy::CountBox {
            max_retries: config.max_attempts,
            inner: Box::new(inner),
        }
    }
}

impl IntoValue for Predicate {
    fn into_value(self) -> Value {
        FlattenedRetryPredicate::from(self).into_value()
    }

    fn get_type() -> AnalysedType {
        FlattenedRetryPredicate::get_type()
            .named("retry-predicate")
            .owned(RETRY_WIT_OWNER)
    }
}

impl FromValue for Predicate {
    fn from_value(value: Value) -> Result<Self, String> {
        let flattened = FlattenedRetryPredicate::from_value(value)?;
        flattened.try_into()
    }
}

impl IntoValue for RetryPolicy {
    fn into_value(self) -> Value {
        FlattenedRetryPolicy::from(self).into_value()
    }

    fn get_type() -> AnalysedType {
        FlattenedRetryPolicy::get_type()
            .named("retry-policy")
            .owned(RETRY_WIT_OWNER)
    }
}

impl FromValue for RetryPolicy {
    fn from_value(value: Value) -> Result<Self, String> {
        let flattened = FlattenedRetryPolicy::from_value(value)?;
        flattened.try_into()
    }
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "retry-predicate", owner = "golem:api@1.5.0/retry")]
struct FlattenedRetryPredicate {
    nodes: Vec<FlattenedPredicateNode>,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "predicate-node", owner = "golem:api@1.5.0/retry")]
enum FlattenedPredicateNode {
    PropEq(FlattenedPropertyComparison),
    PropNeq(FlattenedPropertyComparison),
    PropGt(FlattenedPropertyComparison),
    PropGte(FlattenedPropertyComparison),
    PropLt(FlattenedPropertyComparison),
    PropLte(FlattenedPropertyComparison),
    PropExists(String),
    PropIn(FlattenedPropertySetCheck),
    PropMatches(FlattenedPropertyPattern),
    PropStartsWith(FlattenedPropertyPattern),
    PropContains(FlattenedPropertyPattern),
    PredAnd((i32, i32)),
    PredOr((i32, i32)),
    PredNot(i32),
    PredTrue,
    PredFalse,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "property-comparison", owner = "golem:api@1.5.0/retry")]
struct FlattenedPropertyComparison {
    property_name: String,
    value: PredicateValue,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "property-set-check", owner = "golem:api@1.5.0/retry")]
struct FlattenedPropertySetCheck {
    property_name: String,
    values: Vec<PredicateValue>,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "property-pattern", owner = "golem:api@1.5.0/retry")]
struct FlattenedPropertyPattern {
    property_name: String,
    pattern: String,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "retry-policy", owner = "golem:api@1.5.0/retry")]
struct FlattenedRetryPolicy {
    nodes: Vec<FlattenedPolicyNode>,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "policy-node", owner = "golem:api@1.5.0/retry")]
enum FlattenedPolicyNode {
    Periodic(u64),
    Exponential(FlattenedExponentialConfig),
    Fibonacci(FlattenedFibonacciConfig),
    Immediate,
    Never,
    CountBox(FlattenedCountBoxConfig),
    TimeBox(FlattenedTimeBoxConfig),
    ClampDelay(FlattenedClampConfig),
    AddDelay(FlattenedAddDelayConfig),
    Jitter(FlattenedJitterConfig),
    FilteredOn(FlattenedFilteredConfig),
    AndThen((i32, i32)),
    PolicyUnion((i32, i32)),
    PolicyIntersect((i32, i32)),
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "exponential-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedExponentialConfig {
    base_delay: u64,
    factor: f64,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "fibonacci-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedFibonacciConfig {
    first: u64,
    second: u64,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "count-box-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedCountBoxConfig {
    max_retries: u32,
    inner: i32,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "time-box-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedTimeBoxConfig {
    limit: u64,
    inner: i32,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "clamp-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedClampConfig {
    min_delay: u64,
    max_delay: u64,
    inner: i32,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "add-delay-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedAddDelayConfig {
    delay: u64,
    inner: i32,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "jitter-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedJitterConfig {
    factor: f64,
    inner: i32,
}

#[derive(Clone, Debug, PartialEq, IntoValue, FromValue)]
#[wit(name = "filtered-config", owner = "golem:api@1.5.0/retry")]
struct FlattenedFilteredConfig {
    predicate: FlattenedRetryPredicate,
    inner: i32,
}

impl From<Predicate> for FlattenedRetryPredicate {
    fn from(predicate: Predicate) -> Self {
        let mut nodes = Vec::new();
        push_predicate_node(predicate, &mut nodes);
        Self { nodes }
    }
}

impl TryFrom<FlattenedRetryPredicate> for Predicate {
    type Error = String;

    fn try_from(value: FlattenedRetryPredicate) -> Result<Self, Self::Error> {
        if value.nodes.is_empty() {
            return Err("retry-predicate requires at least one node".to_string());
        }

        build_predicate_from_index(&value.nodes, 0, &mut HashSet::new())
    }
}

impl From<RetryPolicy> for FlattenedRetryPolicy {
    fn from(policy: RetryPolicy) -> Self {
        let mut nodes = Vec::new();
        push_policy_node(policy, &mut nodes);
        Self { nodes }
    }
}

impl TryFrom<FlattenedRetryPolicy> for RetryPolicy {
    type Error = String;

    fn try_from(value: FlattenedRetryPolicy) -> Result<Self, Self::Error> {
        if value.nodes.is_empty() {
            return Err("retry-policy requires at least one node".to_string());
        }

        build_policy_from_index(&value.nodes, 0, &mut HashSet::new())
    }
}

fn push_predicate_node(predicate: Predicate, nodes: &mut Vec<FlattenedPredicateNode>) -> i32 {
    let index = nodes.len() as i32;
    nodes.push(FlattenedPredicateNode::PredFalse);

    let node = match predicate {
        Predicate::PropEq { property, value } => {
            FlattenedPredicateNode::PropEq(FlattenedPropertyComparison {
                property_name: property,
                value,
            })
        }
        Predicate::PropNeq { property, value } => {
            FlattenedPredicateNode::PropNeq(FlattenedPropertyComparison {
                property_name: property,
                value,
            })
        }
        Predicate::PropGt { property, value } => {
            FlattenedPredicateNode::PropGt(FlattenedPropertyComparison {
                property_name: property,
                value,
            })
        }
        Predicate::PropGte { property, value } => {
            FlattenedPredicateNode::PropGte(FlattenedPropertyComparison {
                property_name: property,
                value,
            })
        }
        Predicate::PropLt { property, value } => {
            FlattenedPredicateNode::PropLt(FlattenedPropertyComparison {
                property_name: property,
                value,
            })
        }
        Predicate::PropLte { property, value } => {
            FlattenedPredicateNode::PropLte(FlattenedPropertyComparison {
                property_name: property,
                value,
            })
        }
        Predicate::PropExists(property) => FlattenedPredicateNode::PropExists(property),
        Predicate::PropIn { property, values } => {
            FlattenedPredicateNode::PropIn(FlattenedPropertySetCheck {
                property_name: property,
                values,
            })
        }
        Predicate::PropMatches { property, pattern } => {
            FlattenedPredicateNode::PropMatches(FlattenedPropertyPattern {
                property_name: property,
                pattern,
            })
        }
        Predicate::PropStartsWith { property, prefix } => {
            FlattenedPredicateNode::PropStartsWith(FlattenedPropertyPattern {
                property_name: property,
                pattern: prefix,
            })
        }
        Predicate::PropContains {
            property,
            substring,
        } => FlattenedPredicateNode::PropContains(FlattenedPropertyPattern {
            property_name: property,
            pattern: substring,
        }),
        Predicate::And(left, right) => {
            let left_index = push_predicate_node(*left, nodes);
            let right_index = push_predicate_node(*right, nodes);
            FlattenedPredicateNode::PredAnd((left_index, right_index))
        }
        Predicate::Or(left, right) => {
            let left_index = push_predicate_node(*left, nodes);
            let right_index = push_predicate_node(*right, nodes);
            FlattenedPredicateNode::PredOr((left_index, right_index))
        }
        Predicate::Not(inner) => {
            let inner_index = push_predicate_node(*inner, nodes);
            FlattenedPredicateNode::PredNot(inner_index)
        }
        Predicate::True => FlattenedPredicateNode::PredTrue,
        Predicate::False => FlattenedPredicateNode::PredFalse,
    };

    nodes[index as usize] = node;
    index
}

fn build_predicate_from_index(
    nodes: &[FlattenedPredicateNode],
    index: i32,
    visiting: &mut HashSet<i32>,
) -> Result<Predicate, String> {
    if index < 0 || (index as usize) >= nodes.len() {
        return Err(format!("Predicate node index out of range: {index}"));
    }
    if !visiting.insert(index) {
        return Err(format!(
            "Cycle detected in predicate nodes at index {index}"
        ));
    }

    let result = match &nodes[index as usize] {
        FlattenedPredicateNode::PropEq(FlattenedPropertyComparison {
            property_name,
            value,
        }) => Predicate::PropEq {
            property: property_name.clone(),
            value: value.clone(),
        },
        FlattenedPredicateNode::PropNeq(FlattenedPropertyComparison {
            property_name,
            value,
        }) => Predicate::PropNeq {
            property: property_name.clone(),
            value: value.clone(),
        },
        FlattenedPredicateNode::PropGt(FlattenedPropertyComparison {
            property_name,
            value,
        }) => Predicate::PropGt {
            property: property_name.clone(),
            value: value.clone(),
        },
        FlattenedPredicateNode::PropGte(FlattenedPropertyComparison {
            property_name,
            value,
        }) => Predicate::PropGte {
            property: property_name.clone(),
            value: value.clone(),
        },
        FlattenedPredicateNode::PropLt(FlattenedPropertyComparison {
            property_name,
            value,
        }) => Predicate::PropLt {
            property: property_name.clone(),
            value: value.clone(),
        },
        FlattenedPredicateNode::PropLte(FlattenedPropertyComparison {
            property_name,
            value,
        }) => Predicate::PropLte {
            property: property_name.clone(),
            value: value.clone(),
        },
        FlattenedPredicateNode::PropExists(property_name) => {
            Predicate::PropExists(property_name.clone())
        }
        FlattenedPredicateNode::PropIn(FlattenedPropertySetCheck {
            property_name,
            values,
        }) => Predicate::PropIn {
            property: property_name.clone(),
            values: values.clone(),
        },
        FlattenedPredicateNode::PropMatches(FlattenedPropertyPattern {
            property_name,
            pattern,
        }) => Predicate::PropMatches {
            property: property_name.clone(),
            pattern: pattern.clone(),
        },
        FlattenedPredicateNode::PropStartsWith(FlattenedPropertyPattern {
            property_name,
            pattern,
        }) => Predicate::PropStartsWith {
            property: property_name.clone(),
            prefix: pattern.clone(),
        },
        FlattenedPredicateNode::PropContains(FlattenedPropertyPattern {
            property_name,
            pattern,
        }) => Predicate::PropContains {
            property: property_name.clone(),
            substring: pattern.clone(),
        },
        FlattenedPredicateNode::PredAnd((left, right)) => Predicate::And(
            Box::new(build_predicate_from_index(nodes, *left, visiting)?),
            Box::new(build_predicate_from_index(nodes, *right, visiting)?),
        ),
        FlattenedPredicateNode::PredOr((left, right)) => Predicate::Or(
            Box::new(build_predicate_from_index(nodes, *left, visiting)?),
            Box::new(build_predicate_from_index(nodes, *right, visiting)?),
        ),
        FlattenedPredicateNode::PredNot(inner) => Predicate::Not(Box::new(
            build_predicate_from_index(nodes, *inner, visiting)?,
        )),
        FlattenedPredicateNode::PredTrue => Predicate::True,
        FlattenedPredicateNode::PredFalse => Predicate::False,
    };

    visiting.remove(&index);
    Ok(result)
}

fn push_policy_node(policy: RetryPolicy, nodes: &mut Vec<FlattenedPolicyNode>) -> i32 {
    let index = nodes.len() as i32;
    nodes.push(FlattenedPolicyNode::Never);

    let node = match policy {
        RetryPolicy::Periodic(delay) => FlattenedPolicyNode::Periodic(duration_to_nanos(delay)),
        RetryPolicy::Exponential { base_delay, factor } => {
            FlattenedPolicyNode::Exponential(FlattenedExponentialConfig {
                base_delay: duration_to_nanos(base_delay),
                factor,
            })
        }
        RetryPolicy::Fibonacci { first, second } => {
            FlattenedPolicyNode::Fibonacci(FlattenedFibonacciConfig {
                first: duration_to_nanos(first),
                second: duration_to_nanos(second),
            })
        }
        RetryPolicy::Immediate => FlattenedPolicyNode::Immediate,
        RetryPolicy::Never => FlattenedPolicyNode::Never,
        RetryPolicy::CountBox { max_retries, inner } => {
            let inner_index = push_policy_node(*inner, nodes);
            FlattenedPolicyNode::CountBox(FlattenedCountBoxConfig {
                max_retries,
                inner: inner_index,
            })
        }
        RetryPolicy::TimeBox { limit, inner } => {
            let inner_index = push_policy_node(*inner, nodes);
            FlattenedPolicyNode::TimeBox(FlattenedTimeBoxConfig {
                limit: duration_to_nanos(limit),
                inner: inner_index,
            })
        }
        RetryPolicy::Clamp {
            min_delay,
            max_delay,
            inner,
        } => {
            let inner_index = push_policy_node(*inner, nodes);
            FlattenedPolicyNode::ClampDelay(FlattenedClampConfig {
                min_delay: duration_to_nanos(min_delay),
                max_delay: duration_to_nanos(max_delay),
                inner: inner_index,
            })
        }
        RetryPolicy::AddDelay { delay, inner } => {
            let inner_index = push_policy_node(*inner, nodes);
            FlattenedPolicyNode::AddDelay(FlattenedAddDelayConfig {
                delay: duration_to_nanos(delay),
                inner: inner_index,
            })
        }
        RetryPolicy::Jitter { factor, inner } => {
            let inner_index = push_policy_node(*inner, nodes);
            FlattenedPolicyNode::Jitter(FlattenedJitterConfig {
                factor,
                inner: inner_index,
            })
        }
        RetryPolicy::FilteredOn { predicate, inner } => {
            let inner_index = push_policy_node(*inner, nodes);
            FlattenedPolicyNode::FilteredOn(FlattenedFilteredConfig {
                predicate: predicate.into(),
                inner: inner_index,
            })
        }
        RetryPolicy::AndThen(left, right) => {
            let left_index = push_policy_node(*left, nodes);
            let right_index = push_policy_node(*right, nodes);
            FlattenedPolicyNode::AndThen((left_index, right_index))
        }
        RetryPolicy::Union(left, right) => {
            let left_index = push_policy_node(*left, nodes);
            let right_index = push_policy_node(*right, nodes);
            FlattenedPolicyNode::PolicyUnion((left_index, right_index))
        }
        RetryPolicy::Intersect(left, right) => {
            let left_index = push_policy_node(*left, nodes);
            let right_index = push_policy_node(*right, nodes);
            FlattenedPolicyNode::PolicyIntersect((left_index, right_index))
        }
    };

    nodes[index as usize] = node;
    index
}

fn build_policy_from_index(
    nodes: &[FlattenedPolicyNode],
    index: i32,
    visiting: &mut HashSet<i32>,
) -> Result<RetryPolicy, String> {
    if index < 0 || (index as usize) >= nodes.len() {
        return Err(format!("Policy node index out of range: {index}"));
    }
    if !visiting.insert(index) {
        return Err(format!("Cycle detected in policy nodes at index {index}"));
    }

    let result = match &nodes[index as usize] {
        FlattenedPolicyNode::Periodic(delay) => RetryPolicy::Periodic(nanos_to_duration(*delay)),
        FlattenedPolicyNode::Exponential(FlattenedExponentialConfig { base_delay, factor }) => {
            RetryPolicy::Exponential {
                base_delay: nanos_to_duration(*base_delay),
                factor: *factor,
            }
        }
        FlattenedPolicyNode::Fibonacci(FlattenedFibonacciConfig { first, second }) => {
            RetryPolicy::Fibonacci {
                first: nanos_to_duration(*first),
                second: nanos_to_duration(*second),
            }
        }
        FlattenedPolicyNode::Immediate => RetryPolicy::Immediate,
        FlattenedPolicyNode::Never => RetryPolicy::Never,
        FlattenedPolicyNode::CountBox(FlattenedCountBoxConfig { max_retries, inner }) => {
            RetryPolicy::CountBox {
                max_retries: *max_retries,
                inner: Box::new(build_policy_from_index(nodes, *inner, visiting)?),
            }
        }
        FlattenedPolicyNode::TimeBox(FlattenedTimeBoxConfig { limit, inner }) => {
            RetryPolicy::TimeBox {
                limit: nanos_to_duration(*limit),
                inner: Box::new(build_policy_from_index(nodes, *inner, visiting)?),
            }
        }
        FlattenedPolicyNode::ClampDelay(FlattenedClampConfig {
            min_delay,
            max_delay,
            inner,
        }) => RetryPolicy::Clamp {
            min_delay: nanos_to_duration(*min_delay),
            max_delay: nanos_to_duration(*max_delay),
            inner: Box::new(build_policy_from_index(nodes, *inner, visiting)?),
        },
        FlattenedPolicyNode::AddDelay(FlattenedAddDelayConfig { delay, inner }) => {
            RetryPolicy::AddDelay {
                delay: nanos_to_duration(*delay),
                inner: Box::new(build_policy_from_index(nodes, *inner, visiting)?),
            }
        }
        FlattenedPolicyNode::Jitter(FlattenedJitterConfig { factor, inner }) => {
            RetryPolicy::Jitter {
                factor: *factor,
                inner: Box::new(build_policy_from_index(nodes, *inner, visiting)?),
            }
        }
        FlattenedPolicyNode::FilteredOn(FlattenedFilteredConfig { predicate, inner }) => {
            RetryPolicy::FilteredOn {
                predicate: predicate.clone().try_into()?,
                inner: Box::new(build_policy_from_index(nodes, *inner, visiting)?),
            }
        }
        FlattenedPolicyNode::AndThen((left, right)) => RetryPolicy::AndThen(
            Box::new(build_policy_from_index(nodes, *left, visiting)?),
            Box::new(build_policy_from_index(nodes, *right, visiting)?),
        ),
        FlattenedPolicyNode::PolicyUnion((left, right)) => RetryPolicy::Union(
            Box::new(build_policy_from_index(nodes, *left, visiting)?),
            Box::new(build_policy_from_index(nodes, *right, visiting)?),
        ),
        FlattenedPolicyNode::PolicyIntersect((left, right)) => RetryPolicy::Intersect(
            Box::new(build_policy_from_index(nodes, *left, visiting)?),
            Box::new(build_policy_from_index(nodes, *right, visiting)?),
        ),
    };

    visiting.remove(&index);
    Ok(result)
}

pub fn duration_to_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

fn nanos_to_duration(nanos: u64) -> Duration {
    Duration::from_nanos(nanos)
}

fn get_required_property<'a>(
    props: &'a RetryProperties,
    property: &str,
) -> Result<&'a PredicateValue, RetryEvaluationError> {
    props
        .get(property)
        .ok_or_else(|| RetryEvaluationError::PropertyNotFound {
            property: property.to_string(),
        })
}

fn scale_duration(duration: Duration, factor: f64) -> Duration {
    if !factor.is_finite() || factor <= 0.0 {
        return Duration::ZERO;
    }

    let base = duration.as_secs_f64();
    let value = base * factor;

    if !value.is_finite() {
        Duration::MAX
    } else {
        Duration::from_secs_f64(value.min(Duration::MAX.as_secs_f64()))
    }
}

fn saturating_add_duration(left: Duration, right: Duration) -> Duration {
    left.checked_add(right).unwrap_or(Duration::MAX)
}

fn fibonacci_delay(first: Duration, second: Duration, nth: u32) -> Duration {
    match nth {
        0 => Duration::ZERO,
        1 => first,
        2 => second,
        _ => {
            let mut left = first;
            let mut right = second;

            for _ in 3..=nth {
                let next = saturating_add_duration(left, right);
                left = right;
                right = next;
            }

            right
        }
    }
}

fn coerce_cmp(
    actual: &PredicateValue,
    expected: &PredicateValue,
    property: &str,
) -> Result<Ordering, RetryEvaluationError> {
    match (actual, expected) {
        (PredicateValue::Integer(left), PredicateValue::Integer(right)) => Ok(left.cmp(right)),
        (PredicateValue::Text(left), PredicateValue::Text(right)) => Ok(left.cmp(right)),
        (PredicateValue::Boolean(left), PredicateValue::Boolean(right)) => Ok(left.cmp(right)),
        (PredicateValue::Text(left), PredicateValue::Integer(right)) => left
            .parse::<i64>()
            .map(|left| left.cmp(right))
            .map_err(|_| RetryEvaluationError::CoercionFailed {
                property: property.to_string(),
                value: actual.clone(),
                target_type: PredicateValueType::Integer,
            }),
        (PredicateValue::Integer(left), PredicateValue::Text(right)) => {
            Ok(left.to_string().cmp(right))
        }
        (_, expected) => Err(RetryEvaluationError::CoercionFailed {
            property: property.to_string(),
            value: actual.clone(),
            target_type: expected.value_type(),
        }),
    }
}

fn coerce_eq(
    actual: &PredicateValue,
    expected: &PredicateValue,
    property: &str,
) -> Result<bool, RetryEvaluationError> {
    Ok(coerce_cmp(actual, expected, property)? == Ordering::Equal)
}

fn flatten_element_value(props: &mut RetryProperties, key: &str, value: &ElementValue) {
    match value {
        ElementValue::ComponentModel(value) => {
            flatten_component_model_value(props, key, &value.value.value);
        }
        ElementValue::UnstructuredText(value) => {
            let text = match &value.value {
                crate::model::agent::TextReference::Url(url) => url.value.clone(),
                crate::model::agent::TextReference::Inline(source) => source.data.clone(),
            };
            props.set(key.to_string(), PredicateValue::Text(text));
        }
        ElementValue::UnstructuredBinary(_) => {
            // Binary values are intentionally skipped.
        }
    }
}

fn flatten_component_model_value(props: &mut RetryProperties, key: &str, value: &Value) {
    if let Some(predicate_value) = primitive_predicate_value(value) {
        props.set(key.to_string(), predicate_value);
        return;
    }

    match value {
        Value::Tuple(values) | Value::List(values) | Value::Record(values) => {
            for (index, value) in values.iter().enumerate() {
                flatten_component_model_value(props, &format!("{key}.{index}"), value);
            }
        }
        Value::Option(Some(value)) => flatten_component_model_value(props, key, value),
        Value::Variant {
            case_idx,
            case_value,
        } => {
            props.set(
                format!("{key}.case"),
                PredicateValue::Integer(*case_idx as i64),
            );
            if let Some(case_value) = case_value {
                flatten_component_model_value(props, &format!("{key}.value"), case_value);
            }
        }
        Value::Enum(value) => {
            props.set(key.to_string(), PredicateValue::Integer(*value as i64));
        }
        Value::Flags(values) => {
            for (index, value) in values.iter().enumerate() {
                props.set(format!("{key}.{index}"), PredicateValue::Boolean(*value));
            }
        }
        Value::Result(result) => match result {
            Ok(Some(ok)) => flatten_component_model_value(props, &format!("{key}.ok"), ok),
            Ok(None) => {
                props.set(
                    format!("{key}.ok"),
                    PredicateValue::Text("none".to_string()),
                );
            }
            Err(Some(err)) => flatten_component_model_value(props, &format!("{key}.err"), err),
            Err(None) => {
                props.set(
                    format!("{key}.err"),
                    PredicateValue::Text("none".to_string()),
                );
            }
        },
        Value::Handle { .. } | Value::Option(None) => {}
        _ => {}
    }
}

fn primitive_predicate_value(value: &Value) -> Option<PredicateValue> {
    match value {
        Value::Bool(value) => Some(PredicateValue::Boolean(*value)),
        Value::String(value) => Some(PredicateValue::Text(value.clone())),
        Value::S8(value) => Some(PredicateValue::Integer(*value as i64)),
        Value::S16(value) => Some(PredicateValue::Integer(*value as i64)),
        Value::S32(value) => Some(PredicateValue::Integer(*value as i64)),
        Value::S64(value) => Some(PredicateValue::Integer(*value)),
        Value::U8(value) => Some(PredicateValue::Integer(*value as i64)),
        Value::U16(value) => Some(PredicateValue::Integer(*value as i64)),
        Value::U32(value) => Some(PredicateValue::Integer(*value as i64)),
        Value::U64(value) => i64::try_from(*value).ok().map(PredicateValue::Integer),
        Value::F32(value) => Some(PredicateValue::Text(value.to_string())),
        Value::F64(value) => Some(PredicateValue::Text(value.to_string())),
        Value::Char(value) => Some(PredicateValue::Text(value.to_string())),
        _ => None,
    }
}

#[cfg(feature = "full")]
impl From<PredicateValue> for golem_api_grpc::proto::golem::worker::retry::PredicateValue {
    fn from(value: PredicateValue) -> Self {
        use golem_api_grpc::proto::golem::worker::retry::predicate_value::Value;

        Self {
            value: Some(match value {
                PredicateValue::Text(text) => Value::Text(text),
                PredicateValue::Integer(integer) => Value::Integer(integer),
                PredicateValue::Boolean(boolean) => Value::Boolean(boolean),
            }),
        }
    }
}

#[cfg(feature = "full")]
impl TryFrom<golem_api_grpc::proto::golem::worker::retry::PredicateValue> for PredicateValue {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::retry::PredicateValue,
    ) -> Result<Self, Self::Error> {
        match value.value {
            Some(golem_api_grpc::proto::golem::worker::retry::predicate_value::Value::Text(
                text,
            )) => Ok(PredicateValue::Text(text)),
            Some(golem_api_grpc::proto::golem::worker::retry::predicate_value::Value::Integer(
                integer,
            )) => Ok(PredicateValue::Integer(integer)),
            Some(golem_api_grpc::proto::golem::worker::retry::predicate_value::Value::Boolean(
                boolean,
            )) => Ok(PredicateValue::Boolean(boolean)),
            None => Err("Missing predicate value".to_string()),
        }
    }
}

#[cfg(feature = "full")]
impl From<Predicate> for golem_api_grpc::proto::golem::worker::retry::Predicate {
    fn from(value: Predicate) -> Self {
        use golem_api_grpc::proto::golem::worker::retry::predicate::Predicate as ProtoPredicate;

        Self {
            predicate: Some(match value {
                Predicate::PropEq { property, value } => ProtoPredicate::PropEq(
                    golem_api_grpc::proto::golem::worker::retry::PropertyComparison {
                        property_name: property,
                        value: Some(value.into()),
                    },
                ),
                Predicate::PropNeq { property, value } => ProtoPredicate::PropNeq(
                    golem_api_grpc::proto::golem::worker::retry::PropertyComparison {
                        property_name: property,
                        value: Some(value.into()),
                    },
                ),
                Predicate::PropGt { property, value } => ProtoPredicate::PropGt(
                    golem_api_grpc::proto::golem::worker::retry::PropertyComparison {
                        property_name: property,
                        value: Some(value.into()),
                    },
                ),
                Predicate::PropGte { property, value } => ProtoPredicate::PropGte(
                    golem_api_grpc::proto::golem::worker::retry::PropertyComparison {
                        property_name: property,
                        value: Some(value.into()),
                    },
                ),
                Predicate::PropLt { property, value } => ProtoPredicate::PropLt(
                    golem_api_grpc::proto::golem::worker::retry::PropertyComparison {
                        property_name: property,
                        value: Some(value.into()),
                    },
                ),
                Predicate::PropLte { property, value } => ProtoPredicate::PropLte(
                    golem_api_grpc::proto::golem::worker::retry::PropertyComparison {
                        property_name: property,
                        value: Some(value.into()),
                    },
                ),
                Predicate::PropExists(property) => ProtoPredicate::PropExists(property),
                Predicate::PropIn { property, values } => ProtoPredicate::PropIn(
                    golem_api_grpc::proto::golem::worker::retry::PropertySetCheck {
                        property_name: property,
                        values: values.into_iter().map(Into::into).collect(),
                    },
                ),
                Predicate::PropMatches { property, pattern } => ProtoPredicate::PropMatches(
                    golem_api_grpc::proto::golem::worker::retry::PropertyPattern {
                        property_name: property,
                        pattern,
                    },
                ),
                Predicate::PropStartsWith { property, prefix } => ProtoPredicate::PropStartsWith(
                    golem_api_grpc::proto::golem::worker::retry::PropertyPattern {
                        property_name: property,
                        pattern: prefix,
                    },
                ),
                Predicate::PropContains {
                    property,
                    substring,
                } => ProtoPredicate::PropContains(
                    golem_api_grpc::proto::golem::worker::retry::PropertyPattern {
                        property_name: property,
                        pattern: substring,
                    },
                ),
                Predicate::And(left, right) => ProtoPredicate::PredAnd(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::PredicatePair {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                )),
                Predicate::Or(left, right) => ProtoPredicate::PredOr(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::PredicatePair {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                )),
                Predicate::Not(inner) => ProtoPredicate::PredNot(Box::new((*inner).into())),
                Predicate::True => ProtoPredicate::PredTrue(()),
                Predicate::False => ProtoPredicate::PredFalse(()),
            }),
        }
    }
}

#[cfg(feature = "full")]
impl TryFrom<golem_api_grpc::proto::golem::worker::retry::Predicate> for Predicate {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::retry::Predicate,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::retry::predicate::Predicate as ProtoPredicate;

        match value
            .predicate
            .ok_or("Missing predicate field".to_string())?
        {
            ProtoPredicate::PropEq(comparison) => Ok(Predicate::PropEq {
                property: comparison.property_name,
                value: comparison
                    .value
                    .ok_or("Missing prop-eq.value field".to_string())?
                    .try_into()?,
            }),
            ProtoPredicate::PropNeq(comparison) => Ok(Predicate::PropNeq {
                property: comparison.property_name,
                value: comparison
                    .value
                    .ok_or("Missing prop-neq.value field".to_string())?
                    .try_into()?,
            }),
            ProtoPredicate::PropGt(comparison) => Ok(Predicate::PropGt {
                property: comparison.property_name,
                value: comparison
                    .value
                    .ok_or("Missing prop-gt.value field".to_string())?
                    .try_into()?,
            }),
            ProtoPredicate::PropGte(comparison) => Ok(Predicate::PropGte {
                property: comparison.property_name,
                value: comparison
                    .value
                    .ok_or("Missing prop-gte.value field".to_string())?
                    .try_into()?,
            }),
            ProtoPredicate::PropLt(comparison) => Ok(Predicate::PropLt {
                property: comparison.property_name,
                value: comparison
                    .value
                    .ok_or("Missing prop-lt.value field".to_string())?
                    .try_into()?,
            }),
            ProtoPredicate::PropLte(comparison) => Ok(Predicate::PropLte {
                property: comparison.property_name,
                value: comparison
                    .value
                    .ok_or("Missing prop-lte.value field".to_string())?
                    .try_into()?,
            }),
            ProtoPredicate::PropExists(property) => Ok(Predicate::PropExists(property)),
            ProtoPredicate::PropIn(set_check) => Ok(Predicate::PropIn {
                property: set_check.property_name,
                values: set_check
                    .values
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<Vec<_>, _>>()?,
            }),
            ProtoPredicate::PropMatches(pattern) => Ok(Predicate::PropMatches {
                property: pattern.property_name,
                pattern: pattern.pattern,
            }),
            ProtoPredicate::PropStartsWith(pattern) => Ok(Predicate::PropStartsWith {
                property: pattern.property_name,
                prefix: pattern.pattern,
            }),
            ProtoPredicate::PropContains(pattern) => Ok(Predicate::PropContains {
                property: pattern.property_name,
                substring: pattern.pattern,
            }),
            ProtoPredicate::PredAnd(pair) => Ok(Predicate::And(
                Box::new(
                    (*pair.left.ok_or("Missing pred-and.left field".to_string())?).try_into()?,
                ),
                Box::new(
                    (*pair
                        .right
                        .ok_or("Missing pred-and.right field".to_string())?)
                    .try_into()?,
                ),
            )),
            ProtoPredicate::PredOr(pair) => Ok(Predicate::Or(
                Box::new((*pair.left.ok_or("Missing pred-or.left field".to_string())?).try_into()?),
                Box::new(
                    (*pair
                        .right
                        .ok_or("Missing pred-or.right field".to_string())?)
                    .try_into()?,
                ),
            )),
            ProtoPredicate::PredNot(inner) => Ok(Predicate::Not(Box::new((*inner).try_into()?))),
            ProtoPredicate::PredTrue(()) => Ok(Predicate::True),
            ProtoPredicate::PredFalse(()) => Ok(Predicate::False),
        }
    }
}

#[cfg(feature = "full")]
impl From<RetryPolicy> for golem_api_grpc::proto::golem::worker::retry::RetryPolicy {
    fn from(value: RetryPolicy) -> Self {
        use golem_api_grpc::proto::golem::worker::retry::retry_policy::Policy as ProtoPolicy;

        Self {
            policy: Some(match value {
                RetryPolicy::Periodic(delay) => ProtoPolicy::Periodic(duration_to_nanos(delay)),
                RetryPolicy::Exponential { base_delay, factor } => ProtoPolicy::Exponential(
                    golem_api_grpc::proto::golem::worker::retry::ExponentialConfig {
                        base_delay: duration_to_nanos(base_delay),
                        factor,
                    },
                ),
                RetryPolicy::Fibonacci { first, second } => ProtoPolicy::Fibonacci(
                    golem_api_grpc::proto::golem::worker::retry::FibonacciConfig {
                        first: duration_to_nanos(first),
                        second: duration_to_nanos(second),
                    },
                ),
                RetryPolicy::Immediate => ProtoPolicy::Immediate(()),
                RetryPolicy::Never => ProtoPolicy::Never(()),
                RetryPolicy::CountBox { max_retries, inner } => ProtoPolicy::CountBox(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::CountBoxConfig {
                        max_retries,
                        inner: Some(Box::new((*inner).into())),
                    },
                )),
                RetryPolicy::TimeBox { limit, inner } => ProtoPolicy::TimeBox(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::TimeBoxConfig {
                        limit: duration_to_nanos(limit),
                        inner: Some(Box::new((*inner).into())),
                    },
                )),
                RetryPolicy::Clamp {
                    min_delay,
                    max_delay,
                    inner,
                } => ProtoPolicy::Clamp(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::ClampConfig {
                        min_delay: duration_to_nanos(min_delay),
                        max_delay: duration_to_nanos(max_delay),
                        inner: Some(Box::new((*inner).into())),
                    },
                )),
                RetryPolicy::AddDelay { delay, inner } => ProtoPolicy::AddDelay(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::AddDelayConfig {
                        delay: duration_to_nanos(delay),
                        inner: Some(Box::new((*inner).into())),
                    },
                )),
                RetryPolicy::Jitter { factor, inner } => ProtoPolicy::Jitter(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::JitterConfig {
                        factor,
                        inner: Some(Box::new((*inner).into())),
                    },
                )),
                RetryPolicy::FilteredOn { predicate, inner } => ProtoPolicy::FilteredOn(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::FilteredConfig {
                        predicate: Some(predicate.into()),
                        inner: Some(Box::new((*inner).into())),
                    },
                )),
                RetryPolicy::AndThen(left, right) => ProtoPolicy::AndThen(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::RetryPolicyPair {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                )),
                RetryPolicy::Union(left, right) => ProtoPolicy::PolicyUnion(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::RetryPolicyPair {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                )),
                RetryPolicy::Intersect(left, right) => ProtoPolicy::PolicyIntersect(Box::new(
                    golem_api_grpc::proto::golem::worker::retry::RetryPolicyPair {
                        left: Some(Box::new((*left).into())),
                        right: Some(Box::new((*right).into())),
                    },
                )),
            }),
        }
    }
}

#[cfg(feature = "full")]
impl TryFrom<golem_api_grpc::proto::golem::worker::retry::RetryPolicy> for RetryPolicy {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::retry::RetryPolicy,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::worker::retry::retry_policy::Policy as ProtoPolicy;

        match value
            .policy
            .ok_or("Missing retry policy field".to_string())?
        {
            ProtoPolicy::Periodic(delay) => Ok(RetryPolicy::Periodic(nanos_to_duration(delay))),
            ProtoPolicy::Exponential(config) => Ok(RetryPolicy::Exponential {
                base_delay: nanos_to_duration(config.base_delay),
                factor: config.factor,
            }),
            ProtoPolicy::Fibonacci(config) => Ok(RetryPolicy::Fibonacci {
                first: nanos_to_duration(config.first),
                second: nanos_to_duration(config.second),
            }),
            ProtoPolicy::Immediate(()) => Ok(RetryPolicy::Immediate),
            ProtoPolicy::Never(()) => Ok(RetryPolicy::Never),
            ProtoPolicy::CountBox(config) => Ok(RetryPolicy::CountBox {
                max_retries: config.max_retries,
                inner: Box::new(
                    (*config
                        .inner
                        .ok_or("Missing count-box.inner field".to_string())?)
                    .try_into()?,
                ),
            }),
            ProtoPolicy::TimeBox(config) => Ok(RetryPolicy::TimeBox {
                limit: nanos_to_duration(config.limit),
                inner: Box::new(
                    (*config
                        .inner
                        .ok_or("Missing time-box.inner field".to_string())?)
                    .try_into()?,
                ),
            }),
            ProtoPolicy::Clamp(config) => Ok(RetryPolicy::Clamp {
                min_delay: nanos_to_duration(config.min_delay),
                max_delay: nanos_to_duration(config.max_delay),
                inner: Box::new(
                    (*config
                        .inner
                        .ok_or("Missing clamp.inner field".to_string())?)
                    .try_into()?,
                ),
            }),
            ProtoPolicy::AddDelay(config) => Ok(RetryPolicy::AddDelay {
                delay: nanos_to_duration(config.delay),
                inner: Box::new(
                    (*config
                        .inner
                        .ok_or("Missing add-delay.inner field".to_string())?)
                    .try_into()?,
                ),
            }),
            ProtoPolicy::Jitter(config) => Ok(RetryPolicy::Jitter {
                factor: config.factor,
                inner: Box::new(
                    (*config
                        .inner
                        .ok_or("Missing jitter.inner field".to_string())?)
                    .try_into()?,
                ),
            }),
            ProtoPolicy::FilteredOn(config) => Ok(RetryPolicy::FilteredOn {
                predicate: config
                    .predicate
                    .ok_or("Missing filtered-on.predicate field".to_string())?
                    .try_into()?,
                inner: Box::new(
                    (*config
                        .inner
                        .ok_or("Missing filtered-on.inner field".to_string())?)
                    .try_into()?,
                ),
            }),
            ProtoPolicy::AndThen(pair) => Ok(RetryPolicy::AndThen(
                Box::new(
                    (*pair.left.ok_or("Missing and-then.left field".to_string())?).try_into()?,
                ),
                Box::new(
                    (*pair
                        .right
                        .ok_or("Missing and-then.right field".to_string())?)
                    .try_into()?,
                ),
            )),
            ProtoPolicy::PolicyUnion(pair) => Ok(RetryPolicy::Union(
                Box::new(
                    (*pair
                        .left
                        .ok_or("Missing policy-union.left field".to_string())?)
                    .try_into()?,
                ),
                Box::new(
                    (*pair
                        .right
                        .ok_or("Missing policy-union.right field".to_string())?)
                    .try_into()?,
                ),
            )),
            ProtoPolicy::PolicyIntersect(pair) => Ok(RetryPolicy::Intersect(
                Box::new(
                    (*pair
                        .left
                        .ok_or("Missing policy-intersect.left field".to_string())?)
                    .try_into()?,
                ),
                Box::new(
                    (*pair
                        .right
                        .ok_or("Missing policy-intersect.right field".to_string())?)
                    .try_into()?,
                ),
            )),
        }
    }
}

#[cfg(feature = "full")]
impl From<NamedRetryPolicy> for golem_api_grpc::proto::golem::worker::retry::NamedRetryPolicy {
    fn from(value: NamedRetryPolicy) -> Self {
        Self {
            name: value.name,
            priority: value.priority,
            predicate: Some(value.predicate.into()),
            policy: Some(value.policy.into()),
        }
    }
}

#[cfg(feature = "full")]
impl TryFrom<golem_api_grpc::proto::golem::worker::retry::NamedRetryPolicy> for NamedRetryPolicy {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::retry::NamedRetryPolicy,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            priority: value.priority,
            predicate: value
                .predicate
                .ok_or("Missing named-retry-policy.predicate field".to_string())?
                .try_into()?,
            policy: value
                .policy
                .ok_or("Missing named-retry-policy.policy field".to_string())?
                .try_into()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn periodic_strategy_retries_with_constant_delay() {
        let policy = RetryPolicy::Periodic(Duration::from_millis(10));
        let mut state = policy.initial_state();
        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();

        for _ in 0..3 {
            let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
            state = new_state;
            assert_eq!(verdict, RetryVerdict::Retry(Duration::from_millis(10)));
        }
    }

    #[test]
    fn exponential_strategy_scales_delay() {
        let policy = RetryPolicy::Exponential {
            base_delay: Duration::from_millis(10),
            factor: 2.0,
        };
        let mut state = policy.initial_state();
        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();

        let mut delays = Vec::new();
        for _ in 0..4 {
            let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
            state = new_state;
            match verdict {
                RetryVerdict::Retry(delay) => delays.push(delay),
                other => panic!("expected retry verdict, got {other:?}"),
            }
        }

        assert_eq!(
            delays,
            vec![
                Duration::from_millis(10),
                Duration::from_millis(20),
                Duration::from_millis(40),
                Duration::from_millis(80)
            ]
        );
    }

    #[test]
    fn fibonacci_strategy_scales_delay() {
        let policy = RetryPolicy::Fibonacci {
            first: Duration::from_millis(5),
            second: Duration::from_millis(10),
        };
        let mut state = policy.initial_state();
        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();

        let mut delays = Vec::new();
        for _ in 0..6 {
            let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
            state = new_state;
            match verdict {
                RetryVerdict::Retry(delay) => delays.push(delay),
                other => panic!("expected retry verdict, got {other:?}"),
            }
        }

        assert_eq!(
            delays,
            vec![
                Duration::from_millis(5),
                Duration::from_millis(10),
                Duration::from_millis(15),
                Duration::from_millis(25),
                Duration::from_millis(40),
                Duration::from_millis(65)
            ]
        );
    }

    #[test]
    fn count_box_exhausts_after_max_retries() {
        let policy = RetryPolicy::CountBox {
            max_retries: 2,
            inner: Box::new(RetryPolicy::Immediate),
        };
        let mut state = policy.initial_state();
        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();

        let (state_1, verdict_1) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict_1, RetryVerdict::Retry(Duration::ZERO));

        let (state_2, verdict_2) = policy.step(&state_1, Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict_2, RetryVerdict::Retry(Duration::ZERO));

        let (_, verdict_3) = policy.step(&state_2, Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict_3, RetryVerdict::GiveUp);

        state = state_2;
        let _ = state;
    }

    #[test]
    fn time_box_exhausts_after_elapsed_limit() {
        let policy = RetryPolicy::TimeBox {
            limit: Duration::from_secs(5),
            inner: Box::new(RetryPolicy::Immediate),
        };
        let state = policy.initial_state();
        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();

        let (_, verdict_before_limit) =
            policy.step(&state, Duration::from_secs(4), &props, &mut rng);
        assert_eq!(verdict_before_limit, RetryVerdict::Retry(Duration::ZERO));

        let (_, verdict_after_limit) =
            policy.step(&state, Duration::from_secs(5), &props, &mut rng);
        assert_eq!(verdict_after_limit, RetryVerdict::GiveUp);
    }

    #[test]
    fn clamp_add_delay_and_jitter_transform_retry_delay() {
        let policy = RetryPolicy::Jitter {
            factor: 0.1,
            inner: Box::new(RetryPolicy::AddDelay {
                delay: Duration::from_millis(5),
                inner: Box::new(RetryPolicy::Clamp {
                    min_delay: Duration::from_millis(10),
                    max_delay: Duration::from_millis(30),
                    inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(50))),
                }),
            }),
        };

        let mut rng = FixedRng(0.1);
        let props = RetryProperties::new();
        let state = policy.initial_state();
        let (_, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);

        assert_eq!(verdict, RetryVerdict::Retry(Duration::from_micros(38_500)));
    }

    #[test]
    fn filtered_on_uses_predicate_and_propagates_errors() {
        let policy = RetryPolicy::FilteredOn {
            predicate: Predicate::PropEq {
                property: "status-code".to_string(),
                value: PredicateValue::Integer(503),
            },
            inner: Box::new(RetryPolicy::Immediate),
        };

        let mut rng = FixedRng(0.0);
        let mut props = RetryProperties::new();
        props.set("status-code", PredicateValue::Integer(503));

        let state = policy.initial_state();
        let (_, verdict_ok) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict_ok, RetryVerdict::Retry(Duration::ZERO));

        props.set("status-code", PredicateValue::Integer(404));
        let (_, verdict_give_up) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict_give_up, RetryVerdict::GiveUp);

        let empty_props = RetryProperties::new();
        let (_, verdict_err) = policy.step(&state, Duration::ZERO, &empty_props, &mut rng);
        match verdict_err {
            RetryVerdict::Error(RetryEvaluationError::PropertyNotFound { property }) => {
                assert_eq!(property, "status-code");
            }
            other => panic!("expected property-not-found error, got {other:?}"),
        }
    }

    #[test]
    fn and_then_hands_off_to_right_immediately() {
        let left = RetryPolicy::CountBox {
            max_retries: 1,
            inner: Box::new(RetryPolicy::Immediate),
        };
        let right = RetryPolicy::Periodic(Duration::from_millis(20));
        let policy = RetryPolicy::AndThen(Box::new(left), Box::new(right));

        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();
        let state = policy.initial_state();

        let (state_1, verdict_1) = policy.step(&state, Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict_1, RetryVerdict::Retry(Duration::ZERO));

        let (_, verdict_2) = policy.step(&state_1, Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict_2, RetryVerdict::Retry(Duration::from_millis(20)));
    }

    #[test]
    fn union_and_intersect_combine_delays() {
        let left = RetryPolicy::CountBox {
            max_retries: 1,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(10))),
        };
        let right = RetryPolicy::Periodic(Duration::from_millis(20));

        let union = RetryPolicy::Union(Box::new(left.clone()), Box::new(right.clone()));
        let intersect = RetryPolicy::Intersect(Box::new(left), Box::new(right));

        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();

        let (_, union_first) = union.step(&union.initial_state(), Duration::ZERO, &props, &mut rng);
        assert_eq!(union_first, RetryVerdict::Retry(Duration::from_millis(10)));

        let (_, intersect_first) =
            intersect.step(&intersect.initial_state(), Duration::ZERO, &props, &mut rng);
        assert_eq!(
            intersect_first,
            RetryVerdict::Retry(Duration::from_millis(20))
        );
    }

    #[test]
    fn predicate_comparisons_and_text_operators_work() {
        let mut props = RetryProperties::new();
        props
            .set("status", PredicateValue::Integer(503))
            .set("service", PredicateValue::Text("billing-api".to_string()));

        assert!(Predicate::PropGt {
            property: "status".to_string(),
            value: PredicateValue::Integer(500),
        }
        .matches(&props)
        .unwrap());

        assert!(Predicate::PropMatches {
            property: "service".to_string(),
            pattern: "billing-*".to_string(),
        }
        .matches(&props)
        .unwrap());

        assert!(Predicate::PropStartsWith {
            property: "service".to_string(),
            prefix: "bill".to_string(),
        }
        .matches(&props)
        .unwrap());

        assert!(Predicate::PropContains {
            property: "service".to_string(),
            substring: "api".to_string(),
        }
        .matches(&props)
        .unwrap());
    }

    #[test]
    fn coercion_supports_text_integer_and_rejects_boolean_to_integer() {
        let mut props = RetryProperties::new();
        props.set("attempt", PredicateValue::Text("42".to_string()));

        assert!(Predicate::PropEq {
            property: "attempt".to_string(),
            value: PredicateValue::Integer(42),
        }
        .matches(&props)
        .unwrap());

        props.set("attempt", PredicateValue::Integer(42));
        assert!(Predicate::PropEq {
            property: "attempt".to_string(),
            value: PredicateValue::Text("42".to_string()),
        }
        .matches(&props)
        .unwrap());

        props.set("attempt", PredicateValue::Boolean(true));
        let error = Predicate::PropEq {
            property: "attempt".to_string(),
            value: PredicateValue::Integer(42),
        }
        .matches(&props)
        .unwrap_err();

        match error {
            RetryEvaluationError::CoercionFailed {
                property,
                target_type,
                ..
            } => {
                assert_eq!(property, "attempt");
                assert_eq!(target_type, PredicateValueType::Integer);
            }
            other => panic!("expected coercion error, got {other:?}"),
        }
    }

    #[test]
    fn prop_in_propagates_coercion_error() {
        let mut props = RetryProperties::new();
        props.set("attempt", PredicateValue::Boolean(true));

        let error = Predicate::PropIn {
            property: "attempt".to_string(),
            values: vec![PredicateValue::Integer(1), PredicateValue::Integer(2)],
        }
        .matches(&props)
        .unwrap_err();

        assert!(matches!(
            error,
            RetryEvaluationError::CoercionFailed {
                target_type: PredicateValueType::Integer,
                ..
            }
        ));
    }

    #[test]
    fn errors_propagate_through_filtered_union_and_intersect() {
        let missing_predicate = Predicate::PropEq {
            property: "missing".to_string(),
            value: PredicateValue::Integer(1),
        };
        let filtered = RetryPolicy::FilteredOn {
            predicate: missing_predicate,
            inner: Box::new(RetryPolicy::Immediate),
        };
        let union =
            RetryPolicy::Union(Box::new(filtered.clone()), Box::new(RetryPolicy::Immediate));
        let intersect =
            RetryPolicy::Intersect(Box::new(filtered), Box::new(RetryPolicy::Immediate));

        let props = RetryProperties::new();
        let mut rng = FixedRng(0.0);

        let (_, union_verdict) =
            union.step(&union.initial_state(), Duration::ZERO, &props, &mut rng);
        assert!(matches!(
            union_verdict,
            RetryVerdict::Error(RetryEvaluationError::PropertyNotFound { .. })
        ));

        let (_, intersect_verdict) =
            intersect.step(&intersect.initial_state(), Duration::ZERO, &props, &mut rng);
        assert!(matches!(
            intersect_verdict,
            RetryVerdict::Error(RetryEvaluationError::PropertyNotFound { .. })
        ));
    }

    #[test]
    fn reconstruct_state_from_count_replays_steps() {
        let policy = RetryPolicy::CountBox {
            max_retries: 10,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(5))),
        };
        let props = RetryProperties::new();
        let mut rng = FixedRng(0.0);

        let reconstructed = policy
            .reconstruct_state_from_count(3, Duration::ZERO, &props, &mut rng)
            .expect("state reconstruction should succeed");

        match reconstructed {
            RetryPolicyState::CountBox { attempts, .. } => assert_eq!(attempts, 3),
            other => panic!("unexpected reconstructed state: {other:?}"),
        }
    }

    #[test]
    fn named_policy_resolution_respects_priority_order() {
        let high = NamedRetryPolicy {
            name: "high".to_string(),
            priority: 100,
            predicate: Predicate::PropEq {
                property: "verb".to_string(),
                value: PredicateValue::Text("invoke".to_string()),
            },
            policy: RetryPolicy::Immediate,
        };
        let low = NamedRetryPolicy {
            name: "low".to_string(),
            priority: 10,
            predicate: Predicate::True,
            policy: RetryPolicy::Never,
        };

        let mut props = RetryProperties::new();
        props.set("verb", PredicateValue::Text("invoke".to_string()));

        let policies = vec![high.clone(), low.clone()];
        let resolved = NamedRetryPolicy::resolve(&policies, &props)
            .expect("resolution should succeed")
            .expect("matching policy is expected");
        assert_eq!(resolved.name, "high");

        props.set("verb", PredicateValue::Text("query".to_string()));
        let resolved = NamedRetryPolicy::resolve(&policies, &props)
            .expect("resolution should succeed")
            .expect("fallback policy is expected");
        assert_eq!(resolved.name, "low");

        let no_match_policies = vec![NamedRetryPolicy {
            name: "none".to_string(),
            priority: 0,
            predicate: Predicate::False,
            policy: RetryPolicy::Never,
        }];
        let no_match = NamedRetryPolicy::resolve(&no_match_policies, &props)
            .expect("resolution should succeed");
        assert!(no_match.is_none());
    }

    #[test]
    fn fixed_rng_makes_jitter_deterministic() {
        let policy = RetryPolicy::Jitter {
            factor: 0.5,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(100))),
        };

        let mut rng = FixedRng(0.25);
        let props = RetryProperties::new();
        let (_, verdict) = policy.step(&policy.initial_state(), Duration::ZERO, &props, &mut rng);
        assert_eq!(verdict, RetryVerdict::Retry(Duration::from_millis(125)));
    }

    #[test]
    fn retry_context_populates_expected_http_fields() {
        let props = RetryContext::http_with_response(
            "GET",
            "https://api.example.com:8443/v1/items",
            Some(503),
            "transient",
        );

        assert_eq!(
            props.get("verb"),
            Some(&PredicateValue::Text("GET".to_string()))
        );
        assert_eq!(
            props.get("uri-scheme"),
            Some(&PredicateValue::Text("https".to_string()))
        );
        assert_eq!(
            props.get("uri-host"),
            Some(&PredicateValue::Text("api.example.com".to_string()))
        );
        assert_eq!(props.get("uri-port"), Some(&PredicateValue::Integer(8443)));
        assert_eq!(
            props.get("uri-path"),
            Some(&PredicateValue::Text("/v1/items".to_string()))
        );
        assert_eq!(
            props.get("status-code"),
            Some(&PredicateValue::Integer(503))
        );
        assert_eq!(
            props.get("error-type"),
            Some(&PredicateValue::Text("transient".to_string()))
        );
    }

    #[test]
    fn predicate_and_policy_roundtrip_through_wit_flattening() {
        let predicate = Predicate::And(
            Box::new(Predicate::PropEq {
                property: "verb".to_string(),
                value: PredicateValue::Text("invoke".to_string()),
            }),
            Box::new(Predicate::Not(Box::new(Predicate::PropExists(
                "status-code".to_string(),
            )))),
        );

        let policy = RetryPolicy::Union(
            Box::new(RetryPolicy::CountBox {
                max_retries: 3,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(10))),
            }),
            Box::new(RetryPolicy::FilteredOn {
                predicate: predicate.clone(),
                inner: Box::new(RetryPolicy::Immediate),
            }),
        );

        let predicate_roundtrip = Predicate::from_value(predicate.clone().into_value())
            .expect("predicate should roundtrip through value conversion");
        assert_eq!(predicate_roundtrip, predicate);

        let policy_roundtrip = RetryPolicy::from_value(policy.clone().into_value())
            .expect("policy should roundtrip through value conversion");
        assert_eq!(policy_roundtrip, policy);
    }

    fn assert_predicate_roundtrip(predicate: Predicate) {
        let roundtrip = Predicate::from_value(predicate.clone().into_value())
            .expect("predicate should roundtrip through value conversion");
        assert_eq!(roundtrip, predicate);
    }

    fn assert_policy_roundtrip(policy: RetryPolicy) {
        let roundtrip = RetryPolicy::from_value(policy.clone().into_value())
            .expect("policy should roundtrip through value conversion");
        assert_eq!(roundtrip, policy);
    }

    fn retry_wit_resolver() -> golem_wasm::analysis::wit_parser::AnalysedTypeResolve {
        let wit_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("wit");
        golem_wasm::analysis::wit_parser::AnalysedTypeResolve::from_wit_directory(&wit_dir)
            .expect("Failed to parse WIT directory")
    }

    fn assert_type_matches_wit(
        rust_type: AnalysedType,
        interface: &str,
        type_name: &str,
        resolver: &mut golem_wasm::analysis::wit_parser::AnalysedTypeResolve,
    ) {
        use golem_wasm::analysis::wit_parser::{TypeName, TypeOwner};

        let wit_type = resolver
            .analysed_type(&TypeName {
                package: Some("golem:api@1.5.0".to_string()),
                owner: TypeOwner::Interface(interface.to_string()),
                name: Some(type_name.to_string()),
            })
            .unwrap_or_else(|e| panic!("Failed to find {type_name} in WIT: {e}"));

        assert_eq!(
            rust_type, wit_type,
            "get_type() for {type_name} does not match the WIT definition"
        );
    }

    #[test]
    fn predicate_value_type_matches_wit() {
        let mut resolver = retry_wit_resolver();
        assert_type_matches_wit(PredicateValue::get_type(), "retry", "predicate-value", &mut resolver);
    }

    #[test]
    fn predicate_type_matches_wit() {
        let mut resolver = retry_wit_resolver();
        assert_type_matches_wit(Predicate::get_type(), "retry", "retry-predicate", &mut resolver);
    }

    #[test]
    fn retry_policy_type_matches_wit() {
        let mut resolver = retry_wit_resolver();
        assert_type_matches_wit(RetryPolicy::get_type(), "retry", "retry-policy", &mut resolver);
    }

    #[test]
    fn named_retry_policy_type_matches_wit() {
        let mut resolver = retry_wit_resolver();
        assert_type_matches_wit(NamedRetryPolicy::get_type(), "retry", "named-retry-policy", &mut resolver);
    }

    #[test]
    fn predicate_value_text_roundtrip() {
        let value = PredicateValue::Text("hello".to_string());
        let roundtrip = PredicateValue::from_value(value.clone().into_value())
            .expect("predicate value should roundtrip");
        assert_eq!(roundtrip, value);
    }

    #[test]
    fn predicate_value_integer_roundtrip() {
        let value = PredicateValue::Integer(42);
        let roundtrip = PredicateValue::from_value(value.clone().into_value())
            .expect("predicate value should roundtrip");
        assert_eq!(roundtrip, value);
    }

    #[test]
    fn predicate_value_boolean_roundtrip() {
        let value = PredicateValue::Boolean(true);
        let roundtrip = PredicateValue::from_value(value.clone().into_value())
            .expect("predicate value should roundtrip");
        assert_eq!(roundtrip, value);
    }

    #[test]
    fn predicate_prop_eq_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropEq {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(503),
        });
    }

    #[test]
    fn predicate_prop_neq_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropNeq {
            property: "verb".to_string(),
            value: PredicateValue::Text("DELETE".to_string()),
        });
    }

    #[test]
    fn predicate_prop_gt_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropGt {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(499),
        });
    }

    #[test]
    fn predicate_prop_gte_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropGte {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(500),
        });
    }

    #[test]
    fn predicate_prop_lt_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropLt {
            property: "attempt".to_string(),
            value: PredicateValue::Integer(10),
        });
    }

    #[test]
    fn predicate_prop_lte_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropLte {
            property: "attempt".to_string(),
            value: PredicateValue::Integer(5),
        });
    }

    #[test]
    fn predicate_prop_exists_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropExists("error-type".to_string()));
    }

    #[test]
    fn predicate_prop_in_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropIn {
            property: "status-code".to_string(),
            values: vec![
                PredicateValue::Integer(502),
                PredicateValue::Integer(503),
                PredicateValue::Integer(504),
            ],
        });
    }

    #[test]
    fn predicate_prop_matches_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropMatches {
            property: "uri-path".to_string(),
            pattern: "^/api/.*".to_string(),
        });
    }

    #[test]
    fn predicate_prop_starts_with_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropStartsWith {
            property: "uri-path".to_string(),
            prefix: "/api/".to_string(),
        });
    }

    #[test]
    fn predicate_prop_contains_roundtrip() {
        assert_predicate_roundtrip(Predicate::PropContains {
            property: "error-type".to_string(),
            substring: "timeout".to_string(),
        });
    }

    #[test]
    fn predicate_and_roundtrip() {
        assert_predicate_roundtrip(Predicate::And(
            Box::new(Predicate::PropExists("verb".to_string())),
            Box::new(Predicate::PropGt {
                property: "status-code".to_string(),
                value: PredicateValue::Integer(499),
            }),
        ));
    }

    #[test]
    fn predicate_or_roundtrip() {
        assert_predicate_roundtrip(Predicate::Or(
            Box::new(Predicate::PropEq {
                property: "status-code".to_string(),
                value: PredicateValue::Integer(429),
            }),
            Box::new(Predicate::PropGte {
                property: "status-code".to_string(),
                value: PredicateValue::Integer(500),
            }),
        ));
    }

    #[test]
    fn predicate_not_roundtrip() {
        assert_predicate_roundtrip(Predicate::Not(Box::new(Predicate::PropEq {
            property: "verb".to_string(),
            value: PredicateValue::Text("GET".to_string()),
        })));
    }

    #[test]
    fn predicate_true_roundtrip() {
        assert_predicate_roundtrip(Predicate::True);
    }

    #[test]
    fn predicate_false_roundtrip() {
        assert_predicate_roundtrip(Predicate::False);
    }

    #[test]
    fn predicate_deeply_nested_roundtrip() {
        assert_predicate_roundtrip(Predicate::And(
            Box::new(Predicate::Or(
                Box::new(Predicate::Not(Box::new(Predicate::PropExists(
                    "a".to_string(),
                )))),
                Box::new(Predicate::PropIn {
                    property: "b".to_string(),
                    values: vec![
                        PredicateValue::Text("x".to_string()),
                        PredicateValue::Integer(1),
                    ],
                }),
            )),
            Box::new(Predicate::And(
                Box::new(Predicate::PropMatches {
                    property: "c".to_string(),
                    pattern: ".*".to_string(),
                }),
                Box::new(Predicate::True),
            )),
        ));
    }

    #[test]
    fn policy_periodic_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Periodic(Duration::from_millis(500)));
    }

    #[test]
    fn policy_exponential_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Exponential {
            base_delay: Duration::from_millis(100),
            factor: 2.5,
        });
    }

    #[test]
    fn policy_fibonacci_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Fibonacci {
            first: Duration::from_millis(100),
            second: Duration::from_millis(200),
        });
    }

    #[test]
    fn policy_immediate_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Immediate);
    }

    #[test]
    fn policy_never_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Never);
    }

    #[test]
    fn policy_count_box_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::CountBox {
            max_retries: 5,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(100))),
        });
    }

    #[test]
    fn policy_time_box_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::TimeBox {
            limit: Duration::from_secs(60),
            inner: Box::new(RetryPolicy::Exponential {
                base_delay: Duration::from_millis(50),
                factor: 2.0,
            }),
        });
    }

    #[test]
    fn policy_clamp_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Clamp {
            min_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(5),
            inner: Box::new(RetryPolicy::Exponential {
                base_delay: Duration::from_millis(100),
                factor: 3.0,
            }),
        });
    }

    #[test]
    fn policy_add_delay_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::AddDelay {
            delay: Duration::from_millis(250),
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(100))),
        });
    }

    #[test]
    fn policy_jitter_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Jitter {
            factor: 0.25,
            inner: Box::new(RetryPolicy::Periodic(Duration::from_secs(1))),
        });
    }

    #[test]
    fn policy_filtered_on_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::FilteredOn {
            predicate: Predicate::PropEq {
                property: "error-type".to_string(),
                value: PredicateValue::Text("transient".to_string()),
            },
            inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(200))),
        });
    }

    #[test]
    fn policy_and_then_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::AndThen(
            Box::new(RetryPolicy::CountBox {
                max_retries: 3,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(100))),
            }),
            Box::new(RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Exponential {
                    base_delay: Duration::from_millis(500),
                    factor: 2.0,
                }),
            }),
        ));
    }

    #[test]
    fn policy_union_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Union(
            Box::new(RetryPolicy::Periodic(Duration::from_millis(100))),
            Box::new(RetryPolicy::Immediate),
        ));
    }

    #[test]
    fn policy_intersect_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Intersect(
            Box::new(RetryPolicy::CountBox {
                max_retries: 10,
                inner: Box::new(RetryPolicy::Immediate),
            }),
            Box::new(RetryPolicy::TimeBox {
                limit: Duration::from_secs(30),
                inner: Box::new(RetryPolicy::Immediate),
            }),
        ));
    }

    #[test]
    fn policy_deeply_nested_roundtrip() {
        assert_policy_roundtrip(RetryPolicy::Union(
            Box::new(RetryPolicy::Intersect(
                Box::new(RetryPolicy::CountBox {
                    max_retries: 3,
                    inner: Box::new(RetryPolicy::Jitter {
                        factor: 0.1,
                        inner: Box::new(RetryPolicy::Clamp {
                            min_delay: Duration::from_millis(10),
                            max_delay: Duration::from_secs(5),
                            inner: Box::new(RetryPolicy::Exponential {
                                base_delay: Duration::from_millis(50),
                                factor: 2.0,
                            }),
                        }),
                    }),
                }),
                Box::new(RetryPolicy::TimeBox {
                    limit: Duration::from_secs(120),
                    inner: Box::new(RetryPolicy::Immediate),
                }),
            )),
            Box::new(RetryPolicy::FilteredOn {
                predicate: Predicate::And(
                    Box::new(Predicate::PropExists("error-type".to_string())),
                    Box::new(Predicate::Not(Box::new(Predicate::PropEq {
                        property: "error-type".to_string(),
                        value: PredicateValue::Text("permanent".to_string()),
                    }))),
                ),
                inner: Box::new(RetryPolicy::AndThen(
                    Box::new(RetryPolicy::CountBox {
                        max_retries: 2,
                        inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(100))),
                    }),
                    Box::new(RetryPolicy::AddDelay {
                        delay: Duration::from_secs(1),
                        inner: Box::new(RetryPolicy::Fibonacci {
                            first: Duration::from_millis(100),
                            second: Duration::from_millis(200),
                        }),
                    }),
                )),
            }),
        ));
    }

    #[test]
    fn named_retry_policy_roundtrip_through_wit_flattening() {
        let named = NamedRetryPolicy {
            name: "rpc-transient".to_string(),
            priority: 42,
            predicate: Predicate::And(
                Box::new(Predicate::PropEq {
                    property: "error-type".to_string(),
                    value: PredicateValue::Text("transient".to_string()),
                }),
                Box::new(Predicate::PropGte {
                    property: "status-code".to_string(),
                    value: PredicateValue::Integer(500),
                }),
            ),
            policy: RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Clamp {
                    min_delay: Duration::from_millis(100),
                    max_delay: Duration::from_secs(10),
                    inner: Box::new(RetryPolicy::Exponential {
                        base_delay: Duration::from_millis(200),
                        factor: 2.0,
                    }),
                }),
            },
        };

        let roundtrip = NamedRetryPolicy::from_value(named.clone().into_value())
            .expect("named retry policy should roundtrip through value conversion");
        assert_eq!(roundtrip, named);
    }

    #[test]
    fn retry_policy_state_counter_roundtrip() {
        let state = RetryPolicyState::Counter(7);
        let roundtrip = RetryPolicyState::from_value(state.clone().into_value())
            .expect("retry policy state should roundtrip");
        assert_eq!(roundtrip, state);
    }

    #[test]
    fn retry_policy_state_terminal_roundtrip() {
        let state = RetryPolicyState::Terminal;
        let roundtrip = RetryPolicyState::from_value(state.clone().into_value())
            .expect("retry policy state should roundtrip");
        assert_eq!(roundtrip, state);
    }

    #[test]
    fn retry_policy_state_wrapper_roundtrip() {
        let state = RetryPolicyState::Wrapper(Box::new(RetryPolicyState::Counter(3)));
        let roundtrip = RetryPolicyState::from_value(state.clone().into_value())
            .expect("retry policy state should roundtrip");
        assert_eq!(roundtrip, state);
    }

    #[test]
    fn retry_policy_state_count_box_roundtrip() {
        let state = RetryPolicyState::CountBox {
            attempts: 5,
            inner: Box::new(RetryPolicyState::Counter(5)),
        };
        let roundtrip = RetryPolicyState::from_value(state.clone().into_value())
            .expect("retry policy state should roundtrip");
        assert_eq!(roundtrip, state);
    }

    #[test]
    fn retry_policy_state_and_then_roundtrip() {
        let state = RetryPolicyState::AndThen {
            left: Box::new(RetryPolicyState::Counter(3)),
            right: Box::new(RetryPolicyState::Counter(0)),
            on_right: false,
        };
        let roundtrip = RetryPolicyState::from_value(state.clone().into_value())
            .expect("retry policy state should roundtrip");
        assert_eq!(roundtrip, state);
    }

    #[test]
    fn retry_policy_state_pair_roundtrip() {
        let state = RetryPolicyState::Pair(
            Box::new(RetryPolicyState::Counter(1)),
            Box::new(RetryPolicyState::Terminal),
        );
        let roundtrip = RetryPolicyState::from_value(state.clone().into_value())
            .expect("retry policy state should roundtrip");
        assert_eq!(roundtrip, state);
    }

    #[test]
    fn retry_policy_state_deeply_nested_roundtrip() {
        let state = RetryPolicyState::Pair(
            Box::new(RetryPolicyState::CountBox {
                attempts: 2,
                inner: Box::new(RetryPolicyState::Wrapper(Box::new(
                    RetryPolicyState::Counter(2),
                ))),
            }),
            Box::new(RetryPolicyState::AndThen {
                left: Box::new(RetryPolicyState::Terminal),
                right: Box::new(RetryPolicyState::Wrapper(Box::new(
                    RetryPolicyState::Counter(4),
                ))),
                on_right: true,
            }),
        );
        let roundtrip = RetryPolicyState::from_value(state.clone().into_value())
            .expect("retry policy state should roundtrip");
        assert_eq!(roundtrip, state);
    }

    #[cfg(feature = "full")]
    #[test]
    fn predicate_and_policy_roundtrip_through_protobuf() {
        let predicate = Predicate::And(
            Box::new(Predicate::PropEq {
                property: "verb".to_string(),
                value: PredicateValue::Text("invoke".to_string()),
            }),
            Box::new(Predicate::Not(Box::new(Predicate::PropExists(
                "status-code".to_string(),
            )))),
        );

        let policy = RetryPolicy::Union(
            Box::new(RetryPolicy::CountBox {
                max_retries: 3,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(10))),
            }),
            Box::new(RetryPolicy::FilteredOn {
                predicate: predicate.clone(),
                inner: Box::new(RetryPolicy::Immediate),
            }),
        );

        let predicate_proto: golem_api_grpc::proto::golem::worker::retry::Predicate =
            predicate.clone().into();
        let predicate_roundtrip = Predicate::try_from(predicate_proto)
            .expect("predicate should roundtrip through protobuf conversion");
        assert_eq!(predicate_roundtrip, predicate);

        let policy_proto: golem_api_grpc::proto::golem::worker::retry::RetryPolicy =
            policy.clone().into();
        let policy_roundtrip = RetryPolicy::try_from(policy_proto)
            .expect("policy should roundtrip through protobuf conversion");
        assert_eq!(policy_roundtrip, policy);
    }

    #[cfg(feature = "full")]
    #[test]
    fn named_retry_policy_roundtrip_through_protobuf() {
        let named_policy = NamedRetryPolicy {
            name: "rpc-transient".to_string(),
            priority: 42,
            predicate: Predicate::PropEq {
                property: "error-type".to_string(),
                value: PredicateValue::Text("transient".to_string()),
            },
            policy: RetryPolicy::CountBox {
                max_retries: 5,
                inner: Box::new(RetryPolicy::Periodic(Duration::from_millis(200))),
            },
        };

        let proto: golem_api_grpc::proto::golem::worker::retry::NamedRetryPolicy =
            named_policy.clone().into();
        let roundtrip = NamedRetryPolicy::try_from(proto)
            .expect("named retry policy should roundtrip through protobuf conversion");
        assert_eq!(roundtrip, named_policy);
    }
}
