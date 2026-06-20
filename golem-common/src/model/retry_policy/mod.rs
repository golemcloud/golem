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

mod wit;

use crate::model::agent::parse_agent_id_parts;
use crate::model::{OwnedAgentId, RdbmsPoolKey, RetryConfig};
use desert_rust::BinaryCodec;
use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Range;
use std::time::Duration;

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

impl PredicateValue {
    fn to_json_value(&self) -> JsonValue {
        match self {
            PredicateValue::Text(value) => JsonValue::String(value.clone()),
            PredicateValue::Integer(value) => JsonValue::Number((*value).into()),
            PredicateValue::Boolean(value) => JsonValue::Bool(*value),
        }
    }

    fn from_json_value(value: JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::String(value) => Ok(Self::Text(value)),
            JsonValue::Bool(value) => Ok(Self::Boolean(value)),
            JsonValue::Number(value) => value
                .as_i64()
                .map(Self::Integer)
                .ok_or_else(|| "Predicate integer value must fit i64".to_string()),
            JsonValue::Object(_) => {
                let (kind, payload) = take_single_key_object(value)?;
                match kind.as_str() {
                    "text" => match payload {
                        JsonValue::String(value) => Ok(Self::Text(value)),
                        _ => Err("text value must be a string".to_string()),
                    },
                    "integer" => match payload {
                        JsonValue::Number(value) => value
                            .as_i64()
                            .map(Self::Integer)
                            .ok_or_else(|| "integer value must fit i64".to_string()),
                        _ => Err("integer value must be a number".to_string()),
                    },
                    "boolean" => match payload {
                        JsonValue::Bool(value) => Ok(Self::Boolean(value)),
                        _ => Err("boolean value must be a boolean".to_string()),
                    },
                    _ => Err(format!("Unsupported predicate value variant '{kind}'")),
                }
            }
            _ => Err(
                "Predicate value must be a string, number, boolean, or typed object".to_string(),
            ),
        }
    }
}

impl Serialize for PredicateValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_json_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PredicateValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = JsonValue::deserialize(deserializer)?;
        Self::from_json_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PredicateComparisonPayload {
    property: String,
    value: PredicateValue,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PredicateSetPayload {
    property: String,
    values: Vec<PredicateValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PredicatePatternPayload {
    property: String,
    pattern: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PredicatePrefixPayload {
    property: String,
    prefix: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PredicateSubstringPayload {
    property: String,
    substring: String,
}

impl Predicate {
    fn to_json_value(&self) -> JsonValue {
        match self {
            Predicate::True => JsonValue::Bool(true),
            Predicate::False => JsonValue::Bool(false),
            Predicate::PropEq { property, value } => single_key_object(
                "propEq",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("value".to_string(), value.to_json_value()),
                ])),
            ),
            Predicate::PropNeq { property, value } => single_key_object(
                "propNeq",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("value".to_string(), value.to_json_value()),
                ])),
            ),
            Predicate::PropGt { property, value } => single_key_object(
                "propGt",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("value".to_string(), value.to_json_value()),
                ])),
            ),
            Predicate::PropGte { property, value } => single_key_object(
                "propGte",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("value".to_string(), value.to_json_value()),
                ])),
            ),
            Predicate::PropLt { property, value } => single_key_object(
                "propLt",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("value".to_string(), value.to_json_value()),
                ])),
            ),
            Predicate::PropLte { property, value } => single_key_object(
                "propLte",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("value".to_string(), value.to_json_value()),
                ])),
            ),
            Predicate::PropExists(property) => {
                single_key_object("propExists", JsonValue::String(property.clone()))
            }
            Predicate::PropIn { property, values } => single_key_object(
                "propIn",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    (
                        "values".to_string(),
                        JsonValue::Array(
                            values.iter().map(PredicateValue::to_json_value).collect(),
                        ),
                    ),
                ])),
            ),
            Predicate::PropMatches { property, pattern } => single_key_object(
                "propMatches",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("pattern".to_string(), JsonValue::String(pattern.clone())),
                ])),
            ),
            Predicate::PropStartsWith { property, prefix } => single_key_object(
                "propStartsWith",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    ("prefix".to_string(), JsonValue::String(prefix.clone())),
                ])),
            ),
            Predicate::PropContains {
                property,
                substring,
            } => single_key_object(
                "propContains",
                JsonValue::Object(JsonMap::from_iter([
                    ("property".to_string(), JsonValue::String(property.clone())),
                    (
                        "substring".to_string(),
                        JsonValue::String(substring.clone()),
                    ),
                ])),
            ),
            Predicate::And(left, right) => single_key_object(
                "and",
                JsonValue::Array(vec![left.to_json_value(), right.to_json_value()]),
            ),
            Predicate::Or(left, right) => single_key_object(
                "or",
                JsonValue::Array(vec![left.to_json_value(), right.to_json_value()]),
            ),
            Predicate::Not(predicate) => single_key_object("not", predicate.to_json_value()),
        }
    }

    fn from_json_value(value: JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::Bool(true) => Ok(Self::True),
            JsonValue::Bool(false) => Ok(Self::False),
            JsonValue::String(value) if value == "true" => Ok(Self::True),
            JsonValue::String(value) if value == "false" => Ok(Self::False),
            JsonValue::String(value) => Err(format!(
                "Unknown predicate unit variant '{value}', expected true or false"
            )),
            JsonValue::Object(_) => {
                let (kind, payload) = take_single_key_object(value)?;
                match kind.as_str() {
                    "propEq" => {
                        let payload: PredicateComparisonPayload =
                            serde_json::from_value(payload)
                                .map_err(|e| format!("Invalid propEq payload: {e}"))?;
                        Ok(Self::PropEq {
                            property: payload.property,
                            value: payload.value,
                        })
                    }
                    "propNeq" => {
                        let payload: PredicateComparisonPayload =
                            serde_json::from_value(payload)
                                .map_err(|e| format!("Invalid propNeq payload: {e}"))?;
                        Ok(Self::PropNeq {
                            property: payload.property,
                            value: payload.value,
                        })
                    }
                    "propGt" => {
                        let payload: PredicateComparisonPayload =
                            serde_json::from_value(payload)
                                .map_err(|e| format!("Invalid propGt payload: {e}"))?;
                        Ok(Self::PropGt {
                            property: payload.property,
                            value: payload.value,
                        })
                    }
                    "propGte" => {
                        let payload: PredicateComparisonPayload =
                            serde_json::from_value(payload)
                                .map_err(|e| format!("Invalid propGte payload: {e}"))?;
                        Ok(Self::PropGte {
                            property: payload.property,
                            value: payload.value,
                        })
                    }
                    "propLt" => {
                        let payload: PredicateComparisonPayload =
                            serde_json::from_value(payload)
                                .map_err(|e| format!("Invalid propLt payload: {e}"))?;
                        Ok(Self::PropLt {
                            property: payload.property,
                            value: payload.value,
                        })
                    }
                    "propLte" => {
                        let payload: PredicateComparisonPayload =
                            serde_json::from_value(payload)
                                .map_err(|e| format!("Invalid propLte payload: {e}"))?;
                        Ok(Self::PropLte {
                            property: payload.property,
                            value: payload.value,
                        })
                    }
                    "propExists" => match payload {
                        JsonValue::String(property) => Ok(Self::PropExists(property)),
                        JsonValue::Object(map) => {
                            let property = map
                                .get("property")
                                .and_then(JsonValue::as_str)
                                .ok_or_else(|| {
                                    "Invalid propExists payload: expected property string"
                                        .to_string()
                                })?;
                            Ok(Self::PropExists(property.to_string()))
                        }
                        _ => Err("Invalid propExists payload".to_string()),
                    },
                    "propIn" => {
                        let payload: PredicateSetPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid propIn payload: {e}"))?;
                        Ok(Self::PropIn {
                            property: payload.property,
                            values: payload.values,
                        })
                    }
                    "propMatches" => {
                        let payload: PredicatePatternPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid propMatches payload: {e}"))?;
                        Ok(Self::PropMatches {
                            property: payload.property,
                            pattern: payload.pattern,
                        })
                    }
                    "propStartsWith" => {
                        let payload: PredicatePrefixPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid propStartsWith payload: {e}"))?;
                        Ok(Self::PropStartsWith {
                            property: payload.property,
                            prefix: payload.prefix,
                        })
                    }
                    "propContains" => {
                        let payload: PredicateSubstringPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid propContains payload: {e}"))?;
                        Ok(Self::PropContains {
                            property: payload.property,
                            substring: payload.substring,
                        })
                    }
                    "and" => {
                        let (left, right) = take_pair_array(payload, "and")?;
                        let left = Self::from_json_value(left)?;
                        let right = Self::from_json_value(right)?;
                        Ok(Self::And(Box::new(left), Box::new(right)))
                    }
                    "or" => {
                        let (left, right) = take_pair_array(payload, "or")?;
                        let left = Self::from_json_value(left)?;
                        let right = Self::from_json_value(right)?;
                        Ok(Self::Or(Box::new(left), Box::new(right)))
                    }
                    "not" => Ok(Self::Not(Box::new(Self::from_json_value(payload)?))),
                    _ => Err(format!("Unsupported predicate variant '{kind}'")),
                }
            }
            _ => Err("Invalid predicate encoding".to_string()),
        }
    }
}

