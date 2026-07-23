///|
fn[T] expect_retry_builder_error(
  f : () -> T raise RetryBuilderError,
  expected : String,
) -> Unit raise {
  let result : Result[T, RetryBuilderError] = try? f()
  guard result is Err(RetryBuilderError(message)) else {
    fail("Expected RetryBuilderError")
  }
  assert_eq(message, expected)
}

///|
test "predicate builder flattens nested tree" {
  let predicate = Predicate::and_(
    Predicate::one_of(Props::status_code(), [Value::uint(429), Value::uint(503)]),
    Predicate::not_(Predicate::matches_glob(Props::error_type(), "fatal-*")),
  )

  let raw : @retry.RetryPredicate = try! predicate.to_raw()
  let expected : @retry.RetryPredicate = {
    nodes: [
      @retry.PredicateNode::PredAnd((1, 2)),
      @retry.PredicateNode::PropIn({
        property_name: "status-code",
        values: [PredicateValue::Integer(429), PredicateValue::Integer(503)],
      }),
      @retry.PredicateNode::PredNot(3),
      @retry.PredicateNode::PropMatches({
        property_name: "error-type",
        pattern: "fatal-*",
      }),
    ],
  }

  assert_eq(raw, expected)
}

///|
test "policy builder flattens validated unary chain" {
  let policy = Policy::exponential(Duration::millis(250), 2.0)
    .max_retries(5)
    .within(Duration::seconds(30))
    .only_when(Predicate::eq(Props::verb(), Value::text("invoke")))

  let raw : @retry.RetryPolicy = try! policy.to_raw()
  let expected : @retry.RetryPolicy = {
    nodes: [
      @retry.PolicyNode::FilteredOn({
        predicate: {
          nodes: [
            @retry.PredicateNode::PropEq({
              property_name: "verb",
              value: PredicateValue::Text("invoke"),
            }),
          ],
        },
        inner: 1,
      }),
      @retry.PolicyNode::TimeBox({ limit: 30000000000UL, inner: 2 }),
      @retry.PolicyNode::CountBox({ max_retries: 5, inner: 3 }),
      @retry.PolicyNode::Exponential({ base_delay: 250000000UL, factor: 2.0 }),
    ],
  }

  assert_eq(raw, expected)
}

///|
test "policy builder flattens binary chain" {
  let policy = Policy::immediate().and_then(
    Policy::periodic(Duration::seconds(1)).with_jitter(0.0),
  )

  let raw : @retry.RetryPolicy = try! policy.to_raw()
  let expected : @retry.RetryPolicy = {
    nodes: [
      @retry.PolicyNode::AndThen((1, 2)),
      @retry.PolicyNode::Immediate,
      @retry.PolicyNode::Jitter({ factor: 0.0, inner: 3 }),
      @retry.PolicyNode::Periodic(1000000000UL),
    ],
  }

  assert_eq(raw, expected)
}

///|
test "named policy builder defaults and overrides" {
  let default_rule = NamedPolicy::named("default", Policy::never())
  let default_raw : NamedRetryPolicy = try! default_rule.to_raw()
  let default_expected : NamedRetryPolicy = {
    name: "default",
    priority: 0,
    predicate: { nodes: [@retry.PredicateNode::PredTrue] },
    policy: { nodes: [@retry.PolicyNode::Never] },
  }
  assert_eq(default_raw, default_expected)

  let custom_rule = NamedPolicy::named(
      "timeouts",
      Policy::periodic(Duration::seconds(1)),
    )
    .priority(10)
    .applies_when(Predicate::contains(Props::error_type(), "timeout"))

  let custom_raw : NamedRetryPolicy = try! custom_rule.to_raw()
  let custom_expected : NamedRetryPolicy = {
    name: "timeouts",
    priority: 10,
    predicate: {
      nodes: [
        @retry.PredicateNode::PropContains({
          property_name: "error-type",
          pattern: "timeout",
        }),
      ],
    },
    policy: { nodes: [@retry.PolicyNode::Periodic(1000000000UL)] },
  }
  assert_eq(custom_raw, custom_expected)
}

///|
test "builder validation rejects invalid inputs" {
  expect_retry_builder_error(
    () => Policy::exponential(Duration::seconds(1), 0.0).to_raw(),
    "Exponential factor must be finite and greater than 0",
  )
  expect_retry_builder_error(
    () => Policy::immediate().with_jitter(-0.1).to_raw(),
    "Jitter factor must be finite and greater than or equal to 0",
  )
  expect_retry_builder_error(
    () => {
      Policy::immediate()
      .clamp(Duration::seconds(3), Duration::seconds(1))
      .to_raw()
    },
    "Clamp min_delay must be less than or equal to max_delay",
  )
  expect_retry_builder_error(
    () => Value::uint64(9223372036854775808UL).to_raw(),
    "Predicate integer value exceeds s64 range: 9223372036854775808",
  )
  expect_retry_builder_error(
    () => Duration::hours(18446744073709551615UL).to_nanos(),
    "Duration hours(18446744073709551615) exceeds u64 nanoseconds",
  )
}

///|
test "builder validation rejects non-finite factors" {
  let nan = 0.0 / 0.0
  let inf = 1.0 / 0.0

  expect_retry_builder_error(
    () => Policy::exponential(Duration::seconds(1), nan).to_raw(),
    "Exponential factor must be finite and greater than 0",
  )
  expect_retry_builder_error(
    () => Policy::exponential(Duration::seconds(1), inf).to_raw(),
    "Exponential factor must be finite and greater than 0",
  )
  expect_retry_builder_error(
    () => Policy::immediate().with_jitter(nan).to_raw(),
    "Jitter factor must be finite and greater than or equal to 0",
  )
  expect_retry_builder_error(
    () => Policy::immediate().with_jitter(inf).to_raw(),
    "Jitter factor must be finite and greater than or equal to 0",
  )
}

///|
test "builder boundary values convert successfully" {
  let raw_value = try! Value::uint64(9223372036854775807UL).to_raw()
  assert_eq(raw_value, PredicateValue::Integer(9223372036854775807L))

  let nanos = try! Duration::seconds(18446744073UL).to_nanos()
  assert_eq(nanos, 18446744073000000000UL)

  let raw_policy = try! Policy::immediate()
    .clamp(Duration::millis(250), Duration::millis(250))
    .to_raw()
  let expected : @retry.RetryPolicy = {
    nodes: [
      @retry.PolicyNode::ClampDelay({
        min_delay: 250000000UL,
        max_delay: 250000000UL,
        inner: 1,
      }),
      @retry.PolicyNode::Immediate,
    ],
  }
  assert_eq(raw_policy, expected)
}
