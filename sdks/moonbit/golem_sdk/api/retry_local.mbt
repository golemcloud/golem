///|
/// Errors raised while interpreting a raw semantic retry policy locally.
pub suberror LocalRetryError {
  InvalidPolicy(String)
  PredicateEvaluation(String)
} derive(Debug, Eq)

///|
let max_compiled_nodes : Int = 4096

///|
let max_compiled_payload_bytes : Int = 1048576

///|
let max_policy_depth : Int = 256

///|
let predicate_value_payload_bytes : Int = 16

///|
priv enum LocalPredicate {
  Eq(String, PredicateValue)
  Neq(String, PredicateValue)
  Gt(String, PredicateValue)
  Gte(String, PredicateValue)
  Lt(String, PredicateValue)
  Lte(String, PredicateValue)
  Exists(String)
  In(String, Array[PredicateValue])
  Matches(String, String)
  StartsWith(String, String)
  Contains(String, String)
  And(LocalPredicate, LocalPredicate)
  Or(LocalPredicate, LocalPredicate)
  Not(LocalPredicate)
  True
  False
}

///|
priv enum LocalPolicy {
  Periodic(UInt64)
  Exponential(UInt64, Double)
  Fibonacci(UInt64, UInt64)
  Immediate
  Never
  CountBox(UInt, LocalPolicy)
  TimeBox(UInt64, LocalPolicy)
  Clamp(UInt64, UInt64, LocalPolicy)
  AddDelay(UInt64, LocalPolicy)
  Jitter(Double, LocalPolicy)
  Filtered(LocalPredicate, LocalPolicy)
  AndThen(LocalPolicy, LocalPolicy)
  Union(LocalPolicy, LocalPolicy)
  Intersect(LocalPolicy, LocalPolicy)
}

///|
priv enum LocalPolicyState {
  Counter(UInt)
  Terminal
  Wrapper(LocalPolicyState)
  Counted(UInt, LocalPolicyState)
  Sequenced(LocalPolicyState, LocalPolicyState, Bool)
  Pair(LocalPolicyState, LocalPolicyState)
}

///|
priv enum LocalVerdict {
  Retry(UInt64)
  GiveUp
}

///|
fn invalid_policy(message : String) -> LocalRetryError {
  LocalRetryError::InvalidPolicy(message)
}

///|
priv struct LocalPolicyCompiler {
  mut compiled_nodes : Int
  mut compiled_payload_bytes : Int
  policy_visiting : Array[Bool]
}

///|
fn policy_too_complex() -> LocalRetryError {
  invalid_policy("Retry policy is too complex")
}

///|
fn LocalPolicyCompiler::enter(
  self : LocalPolicyCompiler,
  depth : Int,
) -> Unit raise LocalRetryError {
  self.compiled_nodes += 1
  if depth > max_policy_depth || self.compiled_nodes > max_compiled_nodes {
    raise policy_too_complex()
  }
}

///|
fn LocalPolicyCompiler::add_payload(
  self : LocalPolicyCompiler,
  payload_bytes : Int,
) -> Unit raise LocalRetryError {
  self.compiled_payload_bytes = capped_payload_add(
    self.compiled_payload_bytes,
    payload_bytes,
  )
  if self.compiled_payload_bytes > max_compiled_payload_bytes {
    raise policy_too_complex()
  }
}

///|
fn capped_payload_add(left : Int, right : Int) -> Int {
  if right > max_compiled_payload_bytes ||
    left > max_compiled_payload_bytes - right {
    max_compiled_payload_bytes + 1
  } else {
    left + right
  }
}

///|
fn utf8_length_up_to(value : String, limit : Int) -> Int {
  let mut size = 0
  for char in value {
    let codepoint = char.to_int()
    let width = if codepoint <= 0x7f {
      1
    } else if codepoint <= 0x7ff {
      2
    } else if codepoint <= 0xffff {
      3
    } else {
      4
    }
    if size > limit - width {
      return limit + 1
    }
    size += width
  }
  size
}

///|
fn add_string_payload(size : Int, value : String) -> Int {
  if size > max_compiled_payload_bytes {
    size
  } else {
    capped_payload_add(
      size,
      utf8_length_up_to(value, max_compiled_payload_bytes - size),
    )
  }
}

///|
fn add_predicate_value_payload(size : Int, value : PredicateValue) -> Int {
  match value {
    PredicateValue::Text(value) => add_string_payload(size, value)
    PredicateValue::Integer(_) | PredicateValue::Boolean(_) => size
  }
}

///|
fn comparison_payload(comparison : @retry.PropertyComparison) -> Int {
  add_predicate_value_payload(
    add_string_payload(0, comparison.property_name),
    comparison.value,
  )
}

///|
fn pattern_payload(pattern : @retry.PropertyPattern) -> Int {
  add_string_payload(
    add_string_payload(0, pattern.property_name),
    pattern.pattern,
  )
}