impl Serialize for Predicate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_json_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Predicate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = JsonValue::deserialize(deserializer)?;
        Self::from_json_value(value).map_err(serde::de::Error::custom)
    }
}

/// Deserializes a [`Duration`] from either a humantime string (e.g. `"200ms"`,
/// `"5s"`, `"1m"`) or the canonical Rust `{ "secs": <u64>, "nanos": <u32> }`
/// struct form. Both forms are accepted in JSON and YAML retry policy
/// payloads. The struct form is used when serializing.
fn deserialize_duration_humantime_or_struct<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DurationRepr {
        Humantime(String),
        // Use the default `Duration` representation here: `{ secs, nanos }`.
        Struct(Duration),
    }

    match DurationRepr::deserialize(deserializer)? {
        DurationRepr::Humantime(s) => humantime::parse_duration(&s)
            .map_err(|e| serde::de::Error::custom(format!("invalid duration string {s:?}: {e}"))),
        DurationRepr::Struct(d) => Ok(d),
    }
}

/// Same as [`deserialize_duration_humantime_or_struct`] but operates on an
/// already-extracted [`JsonValue`] payload, used by the `RetryPolicy::Periodic`
/// branch which deserializes the payload directly as a `Duration` rather than
/// through a struct field.
fn deserialize_duration_value_humantime_or_struct(value: JsonValue) -> Result<Duration, String> {
    match value {
        JsonValue::String(s) => {
            humantime::parse_duration(&s).map_err(|e| format!("invalid duration string {s:?}: {e}"))
        }
        other => serde_json::from_value::<Duration>(other).map_err(|e| e.to_string()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExponentialPayload {
    #[serde(
        alias = "base_delay",
        deserialize_with = "deserialize_duration_humantime_or_struct"
    )]
    base_delay: Duration,
    factor: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FibonacciPayload {
    #[serde(deserialize_with = "deserialize_duration_humantime_or_struct")]
    first: Duration,
    #[serde(deserialize_with = "deserialize_duration_humantime_or_struct")]
    second: Duration,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountBoxPayload {
    #[serde(alias = "max_retries")]
    max_retries: u32,
    inner: RetryPolicy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimeBoxPayload {
    #[serde(deserialize_with = "deserialize_duration_humantime_or_struct")]
    limit: Duration,
    inner: RetryPolicy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClampPayload {
    #[serde(
        alias = "min_delay",
        deserialize_with = "deserialize_duration_humantime_or_struct"
    )]
    min_delay: Duration,
    #[serde(
        alias = "max_delay",
        deserialize_with = "deserialize_duration_humantime_or_struct"
    )]
    max_delay: Duration,
    inner: RetryPolicy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddDelayPayload {
    #[serde(deserialize_with = "deserialize_duration_humantime_or_struct")]
    delay: Duration,
    inner: RetryPolicy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JitterPayload {
    factor: f64,
    inner: RetryPolicy,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilteredOnPayload {
    predicate: Predicate,
    inner: RetryPolicy,
}

impl RetryPolicy {
    fn to_json_value(&self) -> JsonValue {
        match self {
            RetryPolicy::Immediate => JsonValue::String("immediate".to_string()),
            RetryPolicy::Never => JsonValue::String("never".to_string()),
            RetryPolicy::Periodic(delay) => single_key_object(
                "periodic",
                serde_json::to_value(delay).expect("Duration serialization should not fail"),
            ),
            RetryPolicy::Exponential { base_delay, factor } => single_key_object(
                "exponential",
                JsonValue::Object(JsonMap::from_iter([
                    (
                        "baseDelay".to_string(),
                        serde_json::to_value(base_delay)
                            .expect("Duration serialization should not fail"),
                    ),
                    (
                        "factor".to_string(),
                        serde_json::to_value(factor).expect("f64 serialization should not fail"),
                    ),
                ])),
            ),
            RetryPolicy::Fibonacci { first, second } => single_key_object(
                "fibonacci",
                JsonValue::Object(JsonMap::from_iter([
                    (
                        "first".to_string(),
                        serde_json::to_value(first)
                            .expect("Duration serialization should not fail"),
                    ),
                    (
                        "second".to_string(),
                        serde_json::to_value(second)
                            .expect("Duration serialization should not fail"),
                    ),
                ])),
            ),
            RetryPolicy::CountBox { max_retries, inner } => single_key_object(
                "countBox",
                JsonValue::Object(JsonMap::from_iter([
                    (
                        "maxRetries".to_string(),
                        serde_json::to_value(max_retries)
                            .expect("u32 serialization should not fail"),
                    ),
                    ("inner".to_string(), inner.to_json_value()),
                ])),
            ),
            RetryPolicy::TimeBox { limit, inner } => single_key_object(
                "timeBox",
                JsonValue::Object(JsonMap::from_iter([
                    (
                        "limit".to_string(),
                        serde_json::to_value(limit)
                            .expect("Duration serialization should not fail"),
                    ),
                    ("inner".to_string(), inner.to_json_value()),
                ])),
            ),
            RetryPolicy::Clamp {
                min_delay,
                max_delay,
                inner,
            } => single_key_object(
                "clamp",
                JsonValue::Object(JsonMap::from_iter([
                    (
                        "minDelay".to_string(),
                        serde_json::to_value(min_delay)
                            .expect("Duration serialization should not fail"),
                    ),
                    (
                        "maxDelay".to_string(),
                        serde_json::to_value(max_delay)
                            .expect("Duration serialization should not fail"),
                    ),
                    ("inner".to_string(), inner.to_json_value()),
                ])),
            ),
            RetryPolicy::AddDelay { delay, inner } => single_key_object(
                "addDelay",
                JsonValue::Object(JsonMap::from_iter([
                    (
                        "delay".to_string(),
                        serde_json::to_value(delay)
                            .expect("Duration serialization should not fail"),
                    ),
                    ("inner".to_string(), inner.to_json_value()),
                ])),
            ),
            RetryPolicy::Jitter { factor, inner } => single_key_object(
                "jitter",
                JsonValue::Object(JsonMap::from_iter([
                    (
                        "factor".to_string(),
                        serde_json::to_value(factor).expect("f64 serialization should not fail"),
                    ),
                    ("inner".to_string(), inner.to_json_value()),
                ])),
            ),
            RetryPolicy::FilteredOn { predicate, inner } => single_key_object(
                "filteredOn",
                JsonValue::Object(JsonMap::from_iter([
                    ("predicate".to_string(), predicate.to_json_value()),
                    ("inner".to_string(), inner.to_json_value()),
                ])),
            ),
            RetryPolicy::AndThen(left, right) => single_key_object(
                "andThen",
                JsonValue::Array(vec![left.to_json_value(), right.to_json_value()]),
            ),
            RetryPolicy::Union(left, right) => single_key_object(
                "union",
                JsonValue::Array(vec![left.to_json_value(), right.to_json_value()]),
            ),
            RetryPolicy::Intersect(left, right) => single_key_object(
                "intersect",
                JsonValue::Array(vec![left.to_json_value(), right.to_json_value()]),
            ),
        }
    }

    fn from_json_value(value: JsonValue) -> Result<Self, String> {
        match value {
            JsonValue::String(value) if value == "immediate" => Ok(Self::Immediate),
            JsonValue::String(value) if value == "never" => Ok(Self::Never),
            JsonValue::String(value) => Err(format!(
                "Unknown retry policy unit variant '{value}', expected immediate or never"
            )),
            JsonValue::Object(_) => {
                let (kind, payload) = take_single_key_object(value)?;
                match kind.as_str() {
                    "periodic" => Ok(Self::Periodic(
                        deserialize_duration_value_humantime_or_struct(payload)
                            .map_err(|e| format!("Invalid periodic payload: {e}"))?,
                    )),
                    "exponential" => {
                        let payload: ExponentialPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid exponential payload: {e}"))?;
                        Ok(Self::Exponential {
                            base_delay: payload.base_delay,
                            factor: payload.factor,
                        })
                    }
                    "fibonacci" => {
                        let payload: FibonacciPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid fibonacci payload: {e}"))?;
                        Ok(Self::Fibonacci {
                            first: payload.first,
                            second: payload.second,
                        })
                    }
                    "countBox" => {
                        let payload: CountBoxPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid countBox payload: {e}"))?;
                        Ok(Self::CountBox {
                            max_retries: payload.max_retries,
                            inner: Box::new(payload.inner),
                        })
                    }
                    "timeBox" => {
                        let payload: TimeBoxPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid timeBox payload: {e}"))?;
                        Ok(Self::TimeBox {
                            limit: payload.limit,
                            inner: Box::new(payload.inner),
                        })
                    }
                    "clamp" => {
                        let payload: ClampPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid clamp payload: {e}"))?;
                        Ok(Self::Clamp {
                            min_delay: payload.min_delay,
                            max_delay: payload.max_delay,
                            inner: Box::new(payload.inner),
                        })
                    }
                    "addDelay" => {
                        let payload: AddDelayPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid addDelay payload: {e}"))?;
                        Ok(Self::AddDelay {
                            delay: payload.delay,
                            inner: Box::new(payload.inner),
                        })
                    }
                    "jitter" => {
                        let payload: JitterPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid jitter payload: {e}"))?;
                        Ok(Self::Jitter {
                            factor: payload.factor,
                            inner: Box::new(payload.inner),
                        })
                    }
                    "filteredOn" => {
                        let payload: FilteredOnPayload = serde_json::from_value(payload)
                            .map_err(|e| format!("Invalid filteredOn payload: {e}"))?;
                        Ok(Self::FilteredOn {
                            predicate: payload.predicate,
                            inner: Box::new(payload.inner),
                        })
                    }
                    "andThen" => {
                        let (left, right) = take_pair_array(payload, "andThen")?;
                        let left = Self::from_json_value(left)?;
                        let right = Self::from_json_value(right)?;
                        Ok(Self::AndThen(Box::new(left), Box::new(right)))
                    }
                    "union" => {
                        let (left, right) = take_pair_array(payload, "union")?;
                        let left = Self::from_json_value(left)?;
                        let right = Self::from_json_value(right)?;
                        Ok(Self::Union(Box::new(left), Box::new(right)))
                    }
                    "intersect" => {
                        let (left, right) = take_pair_array(payload, "intersect")?;
                        let left = Self::from_json_value(left)?;
                        let right = Self::from_json_value(right)?;
                        Ok(Self::Intersect(Box::new(left), Box::new(right)))
                    }
                    _ => Err(format!("Unsupported retry policy variant '{kind}'")),
                }
            }
            _ => Err("Invalid retry policy encoding".to_string()),
        }
    }
}

impl Serialize for RetryPolicy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.to_json_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RetryPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = JsonValue::deserialize(deserializer)?;
        Self::from_json_value(value).map_err(serde::de::Error::custom)
    }
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

    /// Wraps this state in an exhausted marker while preserving its retry count.
    pub fn exhausted(self) -> RetryPolicyState {
        if self.retry_count() == 0 {
            RetryPolicyState::Terminal
        } else {
            RetryPolicyState::AndThen {
                left: Box::new(self),
                right: Box::new(RetryPolicyState::Terminal),
                on_right: true,
            }
        }
    }

    /// Returns whether this state marks a retry policy that has already given up.
    pub fn is_exhausted(&self) -> bool {
        matches!(self, RetryPolicyState::Terminal)
            || matches!(
                self,
                RetryPolicyState::AndThen {
                    right,
                    on_right: true,
                    ..
                } if matches!(**right, RetryPolicyState::Terminal)
            )
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

    /// Evaluates this predicate while treating missing properties as a non-match.
    ///
    /// Unlike wrapping [`Predicate::matches`] and mapping a top-level
    /// [`RetryEvaluationError::PropertyNotFound`] to `false`, this is recursive so
    /// compound predicates can still match on branches whose properties are present.
    /// For example, `Or(missing_property, matching_property)` still matches, while
    /// `And(missing_property, matching_property)` does not. A missing property under
    /// `Not` remains a non-match, so `Not(status-code == 500)` does not accidentally
    /// select a status-code policy in contexts that do not have `status-code`.
    pub fn matches_treating_missing_properties_as_false(
        &self,
        props: &RetryProperties,
    ) -> Result<bool, RetryEvaluationError> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum LenientMatch {
            Matched,
            NotMatched,
            MissingProperty,
        }

        fn evaluate(
            predicate: &Predicate,
            props: &RetryProperties,
        ) -> Result<LenientMatch, RetryEvaluationError> {
            match predicate {
                Predicate::And(left, right) => {
                    match (evaluate(left, props)?, evaluate(right, props)?) {
                        (LenientMatch::Matched, LenientMatch::Matched) => Ok(LenientMatch::Matched),
                        (LenientMatch::MissingProperty, _) | (_, LenientMatch::MissingProperty) => {
                            Ok(LenientMatch::MissingProperty)
                        }
                        _ => Ok(LenientMatch::NotMatched),
                    }
                }
                Predicate::Or(left, right) => {
                    match (evaluate(left, props)?, evaluate(right, props)?) {
                        (LenientMatch::Matched, _) | (_, LenientMatch::Matched) => {
                            Ok(LenientMatch::Matched)
                        }
                        (LenientMatch::MissingProperty, _) | (_, LenientMatch::MissingProperty) => {
                            Ok(LenientMatch::MissingProperty)
                        }
                        _ => Ok(LenientMatch::NotMatched),
                    }
                }
                Predicate::Not(inner) => match evaluate(inner, props)? {
                    LenientMatch::Matched => Ok(LenientMatch::NotMatched),
                    LenientMatch::NotMatched => Ok(LenientMatch::Matched),
                    LenientMatch::MissingProperty => Ok(LenientMatch::MissingProperty),
                },
                other => match other.matches(props) {
                    Ok(true) => Ok(LenientMatch::Matched),
                    Ok(false) => Ok(LenientMatch::NotMatched),
                    Err(RetryEvaluationError::PropertyNotFound { .. }) => {
                        Ok(LenientMatch::MissingProperty)
                    }
                    Err(other) => Err(other),
                },
            }
        }

        Ok(evaluate(self, props)? == LenientMatch::Matched)
    }

    /// Returns `true` if this predicate explicitly references the given property name
    /// anywhere in its tree.
    ///
    /// This is used by retry decision sites whose entire behavior depends on a policy
    /// having opted into a specific property (e.g. HTTP status-code retries should only
    /// consider policies whose predicate explicitly references `status-code`). Catch-all
    /// predicates such as [`Predicate::True`] return `false` because they do not
    /// reference any property.
    pub fn references_property(&self, name: &str) -> bool {
        match self {
            Predicate::PropEq { property, .. }
            | Predicate::PropNeq { property, .. }
            | Predicate::PropGt { property, .. }
            | Predicate::PropGte { property, .. }
            | Predicate::PropLt { property, .. }
            | Predicate::PropLte { property, .. }
            | Predicate::PropExists(property)
            | Predicate::PropIn { property, .. }
            | Predicate::PropMatches { property, .. }
            | Predicate::PropStartsWith { property, .. }
            | Predicate::PropContains { property, .. } => property == name,
            Predicate::And(left, right) | Predicate::Or(left, right) => {
                left.references_property(name) || right.references_property(name)
            }
            Predicate::Not(inner) => inner.references_property(name),
            Predicate::True | Predicate::False => false,
        }
    }
}

impl RetryPolicy {
    /// Returns `true` if any nested predicate inside this policy explicitly references
    /// the given property name.
    ///
    /// Mirrors [`Predicate::references_property`] but recurses through retry-policy
    /// combinators that carry predicates (currently [`RetryPolicy::FilteredOn`]) and
    /// through structural wrappers that hold an `inner` policy. This lets retry-decision
    /// sites that are gated on explicit property opt-in (e.g. HTTP status-code retries)
    /// also recognize policies whose status-code logic is encoded inside `FilteredOn`
    /// rather than in the [`NamedRetryPolicy`] selector predicate.
    pub fn references_property(&self, name: &str) -> bool {
        match self {
            RetryPolicy::Periodic(_)
            | RetryPolicy::Exponential { .. }
            | RetryPolicy::Fibonacci { .. }
            | RetryPolicy::Immediate
            | RetryPolicy::Never => false,
            RetryPolicy::CountBox { inner, .. }
            | RetryPolicy::TimeBox { inner, .. }
            | RetryPolicy::Clamp { inner, .. }
            | RetryPolicy::AddDelay { inner, .. }
            | RetryPolicy::Jitter { inner, .. } => inner.references_property(name),
            RetryPolicy::FilteredOn { predicate, inner } => {
                predicate.references_property(name) || inner.references_property(name)
            }
            RetryPolicy::AndThen(left, right)
            | RetryPolicy::Union(left, right)
            | RetryPolicy::Intersect(left, right) => {
                left.references_property(name) || right.references_property(name)
            }
        }
    }

    /// Returns whether this policy body is applicable in the given retry context,
    /// treating missing properties in [`RetryPolicy::FilteredOn`] predicates as a
    /// non-match.
    ///
    /// This is intentionally separate from [`RetryPolicy::step`], which remains
    /// strict and reports missing properties as evaluation errors. It is used by
    /// retry-policy selection sites that need to skip policies that cannot apply
    /// to the current context before committing to a single named policy.
    pub fn applicable_treating_missing_properties_as_no_match(
        &self,
        properties: &RetryProperties,
    ) -> Result<bool, RetryEvaluationError> {
        match self {
            RetryPolicy::Periodic(_)
            | RetryPolicy::Exponential { .. }
            | RetryPolicy::Fibonacci { .. }
            | RetryPolicy::Immediate
            | RetryPolicy::Never => Ok(true),
            RetryPolicy::CountBox { inner, .. } => {
                inner.applicable_treating_missing_properties_as_no_match(properties)
            }
            RetryPolicy::TimeBox { inner, .. }
            | RetryPolicy::Clamp { inner, .. }
            | RetryPolicy::AddDelay { inner, .. }
            | RetryPolicy::Jitter { inner, .. } => {
                inner.applicable_treating_missing_properties_as_no_match(properties)
            }
            RetryPolicy::FilteredOn { predicate, inner } => {
                if predicate.matches_treating_missing_properties_as_false(properties)? {
                    inner.applicable_treating_missing_properties_as_no_match(properties)
                } else {
                    Ok(false)
                }
            }
            RetryPolicy::AndThen(left, right) | RetryPolicy::Union(left, right) => Ok(left
                .applicable_treating_missing_properties_as_no_match(properties)?
                || right.applicable_treating_missing_properties_as_no_match(properties)?),
            RetryPolicy::Intersect(left, right) => Ok(left
                .applicable_treating_missing_properties_as_no_match(properties)?
                && right.applicable_treating_missing_properties_as_no_match(properties)?),
        }
    }

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
                RetryPolicy::CountBox { max_retries, inner },
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
            (RetryPolicy::TimeBox { limit, inner }, RetryPolicyState::Wrapper(inner_state)) => {
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
            (RetryPolicy::AddDelay { delay, inner }, RetryPolicyState::Wrapper(inner_state)) => {
                let (new_inner, verdict) = inner.step(inner_state, elapsed, properties, rng);
                let verdict = match verdict {
                    RetryVerdict::Retry(current) => {
                        RetryVerdict::Retry(saturating_add_duration(current, *delay))
                    }
                    other => other,
                };
                (RetryPolicyState::Wrapper(Box::new(new_inner)), verdict)
            }
            (RetryPolicy::Jitter { factor, inner }, RetryPolicyState::Wrapper(inner_state)) => {
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
                    let (new_left, left_verdict) = left_policy.step(left, elapsed, properties, rng);
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
        ordered.sort_by_key(|p| std::cmp::Reverse(p.priority));

        for policy in ordered {
            if policy.predicate.matches(properties)? {
                return Ok(Some(policy));
            }
        }

        Ok(None)
    }

    /// Like [`resolve`], but treats [`RetryEvaluationError::PropertyNotFound`] as a
    /// non-match instead of a hard error.
    ///
    /// This is intended for retry decision sites whose context only exposes a subset of
    /// retry properties (for example, the WASM trap path does not populate
    /// `status-code`). A user policy keyed on a property that does not exist in the
    /// current context should be silently skipped — it cannot apply here by definition
    /// — rather than producing a confusing "property not found" error in logs.
    ///
    /// Other evaluation errors (such as type-coercion failures) are still propagated.
    ///
    /// [`resolve`]: NamedRetryPolicy::resolve
    pub fn resolve_treating_missing_properties_as_no_match<'a>(
        policies: &'a [NamedRetryPolicy],
        properties: &RetryProperties,
    ) -> Result<Option<&'a NamedRetryPolicy>, RetryEvaluationError> {
        let mut ordered = policies.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|p| std::cmp::Reverse(p.priority));

        for policy in ordered {
            match policy.predicate.matches(properties) {
                Ok(true) => return Ok(Some(policy)),
                Ok(false) => continue,
                Err(RetryEvaluationError::PropertyNotFound { .. }) => continue,
                Err(other) => return Err(other),
            }
        }

        Ok(None)
    }

    /// Like [`resolve_treating_missing_properties_as_no_match`], but also
    /// considers [`RetryPolicy::FilteredOn`] predicates in the policy body as
    /// applicability filters.
    ///
    /// This is intended for retry decision sites where a single selected named
    /// policy is evaluated afterwards, but user policies may encode contextual
    /// conditions either in the named selector predicate or in `FilteredOn`
    /// wrappers inside the policy body.
    pub fn resolve_applicable_treating_missing_properties_as_no_match<'a>(
        policies: &'a [NamedRetryPolicy],
        properties: &RetryProperties,
    ) -> Result<Option<&'a NamedRetryPolicy>, RetryEvaluationError> {
        let mut ordered = policies.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|p| std::cmp::Reverse(p.priority));

        for policy in ordered {
            if policy
                .predicate
                .matches_treating_missing_properties_as_false(properties)?
                && policy
                    .policy
                    .applicable_treating_missing_properties_as_no_match(properties)?
            {
                return Ok(Some(policy));
            }
        }

        Ok(None)
    }

    /// Creates a default catch-all retry policy from a legacy `RetryConfig`.
    ///
    /// This policy uses `Predicate::True` (matches everything) with priority 0,
    /// so any user-defined policy with a higher priority will take precedence.
    pub fn default_from_config(config: &RetryConfig) -> Self {
        Self {
            name: "default".to_string(),
            priority: 0,
            predicate: Predicate::True,
            policy: RetryPolicy::from(config.clone()),
        }
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
        return Duration::MAX;
    }

    // `Duration::from_secs_f64` panics if the value overflows `Duration`'s representation,
    // and `Duration::MAX.as_secs_f64()` rounds up to `2^64` (just past `u64::MAX`), so
    // clamping with `value.min(Duration::MAX.as_secs_f64())` is not safe. Use the
    // non-panicking variant and saturate to `Duration::MAX` on overflow.
    Duration::try_from_secs_f64(value).unwrap_or(Duration::MAX)
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

