//! Local retry policy interpretation and execution.

use super::builder::Policy;
use super::retry_api;
use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::ops::AsyncFnMut;
use std::time::Duration;

const MAX_COMPILED_NODES: usize = 4_096;
const MAX_COMPILED_PAYLOAD_BYTES: usize = 1_048_576;
const MAX_POLICY_DEPTH: usize = 256;

/// A validated retry schedule for retrying fallible user code locally.
///
/// Unlike named retry policies installed through [`super::set_retry_policy`], this schedule is
/// interpreted entirely by the component. It does not install a named policy or create a
/// host-managed retry sequence. The loop is ordinary guest execution, so its host calls retain
/// their normal durable replay and suspension behavior.
#[derive(Clone, Debug)]
pub struct RetrySchedule {
    policy: CompiledPolicy,
}

/// An error found while validating a flattened raw retry policy.
#[derive(Clone, Debug, PartialEq)]
pub enum RetryPolicyError {
    EmptyPolicy,
    InvalidPolicyNodeIndex(i32),
    CyclicPolicyNode(i32),
    EmptyPredicate,
    InvalidPredicateNodeIndex(i32),
    CyclicPredicateNode(i32),
    PolicyTooComplex,
    InvalidExponentialFactor(f64),
    InvalidJitterFactor(f64),
    InvalidClampRange {
        min_delay: Duration,
        max_delay: Duration,
    },
}

