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

use crate::model::retry_policy::{Predicate, PredicateValue, RetryPolicy};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// --- ApiPredicateValue ---

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiTextValue {
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiIntegerValue {
    pub value: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiBooleanValue {
    pub value: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ApiPredicateValue {
    Text(ApiTextValue),
    Integer(ApiIntegerValue),
    Boolean(ApiBooleanValue),
}

// --- ApiPredicate ---

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyComparison {
    pub property: String,
    pub value: ApiPredicateValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyExistence {
    pub property: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertySetCheck {
    pub property: String,
    pub values: Vec<ApiPredicateValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyPattern {
    pub property: String,
    pub pattern: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertyPrefix {
    pub property: String,
    pub prefix: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPropertySubstring {
    pub property: String,
    pub substring: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicatePair {
    pub left: Box<ApiPredicate>,
    pub right: Box<ApiPredicate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicateNot {
    pub predicate: Box<ApiPredicate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicateTrue {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPredicateFalse {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ApiPredicate {
    PropEq(ApiPropertyComparison),
    PropNeq(ApiPropertyComparison),
    PropGt(ApiPropertyComparison),
    PropGte(ApiPropertyComparison),
    PropLt(ApiPropertyComparison),
    PropLte(ApiPropertyComparison),
    PropExists(ApiPropertyExistence),
    PropIn(ApiPropertySetCheck),
    PropMatches(ApiPropertyPattern),
    PropStartsWith(ApiPropertyPrefix),
    PropContains(ApiPropertySubstring),
    And(ApiPredicatePair),
    Or(ApiPredicatePair),
    Not(ApiPredicateNot),
    True(ApiPredicateTrue),
    False(ApiPredicateFalse),
}

// --- ApiRetryPolicy ---

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiPeriodicPolicy {
    pub delay_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiExponentialPolicy {
    pub base_delay_ms: u64,
    pub factor: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiFibonacciPolicy {
    pub first_ms: u64,
    pub second_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiImmediatePolicy {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiNeverPolicy {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiCountBoxPolicy {
    pub max_retries: u32,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiTimeBoxPolicy {
    pub limit_ms: u64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiClampPolicy {
    pub min_delay_ms: u64,
    pub max_delay_ms: u64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiAddDelayPolicy {
    pub delay_ms: u64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiJitterPolicy {
    pub factor: f64,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiFilteredOnPolicy {
    pub predicate: ApiPredicate,
    pub inner: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ApiRetryPolicyPair {
    pub first: Box<ApiRetryPolicy>,
    pub second: Box<ApiRetryPolicy>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ApiRetryPolicy {
    Periodic(ApiPeriodicPolicy),
    Exponential(ApiExponentialPolicy),
    Fibonacci(ApiFibonacciPolicy),
    Immediate(ApiImmediatePolicy),
    Never(ApiNeverPolicy),
    CountBox(ApiCountBoxPolicy),
    TimeBox(ApiTimeBoxPolicy),
    Clamp(ApiClampPolicy),
    AddDelay(ApiAddDelayPolicy),
    Jitter(ApiJitterPolicy),
    FilteredOn(ApiFilteredOnPolicy),
    AndThen(ApiRetryPolicyPair),
    Union(ApiRetryPolicyPair),
    Intersect(ApiRetryPolicyPair),
}

// --- From conversions: PredicateValue ---

impl From<PredicateValue> for ApiPredicateValue {
    fn from(value: PredicateValue) -> Self {
        match value {
            PredicateValue::Text(v) => ApiPredicateValue::Text(ApiTextValue { value: v }),
            PredicateValue::Integer(v) => ApiPredicateValue::Integer(ApiIntegerValue { value: v }),
            PredicateValue::Boolean(v) => ApiPredicateValue::Boolean(ApiBooleanValue { value: v }),
        }
    }
}

impl From<ApiPredicateValue> for PredicateValue {
    fn from(value: ApiPredicateValue) -> Self {
        match value {
            ApiPredicateValue::Text(v) => PredicateValue::Text(v.value),
            ApiPredicateValue::Integer(v) => PredicateValue::Integer(v.value),
            ApiPredicateValue::Boolean(v) => PredicateValue::Boolean(v.value),
        }
    }
}

// --- From conversions: Predicate ---

impl From<Predicate> for ApiPredicate {
    fn from(value: Predicate) -> Self {
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

impl From<ApiPredicate> for Predicate {
    fn from(value: ApiPredicate) -> Self {
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

// --- From conversions: RetryPolicy ---

impl From<RetryPolicy> for ApiRetryPolicy {
    fn from(value: RetryPolicy) -> Self {
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
            RetryPolicy::AddDelay { delay, inner } => {
                ApiRetryPolicy::AddDelay(ApiAddDelayPolicy {
                    delay_ms: delay.as_millis() as u64,
                    inner: Box::new((*inner).into()),
                })
            }
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

impl From<ApiRetryPolicy> for RetryPolicy {
    fn from(value: ApiRetryPolicy) -> Self {
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
            ApiRetryPolicy::AndThen(v) => RetryPolicy::AndThen(
                Box::new((*v.first).into()),
                Box::new((*v.second).into()),
            ),
            ApiRetryPolicy::Union(v) => RetryPolicy::Union(
                Box::new((*v.first).into()),
                Box::new((*v.second).into()),
            ),
            ApiRetryPolicy::Intersect(v) => RetryPolicy::Intersect(
                Box::new((*v.first).into()),
                Box::new((*v.second).into()),
            ),
        }
    }
}