pub use crate::base_model::retry_policy::{
    RetryPolicyCreation, RetryPolicyDto, RetryPolicyId, RetryPolicyRevision, RetryPolicyUpdate,
};

// --- From conversions: base_model Api* types <-> model types ---

impl From<PredicateValue> for crate::base_model::retry_policy::ApiPredicateValue {
    fn from(value: PredicateValue) -> Self {
        use crate::base_model::retry_policy::*;
        match value {
            PredicateValue::Text(v) => ApiPredicateValue::Text(ApiTextValue { value: v }),
            PredicateValue::Integer(v) => ApiPredicateValue::Integer(ApiIntegerValue { value: v }),
            PredicateValue::Boolean(v) => ApiPredicateValue::Boolean(ApiBooleanValue { value: v }),
        }
    }
}

impl From<crate::base_model::retry_policy::ApiPredicateValue> for PredicateValue {
    fn from(value: crate::base_model::retry_policy::ApiPredicateValue) -> Self {
        use crate::base_model::retry_policy::ApiPredicateValue;
        match value {
            ApiPredicateValue::Text(v) => PredicateValue::Text(v.value),
            ApiPredicateValue::Integer(v) => PredicateValue::Integer(v.value),
            ApiPredicateValue::Boolean(v) => PredicateValue::Boolean(v.value),
        }
    }
}

