///|
/// Error type raised when a high-level retry builder cannot be converted to the
/// raw flattened WIT representation.
pub suberror RetryBuilderError {
  RetryBuilderError(String)
}

///|
let uint64_max : UInt64 = 18446744073709551615UL

///|
let int64_max_as_uint64 : UInt64 = 9223372036854775807UL

///|
enum DurationRepr {
  Nanos(UInt64)
  Micros(UInt64)
  Millis(UInt64)
  Seconds(UInt64)
  Minutes(UInt64)
  Hours(UInt64)
} derive(Show, Eq)

///|
/// Ergonomic duration helper for retry-policy builders.
/// The raw WIT API expects `u64` nanoseconds; this wrapper keeps user code in
/// named units and converts only at the raw boundary.
pub struct Duration {
  repr : DurationRepr
} derive(Show, Eq)

///|
pub fn Duration::nanos(value : UInt64) -> Duration {
  { repr: DurationRepr::Nanos(value) }
}

///|
pub fn Duration::micros(value : UInt64) -> Duration {
  { repr: DurationRepr::Micros(value) }
}

///|
pub fn Duration::millis(value : UInt64) -> Duration {
  { repr: DurationRepr::Millis(value) }
}

///|
pub fn Duration::seconds(value : UInt64) -> Duration {
  { repr: DurationRepr::Seconds(value) }
}

///|
pub fn Duration::minutes(value : UInt64) -> Duration {
  { repr: DurationRepr::Minutes(value) }
}

///|
pub fn Duration::hours(value : UInt64) -> Duration {
  { repr: DurationRepr::Hours(value) }
}

///|
/// Converts this helper duration into raw nanoseconds.
pub fn Duration::to_nanos(self : Duration) -> UInt64 raise RetryBuilderError {
  match self.repr {
    DurationRepr::Nanos(value) => value
    DurationRepr::Micros(value) => checked_mul_uint64(value, 1000, "micros")
    DurationRepr::Millis(value) => checked_mul_uint64(value, 1000000, "millis")
    DurationRepr::Seconds(value) =>
      checked_mul_uint64(value, 1000000000, "seconds")
    DurationRepr::Minutes(value) =>
      checked_mul_uint64(value, 60000000000UL, "minutes")
    DurationRepr::Hours(value) =>
      checked_mul_uint64(value, 3600000000000UL, "hours")
  }
}

///|
pub struct Props {
  key : String
} derive(Show, Eq)

///|
pub fn Props::custom(name : String) -> Props {
  { key: name }
}

///|
pub fn Props::verb() -> Props {
  Props::custom("verb")
}

///|
pub fn Props::noun_uri() -> Props {
  Props::custom("noun-uri")
}

///|
pub fn Props::uri_scheme() -> Props {
  Props::custom("uri-scheme")
}

///|
pub fn Props::uri_host() -> Props {
  Props::custom("uri-host")
}

///|
pub fn Props::uri_port() -> Props {
  Props::custom("uri-port")
}

///|
pub fn Props::uri_path() -> Props {
  Props::custom("uri-path")
}

///|
pub fn Props::status_code() -> Props {
  Props::custom("status-code")
}

///|
pub fn Props::error_type() -> Props {
  Props::custom("error-type")
}

///|
pub fn Props::function() -> Props {
  Props::custom("function")
}

///|
pub fn Props::target_component_id() -> Props {
  Props::custom("target-component-id")
}

///|
pub fn Props::target_agent_type() -> Props {
  Props::custom("target-agent-type")
}

///|
pub fn Props::db_type() -> Props {
  Props::custom("db-type")
}

///|
pub fn Props::trap_type() -> Props {
  Props::custom("trap-type")
}

///|
pub fn Props::key(self : Props) -> String {
  self.key
}

///|
enum ValueRepr {
  Text(String)
  Integer(Int64)
  Boolean(Bool)
  Int(Int)
  UInt(UInt)
  UInt64(UInt64)
} derive(Show, Eq)

///|
/// Explicit predicate-value helper for building retry predicates without using
/// raw WIT enums directly.
pub struct Value {
  repr : ValueRepr
} derive(Show, Eq)

///|
pub fn Value::text(value : String) -> Value {
  { repr: ValueRepr::Text(value) }
}

