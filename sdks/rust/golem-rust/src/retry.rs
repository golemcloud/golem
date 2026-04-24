use crate::bindings::golem::api::retry as retry_api;

pub use builder::{NamedPolicy, Policy, Predicate, Props, RetryBuilderError, Value};
pub use retry_api::{NamedRetryPolicy, PredicateValue, RetryPolicy, RetryPredicate};

/// Get all retry policies active for this agent.
pub fn get_retry_policies() -> Vec<NamedRetryPolicy> {
    retry_api::get_retry_policies()
}

/// Get a specific retry policy by name.
pub fn get_retry_policy_by_name(name: &str) -> Option<NamedRetryPolicy> {
    retry_api::get_retry_policy_by_name(name)
}

/// Resolve the matching retry policy for a given operation context.
/// Evaluates named policies in descending priority order; returns the
/// policy from the first rule whose predicate matches, or none.
pub fn resolve_retry_policy(
    verb: &str,
    noun_uri: &str,
    properties: &[(String, PredicateValue)],
) -> Option<RetryPolicy> {
    let props: Vec<(String, PredicateValue)> = properties.to_vec();
    retry_api::resolve_retry_policy(verb, noun_uri, &props)
}

/// Add or overwrite a named retry policy (persisted to oplog).
/// If a policy with the same name exists, it is replaced.
pub fn set_retry_policy(policy: &NamedRetryPolicy) {
    retry_api::set_retry_policy(policy);
}

/// Add or overwrite a high-level named retry policy after validating and
/// flattening it into the raw WIT representation.
pub fn set_named_policy(policy: &NamedPolicy) -> Result<(), RetryBuilderError> {
    let raw = policy.try_to_raw()?;
    set_retry_policy(&raw);
    Ok(())
}

/// Remove a named retry policy by name (persisted to oplog).
pub fn remove_retry_policy(name: &str) {
    retry_api::remove_retry_policy(name);
}

/// Guard that restores the previous state of a named retry policy on drop.
/// If the policy existed before, it is restored; if it was newly added, it is removed.
pub struct RetryPolicyGuard {
    previous: Option<NamedRetryPolicy>,
    name: String,
}

impl Drop for RetryPolicyGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(original) => set_retry_policy(&original),
            None => remove_retry_policy(&self.name),
        }
    }
}

/// Temporarily sets a named retry policy. When the returned guard is dropped,
/// the previous policy with the same name is restored (or removed if it didn't exist).
#[must_use]
pub fn use_retry_policy(policy: NamedRetryPolicy) -> RetryPolicyGuard {
    let previous = get_retry_policy_by_name(&policy.name);
    let name = policy.name.clone();
    set_retry_policy(&policy);
    RetryPolicyGuard { previous, name }
}

/// Temporarily sets a high-level named retry policy after validating and
/// flattening it into the raw WIT representation.
pub fn use_named_policy(policy: &NamedPolicy) -> Result<RetryPolicyGuard, RetryBuilderError> {
    let raw = policy.try_to_raw()?;
    Ok(use_retry_policy(raw))
}

/// Executes the given function with a named retry policy temporarily set.
pub fn with_retry_policy<R>(policy: NamedRetryPolicy, f: impl FnOnce() -> R) -> R {
    let _guard = use_retry_policy(policy);
    f()
}

/// Executes the given async function with a named retry policy temporarily set.
pub async fn with_retry_policy_async<R, F: std::future::Future<Output = R>>(
    policy: NamedRetryPolicy,
    f: impl FnOnce() -> F,
) -> R {
    let _guard = use_retry_policy(policy);
    f().await
}

/// Executes the given function with a high-level named retry policy temporarily set.
pub fn with_named_policy<R>(
    policy: &NamedPolicy,
    f: impl FnOnce() -> R,
) -> Result<R, RetryBuilderError> {
    let _guard = use_named_policy(policy)?;
    Ok(f())
}