///|
fn predicate_node_payload(node : @retry.PredicateNode) -> Int {
  match node {
    @retry.PredicateNode::PropEq(value)
    | @retry.PredicateNode::PropNeq(value)
    | @retry.PredicateNode::PropGt(value)
    | @retry.PredicateNode::PropGte(value)
    | @retry.PredicateNode::PropLt(value)
    | @retry.PredicateNode::PropLte(value) => comparison_payload(value)
    @retry.PredicateNode::PropExists(name) => add_string_payload(0, name)
    @retry.PredicateNode::PropIn(check) => {
      if check.values.length() >
        max_compiled_payload_bytes / predicate_value_payload_bytes {
        return max_compiled_payload_bytes + 1
      }
      let mut size = add_string_payload(
        check.values.length() * predicate_value_payload_bytes,
        check.property_name,
      )
      for _, value in check.values {
        if size > max_compiled_payload_bytes {
          return size
        }
        size = add_predicate_value_payload(size, value)
      }
      size
    }
    @retry.PredicateNode::PropMatches(value)
    | @retry.PredicateNode::PropStartsWith(value)
    | @retry.PredicateNode::PropContains(value) => pattern_payload(value)
    @retry.PredicateNode::PredAnd(_)
    | @retry.PredicateNode::PredOr(_)
    | @retry.PredicateNode::PredNot(_)
    | @retry.PredicateNode::PredTrue
    | @retry.PredicateNode::PredFalse => 0
  }
}

///|
fn LocalPolicyCompiler::parse_predicate(
  self : LocalPolicyCompiler,
  raw : @retry.RetryPredicate,
  depth : Int,
) -> LocalPredicate raise LocalRetryError {
  if raw.nodes.is_empty() {
    raise invalid_policy("Retry predicate has no root node")
  }
  if raw.nodes.length() > max_compiled_nodes {
    raise policy_too_complex()
  }
  let visiting = Array::make(raw.nodes.length(), false)
  self.parse_predicate_node(raw.nodes, 0, visiting, depth)
}

///|
fn LocalPolicyCompiler::parse_predicate_node(
  self : LocalPolicyCompiler,
  nodes : Array[@retry.PredicateNode],
  index : Int,
  visiting : Array[Bool],
  depth : Int,
) -> LocalPredicate raise LocalRetryError {
  guard nodes.get(index) is Some(node) else {
    raise invalid_policy("Predicate node index out of bounds: \{index}")
  }
  self.enter(depth)
  self.add_payload(predicate_node_payload(node))
  if visiting[index] {
    raise invalid_policy("Cycle in retry predicate at node \{index}")
  }
  visiting[index] = true
  defer {
    visiting[index] = false
  }
  match node {
    @retry.PredicateNode::PropEq(v) =>
      LocalPredicate::Eq(v.property_name, v.value)
    @retry.PredicateNode::PropNeq(v) =>
      LocalPredicate::Neq(v.property_name, v.value)
    @retry.PredicateNode::PropGt(v) =>
      LocalPredicate::Gt(v.property_name, v.value)
    @retry.PredicateNode::PropGte(v) =>
      LocalPredicate::Gte(v.property_name, v.value)
    @retry.PredicateNode::PropLt(v) =>
      LocalPredicate::Lt(v.property_name, v.value)
    @retry.PredicateNode::PropLte(v) =>
      LocalPredicate::Lte(v.property_name, v.value)
    @retry.PredicateNode::PropExists(name) => LocalPredicate::Exists(name)
    @retry.PredicateNode::PropIn(v) =>
      LocalPredicate::In(v.property_name, v.values)
    @retry.PredicateNode::PropMatches(v) =>
      LocalPredicate::Matches(v.property_name, v.pattern)
    @retry.PredicateNode::PropStartsWith(v) =>
      LocalPredicate::StartsWith(v.property_name, v.pattern)
    @retry.PredicateNode::PropContains(v) =>
      LocalPredicate::Contains(v.property_name, v.pattern)
    @retry.PredicateNode::PredAnd((left, right)) =>
      LocalPredicate::And(
        self.parse_predicate_node(nodes, left, visiting, depth + 1),
        self.parse_predicate_node(nodes, right, visiting, depth + 1),
      )
    @retry.PredicateNode::PredOr((left, right)) =>
      LocalPredicate::Or(
        self.parse_predicate_node(nodes, left, visiting, depth + 1),
        self.parse_predicate_node(nodes, right, visiting, depth + 1),
      )
    @retry.PredicateNode::PredNot(inner) =>
      LocalPredicate::Not(
        self.parse_predicate_node(nodes, inner, visiting, depth + 1),
      )
    @retry.PredicateNode::PredTrue => LocalPredicate::True
    @retry.PredicateNode::PredFalse => LocalPredicate::False
  }
}