///|
pub fn Value::integer(value : Int64) -> Value {
  { repr: ValueRepr::Integer(value) }
}

///|
pub fn Value::boolean(value : Bool) -> Value {
  { repr: ValueRepr::Boolean(value) }
}

///|
pub fn Value::int(value : Int) -> Value {
  { repr: ValueRepr::Int(value) }
}

///|
pub fn Value::uint(value : UInt) -> Value {
  { repr: ValueRepr::UInt(value) }
}

///|
pub fn Value::uint64(value : UInt64) -> Value {
  { repr: ValueRepr::UInt64(value) }
}

///|
/// Converts this helper value into the raw retry predicate value.
pub fn Value::to_raw(self : Value) -> PredicateValue raise RetryBuilderError {
  match self.repr {
    ValueRepr::Text(value) => PredicateValue::Text(value)
    ValueRepr::Integer(value) => PredicateValue::Integer(value)
    ValueRepr::Boolean(value) => PredicateValue::Boolean(value)
    ValueRepr::Int(value) => PredicateValue::Integer(Int64::from_int(value))
    ValueRepr::UInt(value) => PredicateValue::Integer(value.to_int64())
    ValueRepr::UInt64(value) => {
      if value > int64_max_as_uint64 {
        raise RetryBuilderError(
          "Predicate integer value exceeds s64 range: \{value.to_string()}",
        )
      }
      PredicateValue::Integer(value.reinterpret_as_int64())
    }
  }
}

///|
enum PredicateRepr {
  Always
  Never
  Eq(property~ : Props, value~ : Value)
  Neq(property~ : Props, value~ : Value)
  Gt(property~ : Props, value~ : Value)
  Gte(property~ : Props, value~ : Value)
  Lt(property~ : Props, value~ : Value)
  Lte(property~ : Props, value~ : Value)
  Exists(Props)
  OneOf(property~ : Props, values~ : Array[Value])
  MatchesGlob(property~ : Props, pattern~ : String)
  StartsWith(property~ : Props, prefix~ : String)
  Contains(property~ : Props, substring~ : String)
  And(left~ : Predicate, right~ : Predicate)
  Or(left~ : Predicate, right~ : Predicate)
  Not(Predicate)
} derive(Show, Eq)

///|
/// Tree-shaped retry predicate builder.
pub struct Predicate {
  repr : PredicateRepr
} derive(Show, Eq)

///|
pub fn Predicate::always() -> Predicate {
  { repr: PredicateRepr::Always }
}

///|
pub fn Predicate::never() -> Predicate {
  { repr: PredicateRepr::Never }
}

///|
pub fn Predicate::eq(property : Props, value : Value) -> Predicate {
  { repr: PredicateRepr::Eq(property~, value~) }
}

///|
pub fn Predicate::neq(property : Props, value : Value) -> Predicate {
  { repr: PredicateRepr::Neq(property~, value~) }
}

///|
pub fn Predicate::gt(property : Props, value : Value) -> Predicate {
  { repr: PredicateRepr::Gt(property~, value~) }
}

///|
pub fn Predicate::gte(property : Props, value : Value) -> Predicate {
  { repr: PredicateRepr::Gte(property~, value~) }
}

///|
pub fn Predicate::lt(property : Props, value : Value) -> Predicate {
  { repr: PredicateRepr::Lt(property~, value~) }
}

///|
pub fn Predicate::lte(property : Props, value : Value) -> Predicate {
  { repr: PredicateRepr::Lte(property~, value~) }
}

///|
pub fn Predicate::exists(property : Props) -> Predicate {
  { repr: PredicateRepr::Exists(property) }
}

///|
pub fn Predicate::one_of(property : Props, values : Array[Value]) -> Predicate {
  { repr: PredicateRepr::OneOf(property~, values~) }
}

///|
pub fn Predicate::matches_glob(property : Props, pattern : String) -> Predicate {
  { repr: PredicateRepr::MatchesGlob(property~, pattern~) }
}

///|
pub fn Predicate::starts_with(property : Props, prefix : String) -> Predicate {
  { repr: PredicateRepr::StartsWith(property~, prefix~) }
}

///|
pub fn Predicate::contains(property : Props, substring : String) -> Predicate {
  { repr: PredicateRepr::Contains(property~, substring~) }
}