impl From<Predicate> for crate::base_model::retry_policy::ApiPredicate {
    fn from(value: Predicate) -> Self {
        use crate::base_model::retry_policy::*;
        match value {
            Predicate::PropEq { property, value } => ApiPredicate::PropEq(ApiPropertyComparison {
                property,
                value: value.into(),
            }),
            Predicate::PropNeq { property, value } => {
                ApiPredicate::PropNeq(ApiPropertyComparison {
                    property,
                    value: value.into(),
                })
            }
            Predicate::PropGt { property, value } => ApiPredicate::PropGt(ApiPropertyComparison {
                property,
                value: value.into(),
            }),
            Predicate::PropGte { property, value } => {
                ApiPredicate::PropGte(ApiPropertyComparison {
                    property,
                    value: value.into(),
                })
            }
            Predicate::PropLt { property, value } => ApiPredicate::PropLt(ApiPropertyComparison {
                property,
                value: value.into(),
            }),
            Predicate::PropLte { property, value } => {
                ApiPredicate::PropLte(ApiPropertyComparison {
                    property,
                    value: value.into(),
                })
            }
            Predicate::PropExists(property) => {
                ApiPredicate::PropExists(ApiPropertyExistence { property })
            }
            Predicate::PropIn { property, values } => ApiPredicate::PropIn(ApiPropertySetCheck {
                property,
                values: values.into_iter().map(|v| v.into()).collect(),
            }),
            Predicate::PropMatches { property, pattern } => {
                ApiPredicate::PropMatches(ApiPropertyPattern { property, pattern })
            }
            Predicate::PropStartsWith { property, prefix } => {
                ApiPredicate::PropStartsWith(ApiPropertyPrefix { property, prefix })
            }
            Predicate::PropContains {
                property,
                substring,
            } => ApiPredicate::PropContains(ApiPropertySubstring {
                property,
                substring,
            }),
            Predicate::And(left, right) => ApiPredicate::And(ApiPredicatePair {
                left: Box::new((*left).into()),
                right: Box::new((*right).into()),
            }),
            Predicate::Or(left, right) => ApiPredicate::Or(ApiPredicatePair {
                left: Box::new((*left).into()),
                right: Box::new((*right).into()),
            }),
            Predicate::Not(inner) => ApiPredicate::Not(ApiPredicateNot {
                predicate: Box::new((*inner).into()),
            }),
            Predicate::True => ApiPredicate::True(ApiPredicateTrue {}),
            Predicate::False => ApiPredicate::False(ApiPredicateFalse {}),
        }
    }
}