///|
fn parse_policy(raw : RetryPolicy) -> LocalPolicy raise LocalRetryError {
  if raw.nodes.is_empty() {
    raise invalid_policy("Retry policy has no root node")
  }
  if raw.nodes.length() > max_compiled_nodes {
    raise policy_too_complex()
  }
  let compiler : LocalPolicyCompiler = {
    compiled_nodes: 0,
    compiled_payload_bytes: 0,
    policy_visiting: Array::make(raw.nodes.length(), false),
  }
  compiler.parse_policy_node(raw.nodes, 0, 0)
}

///|
fn LocalPolicyCompiler::parse_policy_node(
  self : LocalPolicyCompiler,
  nodes : Array[@retry.PolicyNode],
  index : Int,
  depth : Int,
) -> LocalPolicy raise LocalRetryError {
  guard nodes.get(index) is Some(node) else {
    raise invalid_policy("Policy node index out of bounds: \{index}")
  }
  self.enter(depth)
  if self.policy_visiting[index] {
    raise invalid_policy("Cycle in retry policy at node \{index}")
  }
  self.policy_visiting[index] = true
  defer {
    self.policy_visiting[index] = false
  }
  match node {
    @retry.PolicyNode::Periodic(delay) => LocalPolicy::Periodic(delay)
    @retry.PolicyNode::Exponential(config) =>
      LocalPolicy::Exponential(config.base_delay, config.factor)
    @retry.PolicyNode::Fibonacci(config) =>
      LocalPolicy::Fibonacci(config.first, config.second)
    @retry.PolicyNode::Immediate => LocalPolicy::Immediate
    @retry.PolicyNode::Never => LocalPolicy::Never
    @retry.PolicyNode::CountBox(config) =>
      LocalPolicy::CountBox(
        config.max_retries,
        self.parse_policy_node(nodes, config.inner, depth + 1),
      )
    @retry.PolicyNode::TimeBox(config) =>
      LocalPolicy::TimeBox(
        config.limit,
        self.parse_policy_node(nodes, config.inner, depth + 1),
      )
    @retry.PolicyNode::ClampDelay(config) => {
      if config.min_delay > config.max_delay {
        raise invalid_policy("Clamp min_delay must not exceed max_delay")
      }
      LocalPolicy::Clamp(
        config.min_delay,
        config.max_delay,
        self.parse_policy_node(nodes, config.inner, depth + 1),
      )
    }
    @retry.PolicyNode::AddDelay(config) =>
      LocalPolicy::AddDelay(
        config.delay,
        self.parse_policy_node(nodes, config.inner, depth + 1),
      )
    @retry.PolicyNode::Jitter(config) =>
      LocalPolicy::Jitter(
        config.factor,
        self.parse_policy_node(nodes, config.inner, depth + 1),
      )
    @retry.PolicyNode::FilteredOn(config) =>
      LocalPolicy::Filtered(
        self.parse_predicate(config.predicate, depth + 1),
        self.parse_policy_node(nodes, config.inner, depth + 1),
      )
    @retry.PolicyNode::AndThen((left, right)) =>
      LocalPolicy::AndThen(
        self.parse_policy_node(nodes, left, depth + 1),
        self.parse_policy_node(nodes, right, depth + 1),
      )
    @retry.PolicyNode::PolicyUnion((left, right)) =>
      LocalPolicy::Union(
        self.parse_policy_node(nodes, left, depth + 1),
        self.parse_policy_node(nodes, right, depth + 1),
      )
    @retry.PolicyNode::PolicyIntersect((left, right)) =>
      LocalPolicy::Intersect(
        self.parse_policy_node(nodes, left, depth + 1),
        self.parse_policy_node(nodes, right, depth + 1),
      )
  }
}

///|
fn LocalPolicy::initial_state(self : LocalPolicy) -> LocalPolicyState {
  match self {
    Periodic(_) | Exponential(_, _) | Fibonacci(_, _) | Immediate => Counter(0)
    Never => Terminal
    CountBox(_, inner) => Counted(0, inner.initial_state())
    TimeBox(_, inner)
    | Clamp(_, _, inner)
    | AddDelay(_, inner)
    | Jitter(_, inner)
    | Filtered(_, inner) => Wrapper(inner.initial_state())
    AndThen(left, right) =>
      Sequenced(left.initial_state(), right.initial_state(), false)
    Union(left, right) | Intersect(left, right) =>
      Pair(left.initial_state(), right.initial_state())
  }
}

///|
fn saturating_add(left : UInt64, right : UInt64) -> UInt64 {
  if uint64_max - left < right {
    uint64_max
  } else {
    left + right
  }
}

///|
fn saturating_increment(value : UInt) -> UInt {
  if value == 4294967295U {
    value
  } else {
    value + 1
  }
}

///|
priv struct WideUInt {
  hi : UInt64
  lo : UInt64
}

///|
fn multiply_wide(left : UInt64, right : UInt64) -> WideUInt {
  let left_low = left & 0xffffffffUL
  let left_high = left >> 32
  let right_low = right & 0xffffffffUL
  let right_high = right >> 32
  let x = left_low * right_low
  let y = left_high * right_low + (x >> 32)
  let z = left_low * right_high + (y & 0xffffffffUL)
  { hi: left_high * right_high + (y >> 32) + (z >> 32), lo: left * right, }
}