///|
pub fn Predicate::and_(left : Predicate, right : Predicate) -> Predicate {
  { repr: PredicateRepr::And(left~, right~) }
}

///|
pub fn Predicate::or_(left : Predicate, right : Predicate) -> Predicate {
  { repr: PredicateRepr::Or(left~, right~) }
}

///|
pub fn Predicate::not_(inner : Predicate) -> Predicate {
  { repr: PredicateRepr::Not(inner) }
}

///|
/// Compiles this tree-shaped predicate into the flattened raw retry WIT type.
pub fn Predicate::to_raw(
  self : Predicate,
) -> @retry.RetryPredicate raise RetryBuilderError {
  let nodes : Array[@retry.PredicateNode] = []
  let _ = push_predicate_node(self, nodes)
  { nodes, }
}

///|
enum PolicyRepr {
  Immediate
  Never
  Periodic(Duration)
  Exponential(base_delay~ : Duration, factor~ : Double)
  Fibonacci(first~ : Duration, second~ : Duration)
  CountBox(max_retries~ : UInt, inner~ : Policy)
  TimeBox(limit~ : Duration, inner~ : Policy)
  Clamp(min_delay~ : Duration, max_delay~ : Duration, inner~ : Policy)
  AddDelay(delay~ : Duration, inner~ : Policy)
  Jitter(factor~ : Double, inner~ : Policy)
  FilteredOn(predicate~ : Predicate, inner~ : Policy)
  AndThen(left~ : Policy, right~ : Policy)
  Union(left~ : Policy, right~ : Policy)
  Intersect(left~ : Policy, right~ : Policy)
} derive(Show, Eq)

///|
/// Tree-shaped retry policy builder.
pub struct Policy {
  repr : PolicyRepr
} derive(Show, Eq)

///|
pub fn Policy::immediate() -> Policy {
  { repr: PolicyRepr::Immediate }
}

///|
pub fn Policy::never() -> Policy {
  { repr: PolicyRepr::Never }
}

///|
pub fn Policy::periodic(delay : Duration) -> Policy {
  { repr: PolicyRepr::Periodic(delay) }
}

///|
pub fn Policy::exponential(base_delay : Duration, factor : Double) -> Policy {
  { repr: PolicyRepr::Exponential(base_delay~, factor~) }
}

///|
pub fn Policy::fibonacci(first : Duration, second : Duration) -> Policy {
  { repr: PolicyRepr::Fibonacci(first~, second~) }
}

///|
pub fn Policy::max_retries(self : Policy, max_retries : UInt) -> Policy {
  { repr: PolicyRepr::CountBox(max_retries~, inner=self) }
}

///|
pub fn Policy::within(self : Policy, limit : Duration) -> Policy {
  { repr: PolicyRepr::TimeBox(limit~, inner=self) }
}

///|
pub fn Policy::clamp(
  self : Policy,
  min_delay : Duration,
  max_delay : Duration,
) -> Policy {
  { repr: PolicyRepr::Clamp(min_delay~, max_delay~, inner=self) }
}

///|
pub fn Policy::add_delay(self : Policy, delay : Duration) -> Policy {
  { repr: PolicyRepr::AddDelay(delay~, inner=self) }
}

///|
pub fn Policy::with_jitter(self : Policy, factor : Double) -> Policy {
  { repr: PolicyRepr::Jitter(factor~, inner=self) }
}

///|
pub fn Policy::only_when(self : Policy, predicate : Predicate) -> Policy {
  { repr: PolicyRepr::FilteredOn(predicate~, inner=self) }
}

///|
pub fn Policy::and_then(self : Policy, other : Policy) -> Policy {
  { repr: PolicyRepr::AndThen(left=self, right=other) }
}

///|
pub fn Policy::union(self : Policy, other : Policy) -> Policy {
  { repr: PolicyRepr::Union(left=self, right=other) }
}

///|
pub fn Policy::intersect(self : Policy, other : Policy) -> Policy {
  { repr: PolicyRepr::Intersect(left=self, right=other) }
}

///|
/// Compiles this tree-shaped policy into the flattened raw retry WIT type.
pub fn Policy::to_raw(
  self : Policy,
) -> @retry.RetryPolicy raise RetryBuilderError {
  let nodes : Array[@retry.PolicyNode] = []
  let _ = push_policy_node(self, nodes)
  { nodes, }
}

