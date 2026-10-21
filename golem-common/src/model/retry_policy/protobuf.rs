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

use super::{NamedRetryPolicy, Predicate, PredicateValue, RetryPolicy, duration_to_nanos};

fn nanos_to_duration(nanos: u64) -> Duration {
    Duration::from_nanos(nanos)
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