///|
fn WideUInt::shift_left(self : WideUInt, amount : Int) -> WideUInt {
  if amount == 0 {
    self
  } else if amount < 64 {
    {
      hi: (self.hi << amount) | (self.lo >> (64 - amount)),
      lo: self.lo << amount,
    }
  } else {
    { hi: self.lo << (amount - 64), lo: 0, }
  }
}

///|
fn WideUInt::shift_right_low(self : WideUInt, amount : Int) -> UInt64 {
  if amount == 0 {
    self.lo
  } else if amount < 64 {
    (self.lo >> amount) | (self.hi << (64 - amount))
  } else {
    self.hi >> (amount - 64)
  }
}

///|
fn WideUInt::bit(self : WideUInt, index : Int) -> Bool {
  if index < 64 {
    ((self.lo >> index) & 1UL) == 1UL
  } else {
    ((self.hi >> (index - 64)) & 1UL) == 1UL
  }
}

///|
fn WideUInt::lower_bits_nonzero(self : WideUInt, count : Int) -> Bool {
  if count <= 0 {
    false
  } else if count < 64 {
    (self.lo & ((1UL << count) - 1UL)) != 0
  } else if count == 64 {
    self.lo != 0
  } else {
    self.lo != 0 || (self.hi & ((1UL << (count - 64)) - 1UL)) != 0
  }
}

///|
fn round_wide_shift(value : WideUInt, amount : Int) -> UInt64 {
  let truncated = value.shift_right_low(amount)
  let half = value.bit(amount - 1)
  let below_half = value.lower_bits_nonzero(amount - 1)
  if half && (below_half || truncated % 2UL == 1UL) {
    truncated + 1
  } else {
    truncated
  }
}

///|
fn seconds_float_to_nanos(value : Double) -> UInt64 {
  let bits = value.reinterpret_as_uint64()
  let mantissa_mask = 0x000fffffffffffffUL
  let mantissa = (bits & mantissa_mask) | (mantissa_mask + 1UL)
  let exponent = ((bits >> 52) & 0x7ffUL).to_int() - 1023
  let (seconds, nanos) = if exponent < -31 {
    (0UL, 0UL)
  } else if exponent < 0 {
    let shifted = multiply_wide(mantissa, 1000000000UL).shift_left(
      44 + exponent,
    )
    let nanos = round_wide_shift(shifted, 96)
    if nanos == 1000000000UL {
      (1UL, 0UL)
    } else {
      (0UL, nanos)
    }
  } else if exponent < 52 {
    let seconds = mantissa >> (52 - exponent)
    let fraction = (mantissa << exponent) & mantissa_mask
    let nanos = round_wide_shift(multiply_wide(fraction, 1000000000UL), 52)
    if nanos == 1000000000UL {
      (seconds + 1UL, 0UL)
    } else {
      (seconds, nanos)
    }
  } else if exponent < 64 {
    (mantissa << (exponent - 52), 0UL)
  } else {
    return uint64_max
  }
  if seconds > 18446744073UL ||
    (seconds == 18446744073UL && nanos > 709551615UL) {
    uint64_max
  } else {
    seconds * 1000000000UL + nanos
  }
}

///|
fn scale_duration(duration : UInt64, factor : Double) -> UInt64 {
  if !is_finite(factor) || factor <= 0.0 {
    0
  } else {
    let seconds = (duration / 1000000000UL).to_double() +
      (duration % 1000000000UL).to_double() / 1000000000.0
    let value = seconds * factor
    if !is_finite(value) {
      uint64_max
    } else {
      seconds_float_to_nanos(value)
    }
  }
}

///|
fn fibonacci_delay(first : UInt64, second : UInt64, nth : UInt) -> UInt64 {
  match nth {
    0 => 0
    1 => first
    2 => second
    _ => {
      let mut left = first
      let mut right = second
      let mut n = 3U
      while n <= nth {
        let next = saturating_add(left, right)
        left = right
        right = next
        if n == nth {
          break
        }
        n += 1
      }
      right
    }
  }
}

///|
fn find_property(
  properties : Array[(String, PredicateValue)],
  name : String,
) -> PredicateValue? {
  let mut found = None
  for _, entry in properties {
    if entry.0 == name {
      found = Some(entry.1)
    }
  }
  found
}

///|
fn required_property(
  properties : Array[(String, PredicateValue)],
  name : String,
) -> PredicateValue raise LocalRetryError {
  match find_property(properties, name) {
    Some(value) => value
    None =>
      raise LocalRetryError::PredicateEvaluation("Property not found: \{name}")
  }
}