///|
/// A named retry rule with default priority 0 and a default predicate of
/// `Predicate::always()`.
pub struct NamedPolicy {
  name : String
  priority : UInt
  predicate : Predicate
  policy : Policy
} derive(Show, Eq)

///|
pub fn NamedPolicy::named(name : String, policy : Policy) -> NamedPolicy {
  { name, priority: 0, predicate: Predicate::always(), policy }
}

///|
pub fn NamedPolicy::priority(
  self : NamedPolicy,
  priority : UInt,
) -> NamedPolicy {
  { ..self, priority, }
}

///|
pub fn NamedPolicy::applies_when(
  self : NamedPolicy,
  predicate : Predicate,
) -> NamedPolicy {
  { ..self, predicate, }
}

///|
/// Compiles this named retry rule into the raw host type.
pub fn NamedPolicy::to_raw(
  self : NamedPolicy,
) -> NamedRetryPolicy raise RetryBuilderError {
  {
    name: self.name,
    priority: self.priority,
    predicate: self.predicate.to_raw(),
    policy: self.policy.to_raw(),
  }
}

///|
fn checked_mul_uint64(
  value : UInt64,
  factor : UInt64,
  unit : String,
) -> UInt64 raise RetryBuilderError {
  if factor != 0 && value > uint64_max / factor {
    raise RetryBuilderError(
      "Duration \{unit}(\{value.to_string()}) exceeds u64 nanoseconds",
    )
  }
  value * factor
}

///|
fn is_finite(value : Double) -> Bool {
  !value.is_nan() && !value.is_inf()
}

///|
fn make_property_comparison(
  property : Props,
  value : Value,
) -> @retry.PropertyComparison raise RetryBuilderError {
  { property_name: property.key(), value: value.to_raw() }
}

///|
fn make_property_pattern(
  property : Props,
  pattern : String,
) -> @retry.PropertyPattern {
  { property_name: property.key(), pattern }
}

///|
fn push_predicate_node(
  predicate : Predicate,
  nodes : Array[@retry.PredicateNode],
) -> Int raise RetryBuilderError {
  let index = nodes.length()
  nodes.push(@retry.PredicateNode::PredFalse)

  let node = match predicate.repr {
    PredicateRepr::Always => @retry.PredicateNode::PredTrue
    PredicateRepr::Never => @retry.PredicateNode::PredFalse
    PredicateRepr::Eq(property~, value~) =>
      @retry.PredicateNode::PropEq(make_property_comparison(property, value))
    PredicateRepr::Neq(property~, value~) =>
      @retry.PredicateNode::PropNeq(make_property_comparison(property, value))
    PredicateRepr::Gt(property~, value~) =>
      @retry.PredicateNode::PropGt(make_property_comparison(property, value))
    PredicateRepr::Gte(property~, value~) =>
      @retry.PredicateNode::PropGte(make_property_comparison(property, value))
    PredicateRepr::Lt(property~, value~) =>
      @retry.PredicateNode::PropLt(make_property_comparison(property, value))
    PredicateRepr::Lte(property~, value~) =>
      @retry.PredicateNode::PropLte(make_property_comparison(property, value))
    PredicateRepr::Exists(property) =>
      @retry.PredicateNode::PropExists(property.key())
    PredicateRepr::OneOf(property~, values~) => {
      let raw_values = values.map(value => value.to_raw())
      @retry.PredicateNode::PropIn({
        property_name: property.key(),
        values: raw_values,
      })
    }
    PredicateRepr::MatchesGlob(property~, pattern~) =>
      @retry.PredicateNode::PropMatches(
        make_property_pattern(property, pattern),
      )
    PredicateRepr::StartsWith(property~, prefix~) =>
      @retry.PredicateNode::PropStartsWith(
        make_property_pattern(property, prefix),
      )
    PredicateRepr::Contains(property~, substring~) =>
      @retry.PredicateNode::PropContains(
        make_property_pattern(property, substring),
      )
    PredicateRepr::And(left~, right~) => {
      let left_idx = push_predicate_node(left, nodes)
      let right_idx = push_predicate_node(right, nodes)
      @retry.PredicateNode::PredAnd((left_idx, right_idx))
    }
    PredicateRepr::Or(left~, right~) => {
      let left_idx = push_predicate_node(left, nodes)
      let right_idx = push_predicate_node(right, nodes)
      @retry.PredicateNode::PredOr((left_idx, right_idx))
    }
    PredicateRepr::Not(inner) => {
      let inner_idx = push_predicate_node(inner, nodes)
      @retry.PredicateNode::PredNot(inner_idx)
    }
  }

  nodes[index] = node
  index
}