impl fmt::Display for RetryPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPolicy => write!(f, "retry policy has no root node"),
            Self::InvalidPolicyNodeIndex(index) => {
                write!(f, "retry policy references invalid node index {index}")
            }
            Self::CyclicPolicyNode(index) => {
                write!(f, "retry policy contains a cycle at node index {index}")
            }
            Self::EmptyPredicate => write!(f, "retry predicate has no root node"),
            Self::InvalidPredicateNodeIndex(index) => {
                write!(f, "retry predicate references invalid node index {index}")
            }
            Self::CyclicPredicateNode(index) => {
                write!(f, "retry predicate contains a cycle at node index {index}")
            }
            Self::PolicyTooComplex => write!(f, "retry policy is too deeply nested or expansive"),
            Self::InvalidExponentialFactor(factor) => write!(
                f,
                "exponential factor must be finite and greater than 0, got {factor}"
            ),
            Self::InvalidJitterFactor(factor) => write!(
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

impl Error for RetryPolicyError {}

impl TryFrom<&retry_api::RetryPolicy> for RetrySchedule {
    type Error = RetryPolicyError;

    fn try_from(policy: &retry_api::RetryPolicy) -> Result<Self, Self::Error> {
        if policy.nodes.is_empty() {
            return Err(RetryPolicyError::EmptyPolicy);
        }
        if policy.nodes.len() > MAX_COMPILED_NODES {
            return Err(RetryPolicyError::PolicyTooComplex);
        }
        let mut compiler = Compiler::new();
        Ok(Self {
            policy: compiler.compile_policy_node(&policy.nodes, 0, 0)?,
        })
    }
}

impl Policy {
    /// Compiles this high-level policy into a reusable user-space retry schedule.
    pub fn try_to_schedule(&self) -> Result<RetrySchedule, super::builder::RetryBuilderError> {
        let raw = self.try_to_raw()?;
        RetrySchedule::try_from(&raw)
            .map_err(|_| super::builder::RetryBuilderError::PolicyTooComplex)
    }
}

impl RetrySchedule {
    /// Runs an async operation until it succeeds or this schedule gives up.
    ///
    /// Use [`Self::retry_with_properties`] when the schedule contains filtered predicates.
    pub async fn retry<T, E, Operation>(&self, operation: Operation) -> Result<T, E>
    where
        Operation: AsyncFnMut() -> Result<T, E>,
    {
        self.retry_with_properties(operation, |_| std::iter::empty())
            .await
    }

    /// Runs an async operation with retry properties projected from each failure.
    ///
    /// `properties` is evaluated after every failure, so predicates can react to the current
    /// error. When the schedule gives up, the most recent operation error is returned unchanged.
    pub async fn retry_with_properties<T, E, Operation, Properties, PropertyItems>(
        &self,
        operation: Operation,
        properties: Properties,
    ) -> Result<T, E>
    where
        Operation: AsyncFnMut() -> Result<T, E>,
        Properties: FnMut(&E) -> PropertyItems,
        PropertyItems: IntoIterator<Item = (String, retry_api::PredicateValue)>,
    {
        self.retry_with_runtime(
            operation,
            properties,
            crate::wasip3::clocks::monotonic_clock::now,
            || {
                let value = crate::wasip3::random::random::get_random_u64() >> 11;
                value as f64 * (1.0 / ((1_u64 << 53) as f64))
            },
            |delay| crate::wasip3::clocks::monotonic_clock::wait_for(duration_to_nanos(delay)),
        )
        .await
    }

    async fn retry_with_runtime<
        T,
        E,
        Operation,
        Properties,
        PropertyItems,
        Now,
        Random,
        Sleep,
        SleepFuture,
    >(
        &self,
        mut operation: Operation,
        mut properties: Properties,
        mut now: Now,
        mut random: Random,
        mut sleep: Sleep,
    ) -> Result<T, E>
    where
        Operation: AsyncFnMut() -> Result<T, E>,
        Properties: FnMut(&E) -> PropertyItems,
        PropertyItems: IntoIterator<Item = (String, retry_api::PredicateValue)>,
        Now: FnMut() -> u64,
        Random: FnMut() -> f64,
        Sleep: FnMut(Duration) -> SleepFuture,
        SleepFuture: Future<Output = ()>,
    {
        let started = now();
        let mut policy = self.policy.clone();
        loop {
            match operation().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let properties = properties(&error).into_iter().collect::<Vec<_>>();
                    let elapsed = Duration::from_nanos(now().saturating_sub(started));
                    match policy.step(elapsed, &properties, &mut random) {
                        Step::Retry(delay) => sleep(delay).await,
                        Step::GiveUp | Step::Error => return Err(error),
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
enum CompiledPolicy {
    Periodic(Duration),
    Exponential {
        base_delay: Duration,
        factor: f64,
        attempt: u32,
    },
    Fibonacci {
        previous: Duration,
        current: Duration,
        attempt: u32,
    },
    Immediate,
    Never,
    CountBox {
        max_retries: u32,
        attempts: u32,
        inner: Box<Self>,
    },
    TimeBox {
        limit: Duration,
        inner: Box<Self>,
    },
    Clamp {
        min_delay: Duration,
        max_delay: Duration,
        inner: Box<Self>,
    },
    AddDelay {
        delay: Duration,
        inner: Box<Self>,
    },
    Jitter {
        factor: f64,
        inner: Box<Self>,
    },
    FilteredOn {
        predicate: CompiledPredicate,
        inner: Box<Self>,
    },
    AndThen {
        left: Box<Self>,
        right: Box<Self>,
        on_right: bool,
    },
    Union(Box<Self>, Box<Self>),
    Intersect(Box<Self>, Box<Self>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Retry(Duration),
    GiveUp,
    Error,
}

impl CompiledPolicy {
    fn step(
        &mut self,
        elapsed: Duration,
        properties: &[(String, retry_api::PredicateValue)],
        random: &mut impl FnMut() -> f64,
    ) -> Step {
        match self {
            Self::Periodic(delay) => Step::Retry(*delay),
            Self::Exponential {
                base_delay,
                factor,
                attempt,
            } => {
                let delay = scale_duration(*base_delay, factor.powf(*attempt as f64));
                *attempt = attempt.saturating_add(1);
                Step::Retry(delay)
            }
            Self::Fibonacci {
                previous,
                current,
                attempt,
            } => {
                let delay = match *attempt {
                    0 => *previous,
                    1 => *current,
                    _ => {
                        let next = saturating_add(*previous, *current);
                        *previous = *current;
                        *current = next;
                        next
                    }
                };
                *attempt = attempt.saturating_add(1);
                Step::Retry(delay)
            }
            Self::Immediate => Step::Retry(Duration::ZERO),
            Self::Never => Step::GiveUp,
            Self::CountBox {
                max_retries,
                attempts,
                inner,
            } => {
                if *attempts >= *max_retries {
                    Step::GiveUp
                } else {
                    *attempts = attempts.saturating_add(1);
                    inner.step(elapsed, properties, random)
                }
            }
            Self::TimeBox { limit, inner } => {
                if elapsed >= *limit {
                    Step::GiveUp
                } else {
                    inner.step(elapsed, properties, random)
                }
            }
            Self::Clamp {
                min_delay,
                max_delay,
                inner,
            } => match inner.step(elapsed, properties, random) {
                Step::Retry(delay) => Step::Retry(delay.clamp(*min_delay, *max_delay)),
                verdict => verdict,
            },
            Self::AddDelay { delay, inner } => match inner.step(elapsed, properties, random) {
                Step::Retry(current) => Step::Retry(saturating_add(current, *delay)),
                verdict => verdict,
            },
            Self::Jitter { factor, inner } => match inner.step(elapsed, properties, random) {
                Step::Retry(delay) if *factor > 0.0 => Step::Retry(saturating_add(
                    delay,
                    scale_duration(delay, random() * *factor),
                )),
                verdict => verdict,
            },
            Self::FilteredOn { predicate, inner } => match predicate.matches(properties) {
                Ok(true) => inner.step(elapsed, properties, random),
                Ok(false) => Step::GiveUp,
                Err(_) => Step::Error,
            },
            Self::AndThen {
                left,
                right,
                on_right,
            } => {
                if *on_right {
                    right.step(elapsed, properties, random)
                } else {
                    match left.step(elapsed, properties, random) {
                        Step::Retry(delay) => Step::Retry(delay),
                        Step::GiveUp => {
                            *on_right = true;
                            right.step(elapsed, properties, random)
                        }
                        Step::Error => Step::Error,
                    }
                }
            }
            Self::Union(left, right) => match (
                left.step(elapsed, properties, random),
                right.step(elapsed, properties, random),
            ) {
                (Step::Retry(left), Step::Retry(right)) => Step::Retry(left.min(right)),
                (Step::Retry(delay), Step::GiveUp) | (Step::GiveUp, Step::Retry(delay)) => {
                    Step::Retry(delay)
                }
                (Step::GiveUp, Step::GiveUp) => Step::GiveUp,
                (Step::Error, _) | (_, Step::Error) => Step::Error,
            },
            Self::Intersect(left, right) => match (
                left.step(elapsed, properties, random),
                right.step(elapsed, properties, random),
            ) {
                (Step::Retry(left), Step::Retry(right)) => Step::Retry(left.max(right)),
                (Step::Error, _) | (_, Step::Error) => Step::Error,
                _ => Step::GiveUp,
            },
        }
    }
}

#[derive(Clone, Debug)]
enum CompiledPredicate {
    Eq(String, retry_api::PredicateValue),
    Neq(String, retry_api::PredicateValue),
    Gt(String, retry_api::PredicateValue),
    Gte(String, retry_api::PredicateValue),
    Lt(String, retry_api::PredicateValue),
    Lte(String, retry_api::PredicateValue),
    Exists(String),
    In(String, Vec<retry_api::PredicateValue>),
    Matches(String, String),
    StartsWith(String, String),
    Contains(String, String),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Not(Box<Self>),
    True,
    False,
}

#[derive(Clone, Copy, Debug)]
struct PredicateEvaluationError;

impl CompiledPredicate {
    fn matches(
        &self,
        properties: &[(String, retry_api::PredicateValue)],
    ) -> Result<bool, PredicateEvaluationError> {
        use CompiledPredicate::*;
        match self {
            Eq(name, expected) => {
                Ok(compare(required(properties, name)?, expected)? == Ordering::Equal)
            }
            Neq(name, expected) => {
                Ok(compare(required(properties, name)?, expected)? != Ordering::Equal)
            }
            Gt(name, expected) => {
                Ok(compare(required(properties, name)?, expected)? == Ordering::Greater)
            }
            Gte(name, expected) => {
                Ok(compare(required(properties, name)?, expected)? != Ordering::Less)
            }
            Lt(name, expected) => {
                Ok(compare(required(properties, name)?, expected)? == Ordering::Less)
            }
            Lte(name, expected) => {
                Ok(compare(required(properties, name)?, expected)? != Ordering::Greater)
            }
            Exists(name) => Ok(get_property(properties, name).is_some()),
            In(name, expected) => {
                let actual = required(properties, name)?;
                let mut had_error = false;
                for candidate in expected {
                    match compare(actual, candidate) {
                        Ok(Ordering::Equal) => return Ok(true),
                        Ok(_) => {}
                        Err(_) => had_error = true,
                    }
                }
                if had_error {
                    Err(PredicateEvaluationError)
                } else {
                    Ok(false)
                }
            }
            Matches(name, pattern) => Ok(glob_match::glob_match(
                pattern,
                &as_text(required(properties, name)?)?,
            )),
            StartsWith(name, prefix) => {
                Ok(as_text(required(properties, name)?)?.starts_with(prefix))
            }
            Contains(name, substring) => {
                Ok(as_text(required(properties, name)?)?.contains(substring))
            }
            And(left, right) => Ok(left.matches(properties)? && right.matches(properties)?),
            Or(left, right) => Ok(left.matches(properties)? || right.matches(properties)?),
            Not(inner) => Ok(!inner.matches(properties)?),
            True => Ok(true),
            False => Ok(false),
        }
    }
}

struct Compiler {
    compiled_nodes: usize,
    compiled_payload_bytes: usize,
    policy_visiting: Vec<bool>,
}

impl Compiler {
    fn new() -> Self {
        Self {
            compiled_nodes: 0,
            compiled_payload_bytes: 0,
            policy_visiting: Vec::new(),
        }
    }

    fn enter(&mut self, depth: usize, payload_bytes: usize) -> Result<(), RetryPolicyError> {
        self.compiled_nodes = self.compiled_nodes.saturating_add(1);
        self.compiled_payload_bytes = self.compiled_payload_bytes.saturating_add(payload_bytes);
        if depth > MAX_POLICY_DEPTH
            || self.compiled_nodes > MAX_COMPILED_NODES
            || self.compiled_payload_bytes > MAX_COMPILED_PAYLOAD_BYTES
        {
            Err(RetryPolicyError::PolicyTooComplex)
        } else {
            Ok(())
        }
    }

    fn compile_policy_node(
        &mut self,
        nodes: &[retry_api::PolicyNode],
        index: i32,
        depth: usize,
    ) -> Result<CompiledPolicy, RetryPolicyError> {
        self.enter(depth, 0)?;
        if self.policy_visiting.len() != nodes.len() {
            self.policy_visiting = vec![false; nodes.len()];
        }
        let position = usize::try_from(index)
            .ok()
            .filter(|position| *position < nodes.len())
            .ok_or(RetryPolicyError::InvalidPolicyNodeIndex(index))?;
        if self.policy_visiting[position] {
            return Err(RetryPolicyError::CyclicPolicyNode(index));
        }
        self.policy_visiting[position] = true;
        let next_depth = depth + 1;
        let result = match &nodes[position] {
            retry_api::PolicyNode::Periodic(delay) => {
                CompiledPolicy::Periodic(Duration::from_nanos(*delay))
            }
            retry_api::PolicyNode::Exponential(config) => {
                if !config.factor.is_finite() || config.factor <= 0.0 {
                    return Err(RetryPolicyError::InvalidExponentialFactor(config.factor));
                }
                CompiledPolicy::Exponential {
                    base_delay: Duration::from_nanos(config.base_delay),
                    factor: config.factor,
                    attempt: 0,
                }
            }
            retry_api::PolicyNode::Fibonacci(config) => CompiledPolicy::Fibonacci {
                previous: Duration::from_nanos(config.first),
                current: Duration::from_nanos(config.second),
                attempt: 0,
            },
            retry_api::PolicyNode::Immediate => CompiledPolicy::Immediate,
            retry_api::PolicyNode::Never => CompiledPolicy::Never,
            retry_api::PolicyNode::CountBox(config) => CompiledPolicy::CountBox {
                max_retries: config.max_retries,
                attempts: 0,
                inner: Box::new(self.compile_policy_node(nodes, config.inner, next_depth)?),
            },
            retry_api::PolicyNode::TimeBox(config) => CompiledPolicy::TimeBox {
                limit: Duration::from_nanos(config.limit),
                inner: Box::new(self.compile_policy_node(nodes, config.inner, next_depth)?),
            },
            retry_api::PolicyNode::ClampDelay(config) => {
                let min_delay = Duration::from_nanos(config.min_delay);
                let max_delay = Duration::from_nanos(config.max_delay);
                if min_delay > max_delay {
                    return Err(RetryPolicyError::InvalidClampRange {
                        min_delay,
                        max_delay,
                    });
                }
                CompiledPolicy::Clamp {
                    min_delay,
                    max_delay,
                    inner: Box::new(self.compile_policy_node(nodes, config.inner, next_depth)?),
                }
            }
            retry_api::PolicyNode::AddDelay(config) => CompiledPolicy::AddDelay {
                delay: Duration::from_nanos(config.delay),
                inner: Box::new(self.compile_policy_node(nodes, config.inner, next_depth)?),
            },
            retry_api::PolicyNode::Jitter(config) => {
                if !config.factor.is_finite() || config.factor < 0.0 {
                    return Err(RetryPolicyError::InvalidJitterFactor(config.factor));
                }
                CompiledPolicy::Jitter {
                    factor: config.factor,
                    inner: Box::new(self.compile_policy_node(nodes, config.inner, next_depth)?),
                }
            }
            retry_api::PolicyNode::FilteredOn(config) => CompiledPolicy::FilteredOn {
                predicate: self.compile_predicate(&config.predicate, next_depth)?,
                inner: Box::new(self.compile_policy_node(nodes, config.inner, next_depth)?),
            },
            retry_api::PolicyNode::AndThen((left, right)) => CompiledPolicy::AndThen {
                left: Box::new(self.compile_policy_node(nodes, *left, next_depth)?),
                right: Box::new(self.compile_policy_node(nodes, *right, next_depth)?),
                on_right: false,
            },
            retry_api::PolicyNode::PolicyUnion((left, right)) => CompiledPolicy::Union(
                Box::new(self.compile_policy_node(nodes, *left, next_depth)?),
                Box::new(self.compile_policy_node(nodes, *right, next_depth)?),
            ),
            retry_api::PolicyNode::PolicyIntersect((left, right)) => CompiledPolicy::Intersect(
                Box::new(self.compile_policy_node(nodes, *left, next_depth)?),
                Box::new(self.compile_policy_node(nodes, *right, next_depth)?),
            ),
        };
        self.policy_visiting[position] = false;
        Ok(result)
    }

    fn compile_predicate(
        &mut self,
        predicate: &retry_api::RetryPredicate,
        depth: usize,
    ) -> Result<CompiledPredicate, RetryPolicyError> {
        if predicate.nodes.is_empty() {
            return Err(RetryPolicyError::EmptyPredicate);
        }
        if predicate.nodes.len() > MAX_COMPILED_NODES {
            return Err(RetryPolicyError::PolicyTooComplex);
        }
        self.compile_predicate_node(
            &predicate.nodes,
            0,
            depth,
            &mut vec![false; predicate.nodes.len()],
        )
    }

    fn compile_predicate_node(
        &mut self,
        nodes: &[retry_api::PredicateNode],
        index: i32,
        depth: usize,
        visiting: &mut [bool],
    ) -> Result<CompiledPredicate, RetryPolicyError> {
        use retry_api::PredicateNode as Raw;
        let position = usize::try_from(index)
            .ok()
            .filter(|position| *position < nodes.len())
            .ok_or(RetryPolicyError::InvalidPredicateNodeIndex(index))?;
        self.enter(depth, predicate_node_payload_bytes(&nodes[position]))?;
        if visiting[position] {
            return Err(RetryPolicyError::CyclicPredicateNode(index));
        }
        visiting[position] = true;
        let next_depth = depth + 1;
        let comparison = |comparison: &retry_api::PropertyComparison| {
            (comparison.property_name.clone(), comparison.value.clone())
        };
        let result = match &nodes[position] {
            Raw::PropEq(value) => {
                let (name, value) = comparison(value);
                CompiledPredicate::Eq(name, value)
            }
            Raw::PropNeq(value) => {
                let (name, value) = comparison(value);
                CompiledPredicate::Neq(name, value)
            }
            Raw::PropGt(value) => {
                let (name, value) = comparison(value);
                CompiledPredicate::Gt(name, value)
            }
            Raw::PropGte(value) => {
                let (name, value) = comparison(value);
                CompiledPredicate::Gte(name, value)
            }
            Raw::PropLt(value) => {
                let (name, value) = comparison(value);
                CompiledPredicate::Lt(name, value)
            }
            Raw::PropLte(value) => {
                let (name, value) = comparison(value);
                CompiledPredicate::Lte(name, value)
            }
            Raw::PropExists(name) => CompiledPredicate::Exists(name.clone()),
            Raw::PropIn(check) => {
                CompiledPredicate::In(check.property_name.clone(), check.values.clone())
            }
            Raw::PropMatches(pattern) => {
                CompiledPredicate::Matches(pattern.property_name.clone(), pattern.pattern.clone())
            }
            Raw::PropStartsWith(pattern) => CompiledPredicate::StartsWith(
                pattern.property_name.clone(),
                pattern.pattern.clone(),
            ),
            Raw::PropContains(pattern) => {
                CompiledPredicate::Contains(pattern.property_name.clone(), pattern.pattern.clone())
            }
            Raw::PredAnd((left, right)) => CompiledPredicate::And(
                Box::new(self.compile_predicate_node(nodes, *left, next_depth, visiting)?),
                Box::new(self.compile_predicate_node(nodes, *right, next_depth, visiting)?),
            ),
            Raw::PredOr((left, right)) => CompiledPredicate::Or(
                Box::new(self.compile_predicate_node(nodes, *left, next_depth, visiting)?),
                Box::new(self.compile_predicate_node(nodes, *right, next_depth, visiting)?),
            ),
            Raw::PredNot(inner) => CompiledPredicate::Not(Box::new(
                self.compile_predicate_node(nodes, *inner, next_depth, visiting)?,
            )),
            Raw::PredTrue => CompiledPredicate::True,
            Raw::PredFalse => CompiledPredicate::False,
        };
        visiting[position] = false;
        Ok(result)
    }
}

fn predicate_node_payload_bytes(node: &retry_api::PredicateNode) -> usize {
    fn value_bytes(value: &retry_api::PredicateValue) -> usize {
        match value {
            retry_api::PredicateValue::Text(value) => value.len(),
            retry_api::PredicateValue::Integer(_) | retry_api::PredicateValue::Boolean(_) => 0,
        }
    }

    use retry_api::PredicateNode::*;
    match node {
        PropEq(value) | PropNeq(value) | PropGt(value) | PropGte(value) | PropLt(value)
        | PropLte(value) => value
            .property_name
            .len()
            .saturating_add(value_bytes(&value.value)),
        PropExists(name) => name.len(),
        PropIn(check) => {
            let base = check.property_name.len().saturating_add(
                check
                    .values
                    .len()
                    .saturating_mul(std::mem::size_of::<retry_api::PredicateValue>()),
            );
            if base > MAX_COMPILED_PAYLOAD_BYTES {
                usize::MAX
            } else {
                check.values.iter().fold(base, |total, value| {
                    total.saturating_add(value_bytes(value))
                })
            }
        }
        PropMatches(pattern) | PropStartsWith(pattern) | PropContains(pattern) => pattern
            .property_name
            .len()
            .saturating_add(pattern.pattern.len()),
        PredAnd(_) | PredOr(_) | PredNot(_) | PredTrue | PredFalse => 0,
    }
}

fn get_property<'a>(
    properties: &'a [(String, retry_api::PredicateValue)],
    name: &str,
) -> Option<&'a retry_api::PredicateValue> {
    properties
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn required<'a>(
    properties: &'a [(String, retry_api::PredicateValue)],
    name: &str,
) -> Result<&'a retry_api::PredicateValue, PredicateEvaluationError> {
    get_property(properties, name).ok_or(PredicateEvaluationError)
}

fn compare(
    actual: &retry_api::PredicateValue,
    expected: &retry_api::PredicateValue,
) -> Result<Ordering, PredicateEvaluationError> {
    use retry_api::PredicateValue::*;
    match (actual, expected) {
        (Integer(left), Integer(right)) => Ok(left.cmp(right)),
        (Text(left), Text(right)) => Ok(left.cmp(right)),
        (Boolean(left), Boolean(right)) => Ok(left.cmp(right)),
        (Text(left), Integer(right)) => left
            .parse::<i64>()
            .map(|left| left.cmp(right))
            .map_err(|_| PredicateEvaluationError),
        (Integer(left), Text(right)) => Ok(left.to_string().cmp(right)),
        _ => Err(PredicateEvaluationError),
    }
}

fn as_text(value: &retry_api::PredicateValue) -> Result<String, PredicateEvaluationError> {
    match value {
        retry_api::PredicateValue::Text(value) => Ok(value.clone()),
        retry_api::PredicateValue::Integer(value) => Ok(value.to_string()),
        retry_api::PredicateValue::Boolean(_) => Err(PredicateEvaluationError),
    }
}

fn scale_duration(duration: Duration, factor: f64) -> Duration {
    if !factor.is_finite() || factor <= 0.0 {
        return Duration::ZERO;
    }
    let value = duration.as_secs_f64() * factor;
    if !value.is_finite() {
        Duration::MAX
    } else {
        Duration::try_from_secs_f64(value).unwrap_or(Duration::MAX)
    }
}

fn saturating_add(left: Duration, right: Duration) -> Duration {
    left.checked_add(right).unwrap_or(Duration::MAX)
}

fn duration_to_nanos(duration: Duration) -> u64 {
    duration.as_nanos().min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests;