///|
fn compare_values(
  actual : PredicateValue,
  expected : PredicateValue,
  property : String,
) -> Int raise LocalRetryError {
  match (actual, expected) {
    (PredicateValue::Integer(left), PredicateValue::Integer(right)) =>
      left.compare(right)
    (PredicateValue::Text(left), PredicateValue::Text(right)) =>
      @utf8.encode(left).lexical_compare(@utf8.encode(right))
    (PredicateValue::Boolean(left), PredicateValue::Boolean(right)) =>
      left.compare(right)
    (PredicateValue::Text(left), PredicateValue::Integer(right)) => {
      if !is_decimal_integer(left) {
        raise LocalRetryError::PredicateEvaluation(
          "Cannot coerce property '\{property}' to integer",
        )
      }
      let parsed : Int64 = @strconv.from_str(left) catch {
        _ =>
          raise LocalRetryError::PredicateEvaluation(
            "Cannot coerce property '\{property}' to integer",
          )
      }
      parsed.compare(right)
    }
    (PredicateValue::Integer(left), PredicateValue::Text(right)) =>
      @utf8.encode(left.to_string()).lexical_compare(@utf8.encode(right))
    _ =>
      raise LocalRetryError::PredicateEvaluation(
        "Cannot coerce property '\{property}' to the predicate value type",
      )
  }
}

///|
fn is_decimal_integer(value : String) -> Bool {
  let chars = value.to_array()
  if chars.is_empty() {
    return false
  }
  let start = if chars[0] == '+' || chars[0] == '-' { 1 } else { 0 }
  if start == chars.length() {
    return false
  }
  for index = start; index < chars.length(); index = index + 1 {
    if chars[index] < '0' || chars[index] > '9' {
      return false
    }
  }
  true
}

///|
fn predicate_text(
  value : PredicateValue,
  property : String,
) -> String raise LocalRetryError {
  match value {
    PredicateValue::Text(value) => value
    PredicateValue::Integer(value) => value.to_string()
    PredicateValue::Boolean(_) =>
      raise LocalRetryError::PredicateEvaluation(
        "Cannot coerce property '\{property}' to text",
      )
  }
}

///|
fn LocalPredicate::matches(
  self : LocalPredicate,
  properties : Array[(String, PredicateValue)],
) -> Bool raise LocalRetryError {
  match self {
    Eq(name, expected) =>
      compare_values(required_property(properties, name), expected, name) == 0
    Neq(name, expected) =>
      compare_values(required_property(properties, name), expected, name) != 0
    Gt(name, expected) =>
      compare_values(required_property(properties, name), expected, name) > 0
    Gte(name, expected) =>
      compare_values(required_property(properties, name), expected, name) >= 0
    Lt(name, expected) =>
      compare_values(required_property(properties, name), expected, name) < 0
    Lte(name, expected) =>
      compare_values(required_property(properties, name), expected, name) <= 0
    Exists(name) => find_property(properties, name) is Some(_)
    In(name, expected) => {
      let actual = required_property(properties, name)
      let mut first_error : LocalRetryError? = None
      for candidate in expected {
        let comparison = try compare_values(actual, candidate, name) catch {
          error => {
            if first_error is None {
              first_error = Some(error)
            }
            None
          }
        } noraise {
          value => Some(value)
        }
        if comparison == Some(0) {
          return true
        }
      }
      match first_error {
        Some(error) => raise error
        None => false
      }
    }
    Matches(name, pattern) =>
      glob_matches(
        pattern,
        predicate_text(required_property(properties, name), name),
      )
    StartsWith(name, prefix) =>
      predicate_text(required_property(properties, name), name).has_prefix(
        prefix,
      )
    Contains(name, substring) =>
      predicate_text(required_property(properties, name), name).contains(
        substring,
      )
    And(left, right) => left.matches(properties) && right.matches(properties)
    Or(left, right) => left.matches(properties) || right.matches(properties)
    Not(inner) => !inner.matches(properties)
    True => true
    False => false
  }
}

///|
priv struct GlobWildcard {
  glob_index : Int
  path_index : Int
}

///|
priv struct GlobState {
  mut path_index : Int
  mut glob_index : Int
  mut wildcard : GlobWildcard
  mut globstar : GlobWildcard
}

///|
priv enum GlobBraceState {
  Invalid
  Comma
  EndBrace
}

