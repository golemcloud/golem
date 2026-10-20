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

use std::cmp::Ordering;
use std::ops::Range;
use std::time::Duration;

use rand::Rng;

use super::{
    FixedRng, NamedRetryPolicy, Predicate, PredicateValue, PredicateValueType,
    RetryEvaluationError, RetryPolicy, RetryPolicyState, RetryProperties, RetryVerdict, RngSource,
    ThreadRng,
};
use crate::model::RetryConfig;

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

impl RetryPolicyState {
    /// Returns the total number of retry attempts tracked by this state.
    pub fn retry_count(&self) -> u32 {
        match self {
            RetryPolicyState::Counter(n) => *n,
            RetryPolicyState::Terminal => 0,
            RetryPolicyState::Wrapper(inner) => inner.retry_count(),
            RetryPolicyState::CountBox { attempts, .. } => *attempts,
            RetryPolicyState::TimeBox { inner, .. } => inner.retry_count(),
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

impl RetryPolicy {
    /// Whether every possible retry path has a finite structural bound.
    pub fn is_bounded(&self) -> bool {
        match self {
            Self::Never | Self::CountBox { .. } | Self::TimeBox { .. } => true,
            Self::AndThen(l, r) | Self::Union(l, r) => l.is_bounded() && r.is_bounded(),
            Self::Intersect(l, r) => l.is_bounded() || r.is_bounded(),
            Self::Clamp { inner, .. }
            | Self::AddDelay { inner, .. }
            | Self::Jitter { inner, .. }
            | Self::FilteredOn { inner, .. } => inner.is_bounded(),
            _ => false,
        }
    }
}

impl RngSource for ThreadRng {
    fn random_f64(&mut self, range: Range<f64>) -> f64 {
        rand::rng().random_range(range)
    }
}
impl RngSource for FixedRng {
    fn random_f64(&mut self, _range: Range<f64>) -> f64 {
        self.0
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
    /// Returns whether this policy contains an elapsed-time budget.
    pub fn contains_time_box(&self) -> bool {
        match self {
            RetryPolicy::TimeBox { .. } => true,
            RetryPolicy::CountBox { inner, .. }
            | RetryPolicy::Clamp { inner, .. }
            | RetryPolicy::AddDelay { inner, .. }
            | RetryPolicy::Jitter { inner, .. }
            | RetryPolicy::FilteredOn { inner, .. } => inner.contains_time_box(),
            RetryPolicy::AndThen(left, right)
            | RetryPolicy::Union(left, right)
            | RetryPolicy::Intersect(left, right) => {
                left.contains_time_box() || right.contains_time_box()
            }
            RetryPolicy::Periodic(_)
            | RetryPolicy::Exponential { .. }
            | RetryPolicy::Fibonacci { .. }
            | RetryPolicy::Immediate
            | RetryPolicy::Never => false,
        }
    }

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

        // Legacy `RetryConfig::max_attempts` counts *total* attempts including
        // the first one (`get_delay` returns `None` once `attempts >=
        // max_attempts`), while `CountBox::max_retries` counts *retries*
        // (evaluated after each failure, starting from zero). Subtract one so
        // the synthesized policy performs the same number of physical attempts
        // as the legacy retry loop; `max_attempts: 1` means no retries at all.
        RetryPolicy::CountBox {
            max_retries: config.max_attempts.saturating_sub(1),
            inner: Box::new(inner),
        }
    }
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