impl From<crate::base_model::retry_policy::ApiPredicate> for Predicate {
    fn from(value: crate::base_model::retry_policy::ApiPredicate) -> Self {
        use crate::base_model::retry_policy::ApiPredicate;
        match value {
            ApiPredicate::PropEq(v) => Predicate::PropEq {
                property: v.property,
                value: v.value.into(),
            },
            ApiPredicate::PropNeq(v) => Predicate::PropNeq {
                property: v.property,
                value: v.value.into(),
            },
            ApiPredicate::PropGt(v) => Predicate::PropGt {
                property: v.property,
                value: v.value.into(),
            },
            ApiPredicate::PropGte(v) => Predicate::PropGte {
                property: v.property,
                value: v.value.into(),
            },
            ApiPredicate::PropLt(v) => Predicate::PropLt {
                property: v.property,
                value: v.value.into(),
            },
            ApiPredicate::PropLte(v) => Predicate::PropLte {
                property: v.property,
                value: v.value.into(),
            },
            ApiPredicate::PropExists(v) => Predicate::PropExists(v.property),
            ApiPredicate::PropIn(v) => Predicate::PropIn {
                property: v.property,
                values: v.values.into_iter().map(|v| v.into()).collect(),
            },
            ApiPredicate::PropMatches(v) => Predicate::PropMatches {
                property: v.property,
                pattern: v.pattern,
            },
            ApiPredicate::PropStartsWith(v) => Predicate::PropStartsWith {
                property: v.property,
                prefix: v.prefix,
            },
            ApiPredicate::PropContains(v) => Predicate::PropContains {
                property: v.property,
                substring: v.substring,
            },
            ApiPredicate::And(v) => {
                Predicate::And(Box::new((*v.left).into()), Box::new((*v.right).into()))
            }
            ApiPredicate::Or(v) => {
                Predicate::Or(Box::new((*v.left).into()), Box::new((*v.right).into()))
            }
            ApiPredicate::Not(v) => Predicate::Not(Box::new((*v.predicate).into())),
            ApiPredicate::True(_) => Predicate::True,
            ApiPredicate::False(_) => Predicate::False,
        }
    }
}