/// Executes the given async function with a high-level named retry policy temporarily set.
///
/// This mutates the agent's active named retry policies for the lifetime of the future.
/// If the future yields, other interleaved work in the same agent may observe the
/// temporary policy until the guard is dropped.
pub async fn with_named_policy_async<R, F: std::future::Future<Output = R>>(
    policy: &NamedPolicy,
    f: impl FnOnce() -> F,
) -> Result<R, RetryBuilderError> {
    let _guard = use_named_policy(policy)?;
    Ok(f().await)
}

pub mod builder {
    use super::retry_api;
    use std::error::Error;
    use std::fmt;
    use std::time::Duration;

    /// Standard retry-property keys populated by the platform retry context.
    pub struct Props;

    impl Props {
        pub const VERB: &str = "verb";
        pub const NOUN_URI: &str = "noun-uri";
        pub const URI_SCHEME: &str = "uri-scheme";
        pub const URI_HOST: &str = "uri-host";
        pub const URI_PORT: &str = "uri-port";
        pub const URI_PATH: &str = "uri-path";
        pub const STATUS_CODE: &str = "status-code";
        pub const ERROR_TYPE: &str = "error-type";
        pub const FUNCTION: &str = "function";
        pub const TARGET_COMPONENT_ID: &str = "target-component-id";
        pub const TARGET_AGENT_TYPE: &str = "target-agent-type";
        pub const DB_TYPE: &str = "db-type";
        pub const TRAP_TYPE: &str = "trap-type";
    }

