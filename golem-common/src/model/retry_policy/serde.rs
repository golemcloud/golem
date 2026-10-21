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

use std::time::Duration;

use ::serde::{self, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map as JsonMap, Value as JsonValue};

use super::{Predicate, PredicateValue, RetryPolicy};

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