///|
fn glob_matches(pattern : String, value : String) -> Bool {
  let glob = @utf8.encode(pattern)
  let path = @utf8.encode(value)
  let mut state = new_glob_state()
  let brace_stack : Array[GlobState] = []
  let mut longest_brace_match = 0
  let mut negated = false
  while state.glob_index < glob.length() && glob[state.glob_index] == b'!' {
    negated = !negated
    state.glob_index += 1
  }
  while state.glob_index < glob.length() || state.path_index < path.length() {
    if state.glob_index < glob.length() {
      match glob[state.glob_index] {
        b'*' => {
          let is_globstar = state.glob_index + 1 < glob.length() &&
            glob[state.glob_index + 1] == b'*'
          if is_globstar {
            state.glob_index = skip_globstars(glob, state.glob_index + 2) - 2
          }
          state.wildcard = {
            glob_index: state.glob_index,
            path_index: state.path_index + 1,
          }
          if is_globstar {
            state.glob_index += 2
            if state.glob_index == glob.length() {
              state.globstar = state.wildcard
            } else if (
                state.glob_index < 3 || glob[state.glob_index - 3] == b'/'
              ) &&
              glob[state.glob_index] == b'/' {
              if state.path_index == 0 ||
                (
                  state.path_index < path.length() &&
                  path[state.path_index - 1] == b'/'
                ) {
                state.glob_index += 1
              }
              state.globstar = state.wildcard
            }
          } else {
            state.glob_index += 1
          }
          if state.globstar.path_index != state.wildcard.path_index &&
            state.path_index < path.length() &&
            path[state.path_index] == b'/' {
            if state.globstar.path_index > 0 &&
              state.path_index + 1 < path.length() {
              state.glob_index = state.globstar.glob_index
              state.wildcard = state.globstar
            } else {
              state.wildcard = { ..state.wildcard, path_index: 0, }
            }
          }
          if !brace_stack.is_empty() &&
            state.glob_index < glob.length() &&
            (glob[state.glob_index] == b',' || glob[state.glob_index] == b'}') {
            if skip_glob_braces(state, glob, false) is Invalid {
              return false
            }
          }
          continue
        }
        b'?' if state.path_index < path.length() =>
          if path[state.path_index] != b'/' {
            state.glob_index += 1
            state.path_index += 1
            continue
          }
        b'[' if state.path_index < path.length() => {
          state.glob_index += 1
          let actual = path[state.path_index]
          let mut class_negated = false
          if state.glob_index < glob.length() &&
            (glob[state.glob_index] == b'^' || glob[state.glob_index] == b'!') {
            class_negated = true
            state.glob_index += 1
          }
          let mut first = true
          let mut matched = false
          while state.glob_index < glob.length() &&
                (first || glob[state.glob_index] != b']') {
            guard unescape_glob(glob, state.glob_index) is Some((low, next)) else {
              return false
            }
            state.glob_index = next
            let high = if state.glob_index + 1 < glob.length() &&
              glob[state.glob_index] == b'-' &&
              glob[state.glob_index + 1] != b']' {
              guard unescape_glob(glob, state.glob_index + 1)
                is Some((high, next)) else {
                return false
              }
              state.glob_index = next
              high
            } else {
              low
            }
            if low <= actual && actual <= high {
              matched = true
            }
            first = false
          }
          if state.glob_index >= glob.length() {
            return false
          }
          state.glob_index += 1
          if matched != class_negated {
            state.path_index += 1
            continue
          }
        }
        b'{' if state.path_index < path.length() => {
          if brace_stack.length() >= 10 {
            return false
          }
          brace_stack.push(copy_glob_state(state))
          let path_index = state.path_index
          let glob_index = state.glob_index + 1
          state = new_glob_state()
          state.path_index = path_index
          state.glob_index = glob_index
          continue
        }
        b'}' if !brace_stack.is_empty() => {
          longest_brace_match = Int::max(longest_brace_match, state.path_index)
          state.glob_index += 1
          let saved = brace_stack.pop().unwrap()
          state = pop_glob_brace(state, saved, longest_brace_match)
          if brace_stack.is_empty() {
            longest_brace_match = 0
          }
          continue
        }
        b',' if !brace_stack.is_empty() => {
          longest_brace_match = Int::max(longest_brace_match, state.path_index)
          state.path_index = brace_stack[brace_stack.length() - 1].path_index
          state.glob_index += 1
          state.wildcard = empty_glob_wildcard()
          state.globstar = empty_glob_wildcard()
          continue
        }
        _ if state.path_index < path.length() => {
          guard unescape_glob(glob, state.glob_index) is Some((literal, next)) else {
            return false
          }
          state.glob_index = next - 1
          let matched = if literal == b'/' {
            path[state.path_index] == b'/'
          } else {
            path[state.path_index] == literal
          }
          if matched {
            if !brace_stack.is_empty() &&
              state.glob_index > 0 &&
              glob[state.glob_index - 1] == b'}' {
              longest_brace_match = state.path_index
              let saved = brace_stack.pop().unwrap()
              state = pop_glob_brace(state, saved, longest_brace_match)
              if brace_stack.is_empty() {
                longest_brace_match = 0
              }
            }
            state.glob_index += 1
            state.path_index += 1
            if literal != b'/' {
              state.globstar = { ..state.globstar, path_index: 0, }
            }
            continue
          }
        }
        _ => ()
      }
    }
    if state.wildcard.path_index > 0 &&
      state.wildcard.path_index <= path.length() {
      backtrack_glob(state)
      continue
    }
    if !brace_stack.is_empty() {
      match skip_glob_braces(state, glob, true) {
        Invalid => return false
        Comma => {
          state.path_index = brace_stack[brace_stack.length() - 1].path_index
          continue
        }
        EndBrace => ()
      }
      if longest_brace_match > 0 {
        let saved = brace_stack.pop().unwrap()
        state = pop_glob_brace(state, saved, longest_brace_match)
        if brace_stack.is_empty() {
          longest_brace_match = 0
        }
        continue
      } else {
        state = brace_stack.pop().unwrap()
        if state.wildcard.path_index > 0 &&
          state.wildcard.path_index <= path.length() {
          backtrack_glob(state)
          continue
        }
      }
    }
    return negated
  }
  !negated
}