    /// High-level predicate value accepted by the retry builder layer.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Value {
        Text(String),
        Integer(i128),
        Boolean(bool),
    }

    impl From<String> for Value {
        fn from(value: String) -> Self {
            Self::Text(value)
        }
    }

    impl From<&str> for Value {
        fn from(value: &str) -> Self {
            Self::Text(value.to_string())
        }
    }

    impl From<bool> for Value {
        fn from(value: bool) -> Self {
            Self::Boolean(value)
        }
    }

    impl From<i8> for Value {
        fn from(value: i8) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<i16> for Value {
        fn from(value: i16) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<i32> for Value {
        fn from(value: i32) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<i64> for Value {
        fn from(value: i64) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<isize> for Value {
        fn from(value: isize) -> Self {
            Self::Integer(value as i128)
        }
    }

    impl From<u8> for Value {
        fn from(value: u8) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<u16> for Value {
        fn from(value: u16) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<u32> for Value {
        fn from(value: u32) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<u64> for Value {
        fn from(value: u64) -> Self {
            Self::Integer(value.into())
        }
    }

    impl From<usize> for Value {
        fn from(value: usize) -> Self {
            Self::Integer(value as i128)
        }
    }

    /// High-level retry predicate that keeps tree structure until conversion.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Predicate {
        Eq {
            property: String,
            value: Value,
        },
        Neq {
            property: String,
            value: Value,
        },
        Gt {
            property: String,
            value: Value,
        },
        Gte {
            property: String,
            value: Value,
        },
        Lt {
            property: String,
            value: Value,
        },
        Lte {
            property: String,
            value: Value,
        },
        Exists(String),
        OneOf {
            property: String,
            values: Vec<Value>,
        },
        MatchesGlob {
            property: String,
            pattern: String,
        },
        StartsWith {
            property: String,
            prefix: String,
        },
        Contains {
            property: String,
            substring: String,
        },
        And(Box<Predicate>, Box<Predicate>),
        Or(Box<Predicate>, Box<Predicate>),
        Not(Box<Predicate>),
        Always,
        Never,
    }

    impl Predicate {
        pub fn always() -> Self {
            Self::Always
        }

        pub fn never() -> Self {
            Self::Never
        }

        pub fn eq(property: impl Into<String>, value: impl Into<Value>) -> Self {
            Self::Eq {
                property: property.into(),
                value: value.into(),
            }
        }

        pub fn neq(property: impl Into<String>, value: impl Into<Value>) -> Self {
            Self::Neq {
                property: property.into(),
                value: value.into(),
            }
        }

        pub fn gt(property: impl Into<String>, value: impl Into<Value>) -> Self {
            Self::Gt {
                property: property.into(),
                value: value.into(),
            }
        }

        pub fn gte(property: impl Into<String>, value: impl Into<Value>) -> Self {
            Self::Gte {
                property: property.into(),
                value: value.into(),
            }
        }

        pub fn lt(property: impl Into<String>, value: impl Into<Value>) -> Self {
            Self::Lt {
                property: property.into(),
                value: value.into(),
            }
        }

        pub fn lte(property: impl Into<String>, value: impl Into<Value>) -> Self {
            Self::Lte {
                property: property.into(),
                value: value.into(),
            }
        }

        pub fn exists(property: impl Into<String>) -> Self {
            Self::Exists(property.into())
        }

        pub fn one_of<I, V>(property: impl Into<String>, values: I) -> Self
        where
            I: IntoIterator<Item = V>,
            V: Into<Value>,
        {
            Self::OneOf {
                property: property.into(),
                values: values.into_iter().map(Into::into).collect(),
            }
        }

        pub fn matches_glob(property: impl Into<String>, pattern: impl Into<String>) -> Self {
            Self::MatchesGlob {
                property: property.into(),
                pattern: pattern.into(),
            }
        }

        pub fn starts_with(property: impl Into<String>, prefix: impl Into<String>) -> Self {
            Self::StartsWith {
                property: property.into(),
                prefix: prefix.into(),
            }
        }

        pub fn contains(property: impl Into<String>, substring: impl Into<String>) -> Self {
            Self::Contains {
                property: property.into(),
                substring: substring.into(),
            }
        }

        pub fn and(left: Predicate, right: Predicate) -> Self {
            Self::And(Box::new(left), Box::new(right))
        }

        pub fn or(left: Predicate, right: Predicate) -> Self {
            Self::Or(Box::new(left), Box::new(right))
        }

        #[allow(clippy::should_implement_trait)]
        pub fn not(inner: Predicate) -> Self {
            Self::Not(Box::new(inner))
        }

        pub fn try_to_raw(&self) -> Result<retry_api::RetryPredicate, RetryBuilderError> {
            self.clone().try_into()
        }
    }

    /// High-level retry policy that keeps tree structure until conversion.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Policy {
        Periodic(Duration),
        Exponential {
            base_delay: Duration,
            factor: f64,
        },
        Fibonacci {
            first: Duration,
            second: Duration,
        },
        Immediate,
        Never,
        CountBox {
            max_retries: u32,
            inner: Box<Policy>,
        },
        TimeBox {
            limit: Duration,
            inner: Box<Policy>,
        },
        Clamp {
            min_delay: Duration,
            max_delay: Duration,
            inner: Box<Policy>,
        },
        AddDelay {
            delay: Duration,
            inner: Box<Policy>,
        },
        Jitter {
            factor: f64,
            inner: Box<Policy>,
        },
        OnlyWhen {
            predicate: Predicate,
            inner: Box<Policy>,
        },
        AndThen(Box<Policy>, Box<Policy>),
        Union(Box<Policy>, Box<Policy>),
        Intersect(Box<Policy>, Box<Policy>),
    }

    impl Policy {
        pub fn immediate() -> Self {
            Self::Immediate
        }

        pub fn never() -> Self {
            Self::Never
        }

        pub fn periodic(delay: Duration) -> Self {
            Self::Periodic(delay)
        }

        pub fn exponential(base_delay: Duration, factor: f64) -> Self {
            Self::Exponential { base_delay, factor }
        }

        pub fn fibonacci(first: Duration, second: Duration) -> Self {
            Self::Fibonacci { first, second }
        }

        pub fn max_retries(self, max_retries: u32) -> Self {
            Self::CountBox {
                max_retries,
                inner: Box::new(self),
            }
        }

        pub fn within(self, limit: Duration) -> Self {
            Self::TimeBox {
                limit,
                inner: Box::new(self),
            }
        }

        pub fn clamp(self, min_delay: Duration, max_delay: Duration) -> Self {
            Self::Clamp {
                min_delay,
                max_delay,
                inner: Box::new(self),
            }
        }

        pub fn add_delay(self, delay: Duration) -> Self {
            Self::AddDelay {
                delay,
                inner: Box::new(self),
            }
        }

        pub fn with_jitter(self, factor: f64) -> Self {
            Self::Jitter {
                factor,
                inner: Box::new(self),
            }
        }

        pub fn only_when(self, predicate: Predicate) -> Self {
            Self::OnlyWhen {
                predicate,
                inner: Box::new(self),
            }
        }

        pub fn and_then(self, other: Policy) -> Self {
            Self::AndThen(Box::new(self), Box::new(other))
        }

        pub fn union(self, other: Policy) -> Self {
            Self::Union(Box::new(self), Box::new(other))
        }

        pub fn intersect(self, other: Policy) -> Self {
            Self::Intersect(Box::new(self), Box::new(other))
        }

        pub fn try_to_raw(&self) -> Result<retry_api::RetryPolicy, RetryBuilderError> {
            self.clone().try_into()
        }
    }

    /// High-level named retry rule with an outer applicability predicate.
    #[derive(Clone, Debug, PartialEq)]
    pub struct NamedPolicy {
        name: String,
        priority: u32,
        predicate: Predicate,
        policy: Policy,
    }

    impl NamedPolicy {
        pub fn named(name: impl Into<String>, policy: Policy) -> Self {
            Self {
                name: name.into(),
                priority: 0,
                predicate: Predicate::always(),
                policy,
            }
        }

        pub fn priority(mut self, priority: u32) -> Self {
            self.priority = priority;
            self
        }

        pub fn applies_when(mut self, predicate: Predicate) -> Self {
            self.predicate = predicate;
            self
        }

        pub fn try_to_raw(&self) -> Result<retry_api::NamedRetryPolicy, RetryBuilderError> {
            self.clone().try_into()
        }
    }

    /// Validation errors produced when flattening high-level retry builders into raw WIT types.
    #[derive(Clone, Debug, PartialEq)]
    pub enum RetryBuilderError {
        IntegerOutOfRange {
            value: i128,
        },
        DurationOutOfRange {
            field: &'static str,
            duration: Duration,
        },
        InvalidExponentialFactor {
            factor: f64,
        },
        InvalidJitterFactor {
            factor: f64,
        },
        InvalidClampRange {
            min_delay: Duration,
            max_delay: Duration,
        },
    }

    impl fmt::Display for RetryBuilderError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::IntegerOutOfRange { value } => {
                    write!(
                        f,
                        "predicate integer value {value} does not fit into WIT s64"
                    )
                }
                Self::DurationOutOfRange { field, duration } => write!(
                    f,
                    "{field} duration {duration:?} does not fit into WIT duration nanoseconds"
                ),
                Self::InvalidExponentialFactor { factor } => write!(
                    f,
                    "exponential factor must be finite and greater than 0, got {factor}"
                ),
                Self::InvalidJitterFactor { factor } => write!(
                    f,
                    "jitter factor must be finite and greater than or equal to 0, got {factor}"
                ),
                Self::InvalidClampRange {
                    min_delay,
                    max_delay,
                } => write!(
                    f,
                    "clamp min delay {min_delay:?} must be less than or equal to max delay {max_delay:?}"
                ),
            }
        }
    }

    impl Error for RetryBuilderError {}

    impl TryFrom<Value> for retry_api::PredicateValue {
        type Error = RetryBuilderError;

        fn try_from(value: Value) -> Result<Self, Self::Error> {
            match value {
                Value::Text(value) => Ok(Self::Text(value)),
                Value::Boolean(value) => Ok(Self::Boolean(value)),
                Value::Integer(value) => i64::try_from(value)
                    .map(Self::Integer)
                    .map_err(|_| RetryBuilderError::IntegerOutOfRange { value }),
            }
        }
    }

    impl TryFrom<Predicate> for retry_api::RetryPredicate {
        type Error = RetryBuilderError;

        fn try_from(predicate: Predicate) -> Result<Self, Self::Error> {
            let mut nodes = Vec::new();
            push_predicate_node(predicate, &mut nodes)?;
            Ok(Self { nodes })
        }
    }

    impl TryFrom<Policy> for retry_api::RetryPolicy {
        type Error = RetryBuilderError;

        fn try_from(policy: Policy) -> Result<Self, Self::Error> {
            let mut nodes = Vec::new();
            push_policy_node(policy, &mut nodes)?;
            Ok(Self { nodes })
        }
    }

    impl TryFrom<NamedPolicy> for retry_api::NamedRetryPolicy {
        type Error = RetryBuilderError;

        fn try_from(policy: NamedPolicy) -> Result<Self, Self::Error> {
            Ok(Self {
                name: policy.name,
                priority: policy.priority,
                predicate: policy.predicate.try_to_raw()?,
                policy: policy.policy.try_to_raw()?,
            })
        }
    }

    fn push_predicate_node(
        predicate: Predicate,
        nodes: &mut Vec<retry_api::PredicateNode>,
    ) -> Result<i32, RetryBuilderError> {
        let index = nodes.len() as i32;
        nodes.push(retry_api::PredicateNode::PredFalse);

        let node = match predicate {
            Predicate::Eq { property, value } => {
                retry_api::PredicateNode::PropEq(retry_api::PropertyComparison {
                    property_name: property,
                    value: value.try_into()?,
                })
            }
            Predicate::Neq { property, value } => {
                retry_api::PredicateNode::PropNeq(retry_api::PropertyComparison {
                    property_name: property,
                    value: value.try_into()?,
                })
            }
            Predicate::Gt { property, value } => {
                retry_api::PredicateNode::PropGt(retry_api::PropertyComparison {
                    property_name: property,
                    value: value.try_into()?,
                })
            }
            Predicate::Gte { property, value } => {
                retry_api::PredicateNode::PropGte(retry_api::PropertyComparison {
                    property_name: property,
                    value: value.try_into()?,
                })
            }
            Predicate::Lt { property, value } => {
                retry_api::PredicateNode::PropLt(retry_api::PropertyComparison {
                    property_name: property,
                    value: value.try_into()?,
                })
            }
            Predicate::Lte { property, value } => {
                retry_api::PredicateNode::PropLte(retry_api::PropertyComparison {
                    property_name: property,
                    value: value.try_into()?,
                })
            }
            Predicate::Exists(property) => retry_api::PredicateNode::PropExists(property),
            Predicate::OneOf { property, values } => {
                retry_api::PredicateNode::PropIn(retry_api::PropertySetCheck {
                    property_name: property,
                    values: values
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<Vec<_>, _>>()?,
                })
            }
            Predicate::MatchesGlob { property, pattern } => {
                retry_api::PredicateNode::PropMatches(retry_api::PropertyPattern {
                    property_name: property,
                    pattern,
                })
            }
            Predicate::StartsWith { property, prefix } => {
                retry_api::PredicateNode::PropStartsWith(retry_api::PropertyPattern {
                    property_name: property,
                    pattern: prefix,
                })
            }
            Predicate::Contains {
                property,
                substring,
            } => retry_api::PredicateNode::PropContains(retry_api::PropertyPattern {
                property_name: property,
                pattern: substring,
            }),
            Predicate::And(left, right) => {
                let left_index = push_predicate_node(*left, nodes)?;
                let right_index = push_predicate_node(*right, nodes)?;
                retry_api::PredicateNode::PredAnd((left_index, right_index))
            }
            Predicate::Or(left, right) => {
                let left_index = push_predicate_node(*left, nodes)?;
                let right_index = push_predicate_node(*right, nodes)?;
                retry_api::PredicateNode::PredOr((left_index, right_index))
            }
            Predicate::Not(inner) => {
                let inner_index = push_predicate_node(*inner, nodes)?;
                retry_api::PredicateNode::PredNot(inner_index)
            }
            Predicate::Always => retry_api::PredicateNode::PredTrue,
            Predicate::Never => retry_api::PredicateNode::PredFalse,
        };

        nodes[index as usize] = node;
        Ok(index)
    }

    fn push_policy_node(
        policy: Policy,
        nodes: &mut Vec<retry_api::PolicyNode>,
    ) -> Result<i32, RetryBuilderError> {
        let index = nodes.len() as i32;
        nodes.push(retry_api::PolicyNode::Never);

        let node = match policy {
            Policy::Periodic(delay) => {
                retry_api::PolicyNode::Periodic(duration_to_wit(delay, "periodic delay")?)
            }
            Policy::Exponential { base_delay, factor } => {
                if !factor.is_finite() || factor <= 0.0 {
                    return Err(RetryBuilderError::InvalidExponentialFactor { factor });
                }

                retry_api::PolicyNode::Exponential(retry_api::ExponentialConfig {
                    base_delay: duration_to_wit(base_delay, "exponential base delay")?,
                    factor,
                })
            }
            Policy::Fibonacci { first, second } => {
                retry_api::PolicyNode::Fibonacci(retry_api::FibonacciConfig {
                    first: duration_to_wit(first, "fibonacci first delay")?,
                    second: duration_to_wit(second, "fibonacci second delay")?,
                })
            }
            Policy::Immediate => retry_api::PolicyNode::Immediate,
            Policy::Never => retry_api::PolicyNode::Never,
            Policy::CountBox { max_retries, inner } => {
                let inner_index = push_policy_node(*inner, nodes)?;
                retry_api::PolicyNode::CountBox(retry_api::CountBoxConfig {
                    max_retries,
                    inner: inner_index,
                })
            }
            Policy::TimeBox { limit, inner } => {
                let inner_index = push_policy_node(*inner, nodes)?;
                retry_api::PolicyNode::TimeBox(retry_api::TimeBoxConfig {
                    limit: duration_to_wit(limit, "time-box limit")?,
                    inner: inner_index,
                })
            }
            Policy::Clamp {
                min_delay,
                max_delay,
                inner,
            } => {
                if min_delay > max_delay {
                    return Err(RetryBuilderError::InvalidClampRange {
                        min_delay,
                        max_delay,
                    });
                }

                let inner_index = push_policy_node(*inner, nodes)?;
                retry_api::PolicyNode::ClampDelay(retry_api::ClampConfig {
                    min_delay: duration_to_wit(min_delay, "clamp min delay")?,
                    max_delay: duration_to_wit(max_delay, "clamp max delay")?,
                    inner: inner_index,
                })
            }
            Policy::AddDelay { delay, inner } => {
                let inner_index = push_policy_node(*inner, nodes)?;
                retry_api::PolicyNode::AddDelay(retry_api::AddDelayConfig {
                    delay: duration_to_wit(delay, "added delay")?,
                    inner: inner_index,
                })
            }
            Policy::Jitter { factor, inner } => {
                if !factor.is_finite() || factor < 0.0 {
                    return Err(RetryBuilderError::InvalidJitterFactor { factor });
                }

                let inner_index = push_policy_node(*inner, nodes)?;
                retry_api::PolicyNode::Jitter(retry_api::JitterConfig {
                    factor,
                    inner: inner_index,
                })
            }
            Policy::OnlyWhen { predicate, inner } => {
                let inner_index = push_policy_node(*inner, nodes)?;
                retry_api::PolicyNode::FilteredOn(retry_api::FilteredConfig {
                    predicate: predicate.try_to_raw()?,
                    inner: inner_index,
                })
            }
            Policy::AndThen(left, right) => {
                let left_index = push_policy_node(*left, nodes)?;
                let right_index = push_policy_node(*right, nodes)?;
                retry_api::PolicyNode::AndThen((left_index, right_index))
            }
            Policy::Union(left, right) => {
                let left_index = push_policy_node(*left, nodes)?;
                let right_index = push_policy_node(*right, nodes)?;
                retry_api::PolicyNode::PolicyUnion((left_index, right_index))
            }
            Policy::Intersect(left, right) => {
                let left_index = push_policy_node(*left, nodes)?;
                let right_index = push_policy_node(*right, nodes)?;
                retry_api::PolicyNode::PolicyIntersect((left_index, right_index))
            }
        };

        nodes[index as usize] = node;
        Ok(index)
    }

    fn duration_to_wit(duration: Duration, field: &'static str) -> Result<u64, RetryBuilderError> {
        let nanos = duration.as_nanos();
        if nanos > u64::MAX as u128 {
            return Err(RetryBuilderError::DurationOutOfRange { field, duration });
        }

        Ok(nanos as u64)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use test_r::test;

    use super::builder::{NamedPolicy, Policy, Predicate, Props, RetryBuilderError};
    use super::retry_api;

    #[test]
    fn named_policy_builder_flattens_to_raw_wit_types() {
        let named = NamedPolicy::named(
            "transient-http",
            Policy::exponential(Duration::from_millis(200), 2.0)
                .clamp(Duration::from_millis(100), Duration::from_secs(5))
                .only_when(Predicate::one_of(Props::STATUS_CODE, [502_u16, 503, 504]))
                .max_retries(3),
        )
        .priority(10)
        .applies_when(Predicate::and(
            Predicate::eq(Props::VERB, "GET"),
            Predicate::matches_glob(Props::URI_HOST, "*.example.com"),
        ));

        let raw = named
            .try_to_raw()
            .expect("builder should convert to raw WIT types");

        assert_eq!(raw.name, "transient-http");
        assert_eq!(raw.priority, 10);
        assert_eq!(raw.predicate.nodes.len(), 3);
        match &raw.predicate.nodes[0] {
            retry_api::PredicateNode::PredAnd((left, right)) => {
                assert_eq!((*left, *right), (1, 2));
            }
            node => panic!("unexpected root predicate node: {node:?}"),
        }
        match &raw.predicate.nodes[1] {
            retry_api::PredicateNode::PropEq(retry_api::PropertyComparison {
                property_name,
                value: retry_api::PredicateValue::Text(value),
            }) => {
                assert_eq!(property_name, Props::VERB);
                assert_eq!(value, "GET");
            }
            node => panic!("unexpected left predicate node: {node:?}"),
        }
        match &raw.predicate.nodes[2] {
            retry_api::PredicateNode::PropMatches(retry_api::PropertyPattern {
                property_name,
                pattern,
            }) => {
                assert_eq!(property_name, Props::URI_HOST);
                assert_eq!(pattern, "*.example.com");
            }
            node => panic!("unexpected right predicate node: {node:?}"),
        }

        assert_eq!(raw.policy.nodes.len(), 4);
        match &raw.policy.nodes[0] {
            retry_api::PolicyNode::CountBox(retry_api::CountBoxConfig { max_retries, inner }) => {
                assert_eq!((*max_retries, *inner), (3, 1));
            }
            node => panic!("unexpected root policy node: {node:?}"),
        }
        match &raw.policy.nodes[1] {
            retry_api::PolicyNode::FilteredOn(retry_api::FilteredConfig { predicate, inner }) => {
                assert_eq!(*inner, 2);
                assert_eq!(predicate.nodes.len(), 1);
                match &predicate.nodes[0] {
                    retry_api::PredicateNode::PropIn(retry_api::PropertySetCheck {
                        property_name,
                        values,
                    }) => {
                        assert_eq!(property_name, Props::STATUS_CODE);
                        assert_eq!(values.len(), 3);
                        match &values[..] {
                            [
                                retry_api::PredicateValue::Integer(502),
                                retry_api::PredicateValue::Integer(503),
                                retry_api::PredicateValue::Integer(504),
                            ] => {}
                            other => panic!("unexpected predicate values: {other:?}"),
                        }
                    }
                    node => panic!("unexpected filtered predicate node: {node:?}"),
                }
            }
            node => panic!("unexpected filtered policy node: {node:?}"),
        }
        match &raw.policy.nodes[2] {
            retry_api::PolicyNode::ClampDelay(retry_api::ClampConfig {
                min_delay,
                max_delay,
                inner,
            }) => {
                assert_eq!(
                    (*min_delay, *max_delay, *inner),
                    (100_000_000, 5_000_000_000, 3)
                );
            }
            node => panic!("unexpected clamp policy node: {node:?}"),
        }
        match &raw.policy.nodes[3] {
            retry_api::PolicyNode::Exponential(retry_api::ExponentialConfig {
                base_delay,
                factor,
            }) => {
                assert_eq!(*base_delay, 200_000_000);
                assert_eq!(*factor, 2.0);
            }
            node => panic!("unexpected exponential policy node: {node:?}"),
        }
    }

    #[test]
    fn predicate_builder_rejects_integer_values_that_do_not_fit_wit() {
        let predicate = Predicate::eq(Props::STATUS_CODE, u64::try_from(i64::MAX).unwrap() + 1);

        assert_eq!(
            predicate.try_to_raw().unwrap_err(),
            RetryBuilderError::IntegerOutOfRange {
                value: i128::from(i64::MAX) + 1,
            }
        );
    }

    #[test]
    fn policy_builder_rejects_invalid_exponential_factors() {
        let policy = Policy::exponential(Duration::from_millis(100), 0.0);

        assert_eq!(
            policy.try_to_raw().unwrap_err(),
            RetryBuilderError::InvalidExponentialFactor { factor: 0.0 }
        );

        assert!(matches!(
            Policy::exponential(Duration::from_millis(100), f64::NAN)
                .try_to_raw()
                .unwrap_err(),
            RetryBuilderError::InvalidExponentialFactor { factor } if factor.is_nan()
        ));
    }

    #[test]
    fn policy_builder_rejects_invalid_jitter_factors() {
        let policy = Policy::periodic(Duration::from_millis(100)).with_jitter(-0.1);

        assert_eq!(
            policy.try_to_raw().unwrap_err(),
            RetryBuilderError::InvalidJitterFactor { factor: -0.1 }
        );
    }

    #[test]
    fn policy_builder_rejects_invalid_clamp_ranges() {
        let policy = Policy::periodic(Duration::from_millis(100))
            .clamp(Duration::from_secs(2), Duration::from_secs(1));

        assert_eq!(
            policy.try_to_raw().unwrap_err(),
            RetryBuilderError::InvalidClampRange {
                min_delay: Duration::from_secs(2),
                max_delay: Duration::from_secs(1),
            }
        );
    }

    #[test]
    fn policy_builder_rejects_durations_that_do_not_fit_wit() {
        let policy = Policy::periodic(Duration::MAX);

        assert_eq!(
            policy.try_to_raw().unwrap_err(),
            RetryBuilderError::DurationOutOfRange {
                field: "periodic delay",
                duration: Duration::MAX,
            }
        );
    }
}