///|
fn push_policy_node(
  policy : Policy,
  nodes : Array[@retry.PolicyNode],
) -> Int raise RetryBuilderError {
  let index = nodes.length()
  nodes.push(@retry.PolicyNode::Never)

  let node = match policy.repr {
    PolicyRepr::Immediate => @retry.PolicyNode::Immediate
    PolicyRepr::Never => @retry.PolicyNode::Never
    PolicyRepr::Periodic(delay) => @retry.PolicyNode::Periodic(delay.to_nanos())
    PolicyRepr::Exponential(base_delay~, factor~) => {
      if !is_finite(factor) || factor <= 0.0 {
        raise RetryBuilderError(
          "Exponential factor must be finite and greater than 0",
        )
      }
      @retry.PolicyNode::Exponential({
        base_delay: base_delay.to_nanos(),
        factor,
      })
    }
    PolicyRepr::Fibonacci(first~, second~) =>
      @retry.PolicyNode::Fibonacci({
        first: first.to_nanos(),
        second: second.to_nanos(),
      })
    PolicyRepr::CountBox(max_retries~, inner~) => {
      let inner_idx = push_policy_node(inner, nodes)
      @retry.PolicyNode::CountBox({ max_retries, inner: inner_idx })
    }
    PolicyRepr::TimeBox(limit~, inner~) => {
      let inner_idx = push_policy_node(inner, nodes)
      @retry.PolicyNode::TimeBox({ limit: limit.to_nanos(), inner: inner_idx })
    }
    PolicyRepr::Clamp(min_delay~, max_delay~, inner~) => {
      let min_nanos = min_delay.to_nanos()
      let max_nanos = max_delay.to_nanos()
      if min_nanos > max_nanos {
        raise RetryBuilderError(
          "Clamp min_delay must be less than or equal to max_delay",
        )
      }
      let inner_idx = push_policy_node(inner, nodes)
      @retry.PolicyNode::ClampDelay({
        min_delay: min_nanos,
        max_delay: max_nanos,
        inner: inner_idx,
      })
    }
    PolicyRepr::AddDelay(delay~, inner~) => {
      let inner_idx = push_policy_node(inner, nodes)
      @retry.PolicyNode::AddDelay({ delay: delay.to_nanos(), inner: inner_idx })
    }
    PolicyRepr::Jitter(factor~, inner~) => {
      if !is_finite(factor) || factor < 0.0 {
        raise RetryBuilderError(
          "Jitter factor must be finite and greater than or equal to 0",
        )
      }
      let inner_idx = push_policy_node(inner, nodes)
      @retry.PolicyNode::Jitter({ factor, inner: inner_idx })
    }
    PolicyRepr::FilteredOn(predicate~, inner~) => {
      let inner_idx = push_policy_node(inner, nodes)
      @retry.PolicyNode::FilteredOn({
        predicate: predicate.to_raw(),
        inner: inner_idx,
      })
    }
    PolicyRepr::AndThen(left~, right~) => {
      let left_idx = push_policy_node(left, nodes)
      let right_idx = push_policy_node(right, nodes)
      @retry.PolicyNode::AndThen((left_idx, right_idx))
    }
    PolicyRepr::Union(left~, right~) => {
      let left_idx = push_policy_node(left, nodes)
      let right_idx = push_policy_node(right, nodes)
      @retry.PolicyNode::PolicyUnion((left_idx, right_idx))
    }
    PolicyRepr::Intersect(left~, right~) => {
      let left_idx = push_policy_node(left, nodes)
      let right_idx = push_policy_node(right, nodes)
      @retry.PolicyNode::PolicyIntersect((left_idx, right_idx))
    }
  }

  nodes[index] = node
  index
}