///|
fn empty_glob_wildcard() -> GlobWildcard {
  { glob_index: 0, path_index: 0, }
}

///|
fn new_glob_state() -> GlobState {
  {
    path_index: 0,
    glob_index: 0,
    wildcard: empty_glob_wildcard(),
    globstar: empty_glob_wildcard(),
  }
}

///|
fn copy_glob_state(state : GlobState) -> GlobState {
  {
    path_index: state.path_index,
    glob_index: state.glob_index,
    wildcard: state.wildcard,
    globstar: state.globstar,
  }
}

///|
fn skip_globstars(glob : Bytes, start : Int) -> Int {
  let mut index = start
  while index + 3 <= glob.length() &&
        glob[index] == b'/' &&
        glob[index + 1] == b'*' &&
        glob[index + 2] == b'*' {
    index += 3
  }
  index
}

///|
fn unescape_glob(glob : Bytes, index : Int) -> (Byte, Int)? {
  let byte = glob[index]
  if byte == b'\\' {
    guard glob.get(index + 1) is Some(escaped) else { return None }
    let literal = match escaped {
      b'a' => b'\x61'
      b'b' => b'\x08'
      b'n' => b'\n'
      b'r' => b'\r'
      b't' => b'\t'
      other => other
    }
    Some((literal, index + 2))
  } else {
    Some((byte, index + 1))
  }
}

///|
fn skip_glob_braces(
  state : GlobState,
  glob : Bytes,
  stop_on_comma : Bool,
) -> GlobBraceState {
  let mut braces = 1
  let mut in_brackets = false
  while state.glob_index < glob.length() && braces > 0 {
    let byte = glob[state.glob_index]
    if byte == b'{' && !in_brackets {
      braces += 1
    } else if byte == b'}' && !in_brackets {
      braces -= 1
    } else if byte == b',' && stop_on_comma && braces == 1 && !in_brackets {
      state.glob_index += 1
      return Comma
    } else if (byte == b'*' || byte == b'?' || byte == b'[') && !in_brackets {
      if byte == b'[' {
        in_brackets = true
      } else if byte == b'*' &&
        state.glob_index + 1 < glob.length() &&
        glob[state.glob_index + 1] == b'*' {
        state.glob_index = skip_globstars(glob, state.glob_index + 2) - 1
      }
    } else if byte == b']' {
      in_brackets = false
    } else if byte == b'\\' {
      state.glob_index += 1
    }
    state.glob_index += 1
  }
  if braces == 0 {
    EndBrace
  } else {
    Invalid
  }
}

///|
fn pop_glob_brace(
  state : GlobState,
  saved : GlobState,
  longest_match : Int,
) -> GlobState {
  {
    path_index: longest_match,
    glob_index: state.glob_index,
    wildcard: saved.wildcard,
    globstar: saved.globstar,
  }
}

///|
fn backtrack_glob(state : GlobState) -> Unit {
  state.glob_index = state.wildcard.glob_index
  state.path_index = state.wildcard.path_index
}

