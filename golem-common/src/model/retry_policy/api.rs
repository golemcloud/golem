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

use super::{Predicate, PredicateValue, RetryPolicy};

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