impl From<RetryPolicy> for crate::base_model::retry_policy::ApiRetryPolicy {
    fn from(value: RetryPolicy) -> Self {
        use crate::base_model::retry_policy::*;
        match value {
            RetryPolicy::Periodic(delay) => ApiRetryPolicy::Periodic(ApiPeriodicPolicy {
                delay_ms: delay.as_millis() as u64,
            }),
            RetryPolicy::Exponential { base_delay, factor } => {
                ApiRetryPolicy::Exponential(ApiExponentialPolicy {
                    base_delay_ms: base_delay.as_millis() as u64,
                    factor,
                })
            }
            RetryPolicy::Fibonacci { first, second } => {
                ApiRetryPolicy::Fibonacci(ApiFibonacciPolicy {
                    first_ms: first.as_millis() as u64,
                    second_ms: second.as_millis() as u64,
                })
            }
            RetryPolicy::Immediate => ApiRetryPolicy::Immediate(ApiImmediatePolicy {}),
            RetryPolicy::Never => ApiRetryPolicy::Never(ApiNeverPolicy {}),
            RetryPolicy::CountBox { max_retries, inner } => {
                ApiRetryPolicy::CountBox(ApiCountBoxPolicy {
                    max_retries,
                    inner: Box::new((*inner).into()),
                })
            }
            RetryPolicy::TimeBox { limit, inner } => ApiRetryPolicy::TimeBox(ApiTimeBoxPolicy {
                limit_ms: limit.as_millis() as u64,
                inner: Box::new((*inner).into()),
            }),
            RetryPolicy::Clamp {
                min_delay,
                max_delay,
                inner,
            } => ApiRetryPolicy::Clamp(ApiClampPolicy {
                min_delay_ms: min_delay.as_millis() as u64,
                max_delay_ms: max_delay.as_millis() as u64,
                inner: Box::new((*inner).into()),
            }),
            RetryPolicy::AddDelay { delay, inner } => ApiRetryPolicy::AddDelay(ApiAddDelayPolicy {
                delay_ms: delay.as_millis() as u64,
                inner: Box::new((*inner).into()),
            }),
            RetryPolicy::Jitter { factor, inner } => ApiRetryPolicy::Jitter(ApiJitterPolicy {
                factor,
                inner: Box::new((*inner).into()),
            }),
            RetryPolicy::FilteredOn { predicate, inner } => {
                ApiRetryPolicy::FilteredOn(ApiFilteredOnPolicy {
                    predicate: predicate.into(),
                    inner: Box::new((*inner).into()),
                })
            }
            RetryPolicy::AndThen(first, second) => ApiRetryPolicy::AndThen(ApiRetryPolicyPair {
                first: Box::new((*first).into()),
                second: Box::new((*second).into()),
            }),
            RetryPolicy::Union(first, second) => ApiRetryPolicy::Union(ApiRetryPolicyPair {
                first: Box::new((*first).into()),
                second: Box::new((*second).into()),
            }),
            RetryPolicy::Intersect(first, second) => {
                ApiRetryPolicy::Intersect(ApiRetryPolicyPair {
                    first: Box::new((*first).into()),
                    second: Box::new((*second).into()),
                })
            }
        }
    }
}

impl From<crate::base_model::retry_policy::ApiRetryPolicy> for RetryPolicy {
    fn from(value: crate::base_model::retry_policy::ApiRetryPolicy) -> Self {
        use crate::base_model::retry_policy::ApiRetryPolicy;
        match value {
            ApiRetryPolicy::Periodic(v) => RetryPolicy::Periodic(Duration::from_millis(v.delay_ms)),
            ApiRetryPolicy::Exponential(v) => RetryPolicy::Exponential {
                base_delay: Duration::from_millis(v.base_delay_ms),
                factor: v.factor,
            },
            ApiRetryPolicy::Fibonacci(v) => RetryPolicy::Fibonacci {
                first: Duration::from_millis(v.first_ms),
                second: Duration::from_millis(v.second_ms),
            },
            ApiRetryPolicy::Immediate(_) => RetryPolicy::Immediate,
            ApiRetryPolicy::Never(_) => RetryPolicy::Never,
            ApiRetryPolicy::CountBox(v) => RetryPolicy::CountBox {
                max_retries: v.max_retries,
                inner: Box::new((*v.inner).into()),
            },
            ApiRetryPolicy::TimeBox(v) => RetryPolicy::TimeBox {
                limit: Duration::from_millis(v.limit_ms),
                inner: Box::new((*v.inner).into()),
            },
            ApiRetryPolicy::Clamp(v) => RetryPolicy::Clamp {
                min_delay: Duration::from_millis(v.min_delay_ms),
                max_delay: Duration::from_millis(v.max_delay_ms),
                inner: Box::new((*v.inner).into()),
            },
            ApiRetryPolicy::AddDelay(v) => RetryPolicy::AddDelay {
                delay: Duration::from_millis(v.delay_ms),
                inner: Box::new((*v.inner).into()),
            },
            ApiRetryPolicy::Jitter(v) => RetryPolicy::Jitter {
                factor: v.factor,
                inner: Box::new((*v.inner).into()),
            },
            ApiRetryPolicy::FilteredOn(v) => RetryPolicy::FilteredOn {
                predicate: v.predicate.into(),
                inner: Box::new((*v.inner).into()),
            },
            ApiRetryPolicy::AndThen(v) => {
                RetryPolicy::AndThen(Box::new((*v.first).into()), Box::new((*v.second).into()))
            }
            ApiRetryPolicy::Union(v) => {
                RetryPolicy::Union(Box::new((*v.first).into()), Box::new((*v.second).into()))
            }
            ApiRetryPolicy::Intersect(v) => {
                RetryPolicy::Intersect(Box::new((*v.first).into()), Box::new((*v.second).into()))
            }
        }
    }
}

fn single_key_object(key: &str, value: JsonValue) -> JsonValue {
    let mut map = JsonMap::new();
    map.insert(key.to_string(), value);
    JsonValue::Object(map)
}

fn take_single_key_object(value: JsonValue) -> Result<(String, JsonValue), String> {
    let JsonValue::Object(map) = value else {
        return Err("Expected object".to_string());
    };

    if map.len() != 1 {
        return Err("Expected object with exactly one key".to_string());
    }

    map.into_iter()
        .next()
        .ok_or_else(|| "Expected object with one entry".to_string())
}