///|
fn LocalPolicy::step(
  self : LocalPolicy,
  state : LocalPolicyState,
  elapsed : UInt64,
  properties : Array[(String, PredicateValue)],
  random : () -> Double,
) -> (LocalPolicyState, LocalVerdict) raise LocalRetryError {
  match (self, state) {
    (Periodic(delay), Counter(count)) =>
      (Counter(saturating_increment(count)), Retry(delay))
    (Exponential(base, factor), Counter(count)) =>
      (
        Counter(saturating_increment(count)),
        Retry(scale_duration(base, @math.pow(factor, count.to_double()))),
      )
    (Fibonacci(first, second), Counter(count)) =>
      (
        Counter(saturating_increment(count)),
        Retry(fibonacci_delay(first, second, saturating_increment(count))),
      )
    (Immediate, Counter(count)) =>
      (Counter(saturating_increment(count)), Retry(0))
    (Never, Terminal) => (Terminal, GiveUp)
    (CountBox(max_retries, inner), Counted(attempts, inner_state)) =>
      if attempts >= max_retries {
        (Counted(attempts, inner_state), GiveUp)
      } else {
        let (next, verdict) = inner.step(
          inner_state, elapsed, properties, random,
        )
        (Counted(saturating_increment(attempts), next), verdict)
      }
    (TimeBox(limit, inner), Wrapper(inner_state)) =>
      if elapsed >= limit {
        (Wrapper(inner_state), GiveUp)
      } else {
        let (next, verdict) = inner.step(
          inner_state, elapsed, properties, random,
        )
        (Wrapper(next), verdict)
      }
    (Clamp(minimum, maximum, inner), Wrapper(inner_state)) => {
      let (next, verdict) = inner.step(inner_state, elapsed, properties, random)
      let clamped = match verdict {
        Retry(delay) =>
          Retry(
            if delay < minimum {
              minimum
            } else if delay > maximum {
              maximum
            } else {
              delay
            },
          )
        GiveUp => GiveUp
      }
      (Wrapper(next), clamped)
    }
    (AddDelay(extra, inner), Wrapper(inner_state)) => {
      let (next, verdict) = inner.step(inner_state, elapsed, properties, random)
      let added = match verdict {
        Retry(delay) => Retry(saturating_add(delay, extra))
        GiveUp => GiveUp
      }
      (Wrapper(next), added)
    }
    (Jitter(factor, inner), Wrapper(inner_state)) => {
      let (next, verdict) = inner.step(inner_state, elapsed, properties, random)
      let jittered = match verdict {
        Retry(delay) if factor > 0.0 =>
          Retry(saturating_add(delay, scale_duration(delay, random() * factor)))
        other => other
      }
      (Wrapper(next), jittered)
    }
    (Filtered(predicate, inner), Wrapper(inner_state)) =>
      if predicate.matches(properties) {
        let (next, verdict) = inner.step(
          inner_state, elapsed, properties, random,
        )
        (Wrapper(next), verdict)
      } else {
        (Wrapper(inner_state), GiveUp)
      }
    (AndThen(left, right), Sequenced(left_state, right_state, on_right)) =>
      if on_right {
        let (next, verdict) = right.step(
          right_state, elapsed, properties, random,
        )
        (Sequenced(left_state, next, true), verdict)
      } else {
        let (next_left, verdict) = left.step(
          left_state, elapsed, properties, random,
        )
        match verdict {
          Retry(delay) =>
            (Sequenced(next_left, right_state, false), Retry(delay))
          GiveUp => {
            let (next_right, right_verdict) = right.step(
              right_state, elapsed, properties, random,
            )
            (Sequenced(next_left, next_right, true), right_verdict)
          }
        }
      }
    (Union(left, right), Pair(left_state, right_state)) => {
      let (next_left, left_verdict) = left.step(
        left_state, elapsed, properties, random,
      )
      let (next_right, right_verdict) = right.step(
        right_state, elapsed, properties, random,
      )
      let verdict = match (left_verdict, right_verdict) {
        (Retry(a), Retry(b)) => Retry(if a < b { a } else { b })
        (Retry(delay), GiveUp) | (GiveUp, Retry(delay)) => Retry(delay)
        (GiveUp, GiveUp) => GiveUp
      }
      (Pair(next_left, next_right), verdict)
    }
    (Intersect(left, right), Pair(left_state, right_state)) => {
      let (next_left, left_verdict) = left.step(
        left_state, elapsed, properties, random,
      )
      let (next_right, right_verdict) = right.step(
        right_state, elapsed, properties, random,
      )
      let verdict = match (left_verdict, right_verdict) {
        (Retry(a), Retry(b)) => Retry(if a > b { a } else { b })
        (GiveUp, _) | (_, GiveUp) => GiveUp
      }
      (Pair(next_left, next_right), verdict)
    }
    _ =>
      raise invalid_policy("Internal policy state does not match policy node")
  }
}

///|
fn default_random() -> Double {
  let bits = @insecure.get_insecure_random_u64() >> 11
  bits.to_double() / 9007199254740992.0
}

///|
async fn[T, E] retry_with_runtime(
  policy : RetryPolicy,
  operation : async () -> Result[T, E],
  properties : (E) -> Array[(String, PredicateValue)],
  now : () -> UInt64,
  sleep : async (UInt64) -> Unit,
  random : () -> Double,
) -> Result[T, E] {
  let parsed = parse_policy(policy)
  let mut state = parsed.initial_state()
  let started = now()
  for ;; {
    match operation() {
      Ok(value) => return Ok(value)
      Err(error) => {
        let current = now()
        let elapsed = if current >= started { current - started } else { 0 }
        let (next, verdict) = parsed.step(
          state,
          elapsed,
          properties(error),
          random,
        )
        state = next
        match verdict {
          LocalVerdict::Retry(delay) => sleep(delay)
          LocalVerdict::GiveUp => return Err(error)
        }
      }
    }
  }
}

///|
/// Retries an asynchronous fallible operation by interpreting a Golem semantic
/// retry policy entirely in user space.
///
/// `properties` is evaluated after every failure, so filtered policies can react
/// to the current error. When the policy gives up, the latest operation error is
/// returned unchanged. Local attempts are ordinary user code and are not
/// executor-managed retry attempts or a durable retry sequence.
pub async fn[T, E] retry_local(
  policy : RetryPolicy,
  operation : async () -> Result[T, E],
  properties? : (E) -> Array[(String, PredicateValue)] = _ => [],
) -> Result[T, E] {
  retry_with_runtime(
    policy, operation, properties, @monotonic.now, @monotonic.wait_for, default_random,
  )
}