fn take_pair_array(payload: JsonValue, name: &str) -> Result<(JsonValue, JsonValue), String> {
    let JsonValue::Array(items) = payload else {
        return Err(format!("Invalid {name} payload: expected 2-element array"));
    };

    if items.len() != 2 {
        return Err(format!("Invalid {name} payload: expected 2-element array"));
    }

    let mut items = items.into_iter();
    let first = items
        .next()
        .ok_or_else(|| format!("Invalid {name} payload: expected first element"))?;
    let second = items
        .next()
        .ok_or_else(|| format!("Invalid {name} payload: expected second element"))?;
    Ok((first, second))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
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
    fn exponential_strategy_saturates_for_large_counters() {
        // Regression test: with `Policy::exponential(1s, 2.0).clamp(500ms, 10s)` retried
        // forever, after ~64 attempts the unclamped exponential delay overflowed
        // `Duration::MAX.as_secs_f64()` and `Duration::from_secs_f64` panicked before
        // the surrounding clamp could be applied.
        let policy = RetryPolicy::Clamp {
            min_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            inner: Box::new(RetryPolicy::Exponential {
                base_delay: Duration::from_secs(1),
                factor: 2.0,
            }),
        };
        let mut state = policy.initial_state();
        let mut rng = FixedRng(0.0);
        let props = RetryProperties::new();

        for _ in 0..2_000 {
            let (new_state, verdict) = policy.step(&state, Duration::ZERO, &props, &mut rng);
            state = new_state;
            match verdict {
                RetryVerdict::Retry(delay) => {
                    assert!(delay >= Duration::from_millis(500));
                    assert!(delay <= Duration::from_secs(10));
                }
                other => panic!("expected retry verdict, got {other:?}"),
            }
        }
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

        assert!(
            Predicate::PropGt {
                property: "status".to_string(),
                value: PredicateValue::Integer(500),
            }
            .matches(&props)
            .unwrap()
        );

        assert!(
            Predicate::PropMatches {
                property: "service".to_string(),
                pattern: "billing-*".to_string(),
            }
            .matches(&props)
            .unwrap()
        );

        assert!(
            Predicate::PropStartsWith {
                property: "service".to_string(),
                prefix: "bill".to_string(),
            }
            .matches(&props)
            .unwrap()
        );

        assert!(
            Predicate::PropContains {
                property: "service".to_string(),
                substring: "api".to_string(),
            }
            .matches(&props)
            .unwrap()
        );
    }

    #[test]
    fn predicate_references_property_detects_explicit_reference() {
        // Direct leaf reference
        assert!(
            Predicate::PropEq {
                property: "status-code".to_string(),
                value: PredicateValue::Integer(500),
            }
            .references_property("status-code")
        );
        assert!(
            Predicate::PropExists("status-code".to_string()).references_property("status-code")
        );
        assert!(
            Predicate::PropIn {
                property: "status-code".to_string(),
                values: vec![PredicateValue::Integer(500), PredicateValue::Integer(503)],
            }
            .references_property("status-code")
        );

        // Different property — does not reference
        assert!(
            !Predicate::PropEq {
                property: "error-type".to_string(),
                value: PredicateValue::Text("transient".to_string()),
            }
            .references_property("status-code")
        );

        // Catch-all predicates do not reference any property
        assert!(!Predicate::True.references_property("status-code"));
        assert!(!Predicate::False.references_property("status-code"));
    }

    #[test]
    fn predicate_references_property_traverses_composites() {
        let status_eq = Predicate::PropEq {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(500),
        };
        let error_eq = Predicate::PropEq {
            property: "error-type".to_string(),
            value: PredicateValue::Text("transient".to_string()),
        };

        // And/Or detect references in either side
        assert!(
            Predicate::And(Box::new(status_eq.clone()), Box::new(error_eq.clone()))
                .references_property("status-code")
        );
        assert!(
            Predicate::Or(Box::new(error_eq.clone()), Box::new(status_eq.clone()))
                .references_property("status-code")
        );

        // Not detects reference inside negated predicate
        assert!(Predicate::Not(Box::new(status_eq.clone())).references_property("status-code"));

        // Composite of catch-all + unrelated property does not reference
        assert!(
            !Predicate::And(Box::new(Predicate::True), Box::new(error_eq.clone()))
                .references_property("status-code")
        );
        assert!(
            !Predicate::Or(Box::new(error_eq.clone()), Box::new(Predicate::True))
                .references_property("status-code")
        );
    }

    #[test]
    fn retry_policy_references_property_recurses_through_policy_combinators() {
        let status_eq = Predicate::PropEq {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(503),
        };
        let inner = RetryPolicy::Periodic(Duration::from_millis(100));

        // FilteredOn carries a predicate
        let filtered = RetryPolicy::FilteredOn {
            predicate: status_eq.clone(),
            inner: Box::new(inner.clone()),
        };
        assert!(filtered.references_property("status-code"));
        assert!(!filtered.references_property("error-type"));

        // Wrapping combinators recurse into inner
        let wrapped = RetryPolicy::CountBox {
            max_retries: 3,
            inner: Box::new(filtered.clone()),
        };
        assert!(wrapped.references_property("status-code"));

        let wrapped2 = RetryPolicy::TimeBox {
            limit: Duration::from_secs(1),
            inner: Box::new(filtered.clone()),
        };
        assert!(wrapped2.references_property("status-code"));

        // Binary combinators recurse into both sides
        let union = RetryPolicy::Union(
            Box::new(RetryPolicy::Periodic(Duration::from_millis(50))),
            Box::new(filtered.clone()),
        );
        assert!(union.references_property("status-code"));

        // Pure delay/timing policies have no predicate references
        assert!(
            !RetryPolicy::Periodic(Duration::from_millis(100)).references_property("status-code")
        );
        assert!(
            !RetryPolicy::Exponential {
                base_delay: Duration::from_millis(100),
                factor: 2.0,
            }
            .references_property("status-code")
        );
        assert!(!RetryPolicy::Never.references_property("status-code"));
    }

    #[test]
    fn coercion_supports_text_integer_and_rejects_boolean_to_integer() {
        let mut props = RetryProperties::new();
        props.set("attempt", PredicateValue::Text("42".to_string()));

        assert!(
            Predicate::PropEq {
                property: "attempt".to_string(),
                value: PredicateValue::Integer(42),
            }
            .matches(&props)
            .unwrap()
        );

        props.set("attempt", PredicateValue::Integer(42));
        assert!(
            Predicate::PropEq {
                property: "attempt".to_string(),
                value: PredicateValue::Text("42".to_string()),
            }
            .matches(&props)
            .unwrap()
        );

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
    fn resolve_treating_missing_properties_as_no_match_skips_property_not_found() {
        // A status-code-keyed policy that would normally apply to outgoing HTTP
        // responses, plus a fallback policy keyed on the trap context.
        let status_code_policy = NamedRetryPolicy {
            name: "http-5xx".to_string(),
            priority: 100,
            predicate: Predicate::PropEq {
                property: "status-code".to_string(),
                value: PredicateValue::Integer(500),
            },
            policy: RetryPolicy::Immediate,
        };
        let trap_policy = NamedRetryPolicy {
            name: "trap-fallback".to_string(),
            priority: 10,
            predicate: Predicate::PropEq {
                property: "trap-type".to_string(),
                value: PredicateValue::Text("transient-error".to_string()),
            },
            policy: RetryPolicy::Immediate,
        };

        let policies = vec![status_code_policy.clone(), trap_policy.clone()];

        // Trap context: only `trap-type` is populated, no `status-code`.
        let mut trap_props = RetryProperties::new();
        trap_props.set(
            "trap-type",
            PredicateValue::Text("transient-error".to_string()),
        );

        // The strict resolver errors because `status-code` is referenced but missing.
        let strict = NamedRetryPolicy::resolve(&policies, &trap_props);
        assert!(matches!(
            strict,
            Err(RetryEvaluationError::PropertyNotFound { ref property }) if property == "status-code"
        ));

        // The lenient resolver skips the status-code policy and selects the trap one.
        let resolved = NamedRetryPolicy::resolve_treating_missing_properties_as_no_match(
            &policies,
            &trap_props,
        )
        .expect("resolution should succeed")
        .expect("trap-fallback policy should be selected");
        assert_eq!(resolved.name, "trap-fallback");

        // When no policy matches and missing properties are skipped, returns None
        // instead of an error.
        let unmatched_props = RetryProperties::new();
        let resolved = NamedRetryPolicy::resolve_treating_missing_properties_as_no_match(
            &policies,
            &unmatched_props,
        )
        .expect("resolution should succeed");
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_applicable_treating_missing_properties_skips_policy_body_property_references() {
        // Status-code retry policies can encode their status predicate inside
        // the policy body rather than in the named-policy selector. Trap
        // recovery has no `status-code` property, so this policy cannot apply
        // there and must be skipped in favour of the trap/default fallback.
        let status_code_policy = NamedRetryPolicy {
            name: "http-5xx-filtered-body".to_string(),
            priority: 100,
            predicate: Predicate::True,
            policy: RetryPolicy::FilteredOn {
                predicate: Predicate::PropEq {
                    property: "status-code".to_string(),
                    value: PredicateValue::Integer(500),
                },
                inner: Box::new(RetryPolicy::Immediate),
            },
        };
        let trap_policy = NamedRetryPolicy {
            name: "trap-fallback".to_string(),
            priority: 10,
            predicate: Predicate::PropEq {
                property: "trap-type".to_string(),
                value: PredicateValue::Text("transient-error".to_string()),
            },
            policy: RetryPolicy::Immediate,
        };

        let mut trap_props = RetryProperties::new();
        trap_props.set(
            "trap-type",
            PredicateValue::Text("transient-error".to_string()),
        );

        let policies = vec![status_code_policy, trap_policy];
        let resolved =
            NamedRetryPolicy::resolve_applicable_treating_missing_properties_as_no_match(
                &policies,
                &trap_props,
            )
            .expect("resolution should succeed")
            .expect("trap fallback should be selected when status-code is missing");

        assert_eq!(resolved.name, "trap-fallback");
    }

    #[test]
    fn lenient_predicate_not_missing_property_does_not_match() {
        let predicate = Predicate::Not(Box::new(Predicate::PropEq {
            property: "status-code".to_string(),
            value: PredicateValue::Integer(500),
        }));

        assert!(
            !predicate
                .matches_treating_missing_properties_as_false(&RetryProperties::new())
                .expect("lenient predicate evaluation should succeed"),
            "missing properties under Not must remain a no-match"
        );

        let mut props = RetryProperties::new();
        props.set("status-code", PredicateValue::Integer(404));
        assert!(
            predicate
                .matches_treating_missing_properties_as_false(&props)
                .expect("lenient predicate evaluation should succeed"),
            "Not should still match when the referenced property exists and does not match"
        );
    }

    #[test]
    fn resolve_treating_missing_properties_as_no_match_propagates_other_errors() {
        // A predicate that would fail with a coercion error (text vs integer),
        // not with PropertyNotFound. That kind of error must still surface.
        let bad_policy = NamedRetryPolicy {
            name: "type-mismatch".to_string(),
            priority: 100,
            predicate: Predicate::PropGt {
                property: "verb".to_string(),
                value: PredicateValue::Integer(0),
            },
            policy: RetryPolicy::Immediate,
        };

        let mut props = RetryProperties::new();
        props.set("verb", PredicateValue::Text("trap".to_string()));

        let resolved = NamedRetryPolicy::resolve_treating_missing_properties_as_no_match(
            std::slice::from_ref(&bad_policy),
            &props,
        );
        assert!(matches!(
            resolved,
            Err(RetryEvaluationError::CoercionFailed { .. })
        ));
    }

    #[test]
    fn retry_policy_accepts_humantime_duration_strings() {
        // Periodic — Duration is the bare payload (not inside a struct).
        let json = r#"{ "periodic": "200ms" }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
        assert_eq!(parsed, RetryPolicy::Periodic(Duration::from_millis(200)));

        // Exponential — Duration is a struct field with humantime form.
        let json = r#"{ "exponential": { "baseDelay": "500ms", "factor": 2.0 } }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
        assert_eq!(
            parsed,
            RetryPolicy::Exponential {
                base_delay: Duration::from_millis(500),
                factor: 2.0,
            }
        );

        // Clamp + nested exponential.
        let json = r#"{
            "clamp": {
                "minDelay": "100ms",
                "maxDelay": "5s",
                "inner": { "exponential": { "baseDelay": "200ms", "factor": 2.0 } }
            }
        }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
        assert_eq!(
            parsed,
            RetryPolicy::Clamp {
                min_delay: Duration::from_millis(100),
                max_delay: Duration::from_secs(5),
                inner: Box::new(RetryPolicy::Exponential {
                    base_delay: Duration::from_millis(200),
                    factor: 2.0,
                }),
            }
        );

        // Fibonacci.
        let json = r#"{ "fibonacci": { "first": "1s", "second": "2s" } }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
        assert_eq!(
            parsed,
            RetryPolicy::Fibonacci {
                first: Duration::from_secs(1),
                second: Duration::from_secs(2),
            }
        );

        // TimeBox.
        let json = r#"{ "timeBox": { "limit": "30s", "inner": "immediate" } }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
        assert_eq!(
            parsed,
            RetryPolicy::TimeBox {
                limit: Duration::from_secs(30),
                inner: Box::new(RetryPolicy::Immediate),
            }
        );

        // AddDelay.
        let json = r#"{ "addDelay": { "delay": "750ms", "inner": "immediate" } }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse humantime");
        assert_eq!(
            parsed,
            RetryPolicy::AddDelay {
                delay: Duration::from_millis(750),
                inner: Box::new(RetryPolicy::Immediate),
            }
        );
    }

    #[test]
    fn retry_policy_still_accepts_struct_duration_form() {
        // Both string and struct forms must be accepted to preserve backward
        // compatibility with the Rust `Duration` JSON serialization.
        let json = r#"{
            "exponential": {
                "baseDelay": { "secs": 1, "nanos": 500000000 },
                "factor": 2.0
            }
        }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse struct form");
        assert_eq!(
            parsed,
            RetryPolicy::Exponential {
                base_delay: Duration::from_millis(1500),
                factor: 2.0,
            }
        );

        // Periodic struct form.
        let json = r#"{ "periodic": { "secs": 1, "nanos": 0 } }"#;
        let parsed: RetryPolicy = serde_json::from_str(json).expect("should parse struct form");
        assert_eq!(parsed, RetryPolicy::Periodic(Duration::from_secs(1)));
    }

    #[test]
    fn retry_policy_rejects_invalid_duration_string() {
        let json = r#"{ "periodic": "definitely not a duration" }"#;
        let result: Result<RetryPolicy, _> = serde_json::from_str(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("invalid duration string"),
            "expected duration error, got: {err}"
        );
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
    fn retry_predicate_json_and_yaml_example() {
        let model = Predicate::And(
            Box::new(Predicate::True),
            Box::new(Predicate::PropEq {
                property: "status".to_string(),
                value: PredicateValue::Text("transient".to_string()),
            }),
        );

        let json_example = serde_json::json!({
            "and": [
                true,
                {
                    "propEq": {
                        "property": "status",
                        "value": "transient"
                    }
                }
            ]
        });

        let yaml_example = indoc! {r#"
            and:
              - true
              - propEq:
                  property: status
                  value: transient
        "#};

        let from_json =
            serde_json::from_value::<Predicate>(json_example.clone()).expect("json should parse");
        let from_yaml = serde_yaml::from_str::<Predicate>(yaml_example).expect("yaml should parse");

        assert_eq!(from_json, model);
        assert_eq!(from_yaml, model);

        let normalized = serde_json::to_value(model).expect("predicate should serialize");
        assert_eq!(normalized, json_example);
    }

    #[test]
    fn retry_policy_json_and_yaml_example() {
        let model = RetryPolicy::AndThen(
            Box::new(RetryPolicy::Periodic(Duration::from_secs(1))),
            Box::new(RetryPolicy::CountBox {
                max_retries: 3,
                inner: Box::new(RetryPolicy::Never),
            }),
        );

        let json_example = serde_json::json!({
            "andThen": [
                {
                    "periodic": {
                        "secs": 1,
                        "nanos": 0
                    }
                },
                {
                    "countBox": {
                        "maxRetries": 3,
                        "inner": "never"
                    }
                }
            ]
        });

        let yaml_example = indoc! {r#"
            andThen:
              - periodic:
                  secs: 1
                  nanos: 0
              - countBox:
                  maxRetries: 3
                  inner: never
        "#};

        let from_json =
            serde_json::from_value::<RetryPolicy>(json_example.clone()).expect("json should parse");
        let from_yaml =
            serde_yaml::from_str::<RetryPolicy>(yaml_example).expect("yaml should parse");

        assert_eq!(from_json, model);
        assert_eq!(from_yaml, model);

        let normalized = serde_json::to_value(model).expect("retry policy should serialize");
        assert_eq!(normalized, json_example);
    }

    #[test]
    fn retry_policy_yaml_roundtrip_is_plain_yaml() {
        let model = RetryPolicy::FilteredOn {
            predicate: Predicate::False,
            inner: Box::new(RetryPolicy::Immediate),
        };

        let yaml = serde_yaml::to_string(&model).expect("retry policy yaml serialization");
        assert!(!yaml.contains('!'));

        let reparsed =
            serde_yaml::from_str::<RetryPolicy>(&yaml).expect("serialized yaml should parse");
        assert_eq!(reparsed, model);
    }

    #[test]
    fn predicate_units_are_lenient_and_normalized() {
        let json_bool = serde_json::json!(true);
        let json_string = serde_json::json!("true");
        let yaml_bool = indoc! {"true\n"};
        let yaml_string = indoc! {"\"true\"\n"};

        let from_json_bool = serde_json::from_value::<Predicate>(json_bool)
            .expect("json bool predicate unit should deserialize");
        let from_json_string = serde_json::from_value::<Predicate>(json_string)
            .expect("json string predicate unit should deserialize");
        let from_yaml_bool =
            serde_yaml::from_str::<Predicate>(yaml_bool).expect("yaml bool should deserialize");
        let from_yaml_string =
            serde_yaml::from_str::<Predicate>(yaml_string).expect("yaml string should deserialize");

        assert_eq!(from_json_bool, Predicate::True);
        assert_eq!(from_json_string, Predicate::True);
        assert_eq!(from_yaml_bool, Predicate::True);
        assert_eq!(from_yaml_string, Predicate::True);

        let normalized = serde_json::to_value(Predicate::True).expect("predicate should serialize");
        assert_eq!(normalized, serde_json::json!(true));
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
