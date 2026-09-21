///|
fn raw_policy(nodes : Array[@retry.PolicyNode]) -> RetryPolicy {
  { nodes, }
}

///|
fn no_properties() -> Array[(String, PredicateValue)] {
  []
}

///|
fn step_delays(
  raw : RetryPolicy,
  elapsed : Array[UInt64],
  properties : Array[Array[(String, PredicateValue)]],
  random : Double,
) -> Array[UInt64?] {
  let policy = try! parse_policy(raw)
  let mut state = policy.initial_state()
  let result : Array[UInt64?] = []
  for index = 0; index < elapsed.length(); index = index + 1 {
    let (next, verdict) = try! policy.step(
      state,
      elapsed[index],
      properties[index],
      () => random,
    )
    state = next
    result.push(
      match verdict {
        Retry(delay) => Some(delay)
        GiveUp => None
      },
    )
  }
  result
}

///|
test "base schedules and decorators match semantic evaluator" {
  assert_eq(scale_duration(1, 0.5), 1)
  assert_eq(scale_duration(1, 1.5), 2)
  assert_eq(scale_duration(3, 0.5), 1)
  assert_eq(scale_duration(1000000001, 0.5), 500000001)
  assert_eq(scale_duration(18446744073709551615, 2.0), 18446744073709551615)
  assert_eq(scale_duration(9223372036854775807, 1.25), 11529215046068470001)
  assert_eq(scale_duration(999999999, 1.000000001), 1000000000)
  let empty = [
    no_properties(),
    no_properties(),
    no_properties(),
    no_properties(),
  ]
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::Exponential({ base_delay: 10, factor: 3.0, }),
      ]),
      [0, 0, 0, 0],
      empty,
      0.0,
    ),
    content="[Some(10), Some(30), Some(90), Some(270)]",
  )
  debug_inspect(
    step_delays(
      raw_policy([@retry.PolicyNode::Fibonacci({ first: 2, second: 7, })]),
      [0, 0, 0, 0],
      empty,
      0.0,
    ),
    content="[Some(2), Some(7), Some(9), Some(16)]",
  )
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::AddDelay({ delay: 20, inner: 1, }),
        @retry.PolicyNode::ClampDelay({ min_delay: 5, max_delay: 40, inner: 2, }),
        @retry.PolicyNode::Periodic(50),
      ]),
      [0],
      [no_properties()],
      0.0,
    ),
    content="[Some(60)]",
  )
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::Jitter({ factor: 0.5, inner: 1, }),
        @retry.PolicyNode::Periodic(100),
      ]),
      [0],
      [no_properties()],
      0.25,
    ),
    content="[Some(112)]",
  )
}

///|
test "count and elapsed boundaries give up at the canonical boundary" {
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::CountBox({ max_retries: 2, inner: 1, }),
        @retry.PolicyNode::Periodic(9),
      ]),
      [0, 0, 0],
      [no_properties(), no_properties(), no_properties()],
      0.0,
    ),
    content="[Some(9), Some(9), None]",
  )
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::TimeBox({ limit: 100, inner: 1, }),
        @retry.PolicyNode::Immediate,
      ]),
      [99, 100],
      [no_properties(), no_properties()],
      0.0,
    ),
    content="[Some(0), None]",
  )
}

///|
test "sequence union and intersection keep asymmetric independent state" {
  let none = [
    no_properties(),
    no_properties(),
    no_properties(),
    no_properties(),
  ]
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::AndThen((1, 3)),
        @retry.PolicyNode::CountBox({ max_retries: 1, inner: 2, }),
        @retry.PolicyNode::Periodic(3),
        @retry.PolicyNode::Exponential({ base_delay: 10, factor: 2.0, }),
      ]),
      [0, 0, 0],
      none,
      0.0,
    ),
    content="[Some(3), Some(10), Some(20)]",
  )
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::PolicyUnion((1, 3)),
        @retry.PolicyNode::CountBox({ max_retries: 1, inner: 2, }),
        @retry.PolicyNode::Periodic(100),
        @retry.PolicyNode::Exponential({ base_delay: 10, factor: 2.0, }),
      ]),
      [0, 0, 0],
      none,
      0.0,
    ),
    content="[Some(10), Some(20), Some(40)]",
  )
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::PolicyIntersect((1, 3)),
        @retry.PolicyNode::CountBox({ max_retries: 2, inner: 2, }),
        @retry.PolicyNode::Periodic(7),
        @retry.PolicyNode::Exponential({ base_delay: 10, factor: 3.0, }),
      ]),
      [0, 0, 0],
      none,
      0.0,
    ),
    content="[Some(10), Some(30), None]",
  )
}

///|
test "filtered policy projects properties from each failure" {
  let raw = raw_policy([
    @retry.PolicyNode::FilteredOn({
      predicate: {
        nodes: [
          @retry.PredicateNode::PredAnd((1, 2)),
          @retry.PredicateNode::PropGte({
            property_name: "status",
            value: PredicateValue::Integer(500),
          }),
          @retry.PredicateNode::PropStartsWith({
            property_name: "kind",
            pattern: "transient-",
          }),
        ],
      },
      inner: 1,
    }),
    @retry.PolicyNode::Immediate,
  ])
  debug_inspect(
    step_delays(
      raw,
      [0, 0],
      [
        [
          ("status", PredicateValue::Text("503")),
          ("kind", PredicateValue::Text("transient-network")),
        ],
        [
          ("status", PredicateValue::Integer(404)),
          ("kind", PredicateValue::Text("transient-network")),
        ],
      ],
      0.0,
    ),
    content="[Some(0), None]",
  )
}

///|
test "all predicate forms evaluate with canonical coercions" {
  let props = [
    ("number", PredicateValue::Text("12")),
    ("text", PredicateValue::Text("alpha/beta.txt")),
    ("flag", PredicateValue::Boolean(true)),
  ]
  let predicates = [
    LocalPredicate::Eq("number", PredicateValue::Integer(12)),
    LocalPredicate::Neq("number", PredicateValue::Integer(11)),
    LocalPredicate::Gt("number", PredicateValue::Integer(3)),
    LocalPredicate::Gte("number", PredicateValue::Integer(12)),
    LocalPredicate::Lt("number", PredicateValue::Integer(13)),
    LocalPredicate::Lte("number", PredicateValue::Integer(12)),
    LocalPredicate::Exists("flag"),
    LocalPredicate::In("number", [
      PredicateValue::Boolean(false),
      PredicateValue::Integer(12),
    ]),
    LocalPredicate::Matches("text", "alpha/*.txt"),
    LocalPredicate::StartsWith("text", "alpha"),
    LocalPredicate::Contains("text", "beta"),
    LocalPredicate::And(LocalPredicate::True, LocalPredicate::True),
    LocalPredicate::Or(LocalPredicate::False, LocalPredicate::True),
    LocalPredicate::Not(LocalPredicate::False),
  ]
  for _, predicate in predicates {
    assert_true(try! predicate.matches(props))
  }
  assert_true(
    try! (compare_values(
      PredicateValue::Text("z"),
      PredicateValue::Text("aa"),
      "text",
    ) >
    0),
  )
  assert_true(
    try! (compare_values(
      PredicateValue::Integer(9),
      PredicateValue::Text("10"),
      "number",
    ) >
    0),
  )
  for invalid in ["0x10", "1_0"] {
    try
      compare_values(
        PredicateValue::Text(invalid),
        PredicateValue::Integer(10),
        "number",
      )
    catch {
      LocalRetryError::PredicateEvaluation(_) => ()
      _ => fail("expected coercion failure")
    } noraise {
      _ => fail("expected coercion failure")
    }
  }
}

///|
test "glob predicates support canonical classes braces globstars and negation" {
  let cases = [
    ("some/**/needle.{js,txt}", "some/a/b/needle.txt", true),
    ("some/**/needle.{js,txt}", "some/needle.js", true),
    ("some/**/needle.txt", "some/pathneedle.txt", false),
    ("file-[a-c].txt", "file-b.txt", true),
    ("file-[!a-c].txt", "file-z.txt", true),
    ("file-[!a-c].txt", "file-b.txt", false),
    ("*.txt", "nested/file.txt", false),
    ("!*.txt", "file.json", true),
    ("a\\*b", "a*b", true),
    ("?", "é", false),
    ("a**", "a/b", true),
    ("***", "a/b", false),
    ("{a,ab}b", "ab", false),
    ("[]}]", "}", true),
    ("!**/a", "a", true),
    ("!a\\", "b", true),
    ("{*}a", "aa", false),
    ("{\\}}", "}", true),
    ("{\\,a}", "a", true),
  ]
  for _, test_case in cases {
    assert_eq(glob_matches(test_case.0, test_case.1), test_case.2)
  }
  assert_false(glob_matches("[unterminated", "u"))
  assert_true(glob_matches("{unterminated", "unterminated"))
  assert_false(glob_matches("trailing\\", "trailing"))
  assert_true(
    glob_matches(
      "{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}",
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ),
  )
  assert_true(
    glob_matches(
      "{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}",
      "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    ),
  )
}

///|
fn assert_invalid(raw : RetryPolicy, expected : String) -> Unit raise {
  try parse_policy(raw) catch {
    LocalRetryError::InvalidPolicy(message) => assert_eq(message, expected)
    _ => fail("expected invalid policy")
  } noraise {
    _ => fail("expected invalid policy")
  }
}

///|
fn assert_too_complex(raw : RetryPolicy) -> Unit raise {
  assert_invalid(raw, "Retry policy is too complex")
}

///|
test "malformed and cyclic raw policies are rejected before execution" {
  assert_invalid(raw_policy([]), "Retry policy has no root node")
  assert_invalid(
    raw_policy([@retry.PolicyNode::CountBox({ max_retries: 1, inner: 3, })]),
    "Policy node index out of bounds: 3",
  )
  assert_invalid(
    raw_policy([@retry.PolicyNode::AddDelay({ delay: 1, inner: 0, })]),
    "Cycle in retry policy at node 0",
  )
  assert_invalid(
    raw_policy([
      @retry.PolicyNode::FilteredOn({
        predicate: { nodes: [@retry.PredicateNode::PredNot(0)], },
        inner: 1,
      }),
      @retry.PolicyNode::Immediate,
    ]),
    "Cycle in retry predicate at node 0",
  )
  assert_invalid(
    raw_policy([
      @retry.PolicyNode::ClampDelay({ min_delay: 2, max_delay: 1, inner: 1, }),
      @retry.PolicyNode::Immediate,
    ]),
    "Clamp min_delay must not exceed max_delay",
  )
}

///|
test "raw policy depth expansion and predicate payload are bounded" {
  assert_too_complex(
    raw_policy(Array::make(max_compiled_nodes + 1, @retry.PolicyNode::Never)),
  )
  assert_too_complex(
    raw_policy([
      @retry.PolicyNode::FilteredOn({
        predicate: {
          nodes: Array::make(
            max_compiled_nodes + 1,
            @retry.PredicateNode::PredTrue,
          ),
        },
        inner: 1,
      }),
      @retry.PolicyNode::Immediate,
    ]),
  )

  let deepest_valid = Array::make(
    max_policy_depth + 1,
    @retry.PolicyNode::Immediate,
  )
  for index in 0..<max_policy_depth {
    deepest_valid[index] = @retry.PolicyNode::AddDelay({
      delay: 0,
      inner: index + 1,
    })
  }
  ignore(try! parse_policy(raw_policy(deepest_valid)))
  let deep_nodes = Array::make(max_policy_depth + 2, @retry.PolicyNode::Never)
  for index in 0..<=max_policy_depth {
    deep_nodes[index] = @retry.PolicyNode::AddDelay({
      delay: 0,
      inner: index + 1,
    })
  }
  assert_too_complex(raw_policy(deep_nodes))

  let valid_expansion : Array[@retry.PolicyNode] = [
    @retry.PolicyNode::AddDelay({ delay: 0, inner: 1, }),
  ]
  for index in 1..<12 {
    valid_expansion.push(@retry.PolicyNode::PolicyUnion((index + 1, index + 1)))
  }
  valid_expansion.push(@retry.PolicyNode::Immediate)
  ignore(try! parse_policy(raw_policy(valid_expansion)))
  let invalid_expansion : Array[@retry.PolicyNode] = [
    @retry.PolicyNode::AddDelay({ delay: 0, inner: 1, }),
    @retry.PolicyNode::AddDelay({ delay: 0, inner: 2, }),
  ]
  for index in 2..<13 {
    invalid_expansion.push(
      @retry.PolicyNode::PolicyUnion((index + 1, index + 1)),
    )
  }
  invalid_expansion.push(@retry.PolicyNode::Immediate)
  assert_too_complex(raw_policy(invalid_expansion))

  ignore(
    try! parse_policy(
      raw_policy([
        @retry.PolicyNode::FilteredOn({
          predicate: {
            nodes: [
              @retry.PredicateNode::PropExists(
                String::make(max_compiled_payload_bytes, 'a'),
              ),
            ],
          },
          inner: 1,
        }),
        @retry.PolicyNode::Immediate,
      ]),
    ),
  )
  assert_too_complex(
    raw_policy([
      @retry.PolicyNode::FilteredOn({
        predicate: {
          nodes: [
            @retry.PredicateNode::PropMatches({
              property_name: "",
              pattern: String::make(max_compiled_payload_bytes / 2 + 1, 'é'),
            }),
          ],
        },
        inner: 1,
      }),
      @retry.PolicyNode::Immediate,
    ]),
  )
  assert_too_complex(
    raw_policy([
      @retry.PolicyNode::FilteredOn({
        predicate: {
          nodes: [
            @retry.PredicateNode::PropIn({
              property_name: "value",
              values: Array::make(
                max_compiled_payload_bytes / predicate_value_payload_bytes + 1,
                PredicateValue::Integer(0),
              ),
            }),
          ],
        },
        inner: 1,
      }),
      @retry.PolicyNode::Immediate,
    ]),
  )
  let repeated_text = PredicateValue::Text(
    String::make(max_compiled_payload_bytes, 'x'),
  )
  assert_too_complex(
    raw_policy([
      @retry.PolicyNode::FilteredOn({
        predicate: {
          nodes: [
            @retry.PredicateNode::PropIn({
              property_name: "",
              values: Array::make(
                max_compiled_payload_bytes / predicate_value_payload_bytes,
                repeated_text,
              ),
            }),
          ],
        },
        inner: 1,
      }),
      @retry.PolicyNode::Immediate,
    ]),
  )
  let half_payload = String::make(max_compiled_payload_bytes / 2 + 1, 'a')
  assert_too_complex(
    raw_policy([
      @retry.PolicyNode::PolicyUnion((1, 3)),
      @retry.PolicyNode::FilteredOn({
        predicate: { nodes: [@retry.PredicateNode::PropExists(half_payload)], },
        inner: 2,
      }),
      @retry.PolicyNode::Immediate,
      @retry.PolicyNode::FilteredOn({
        predicate: { nodes: [@retry.PredicateNode::PropExists(half_payload)], },
        inner: 4,
      }),
      @retry.PolicyNode::Immediate,
    ]),
  )
}

///|
test "shared raw nodes compile to independent branch state below the limit" {
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::PolicyUnion((1, 1)),
        @retry.PolicyNode::Exponential({ base_delay: 20, factor: 2.0, }),
      ]),
      [0, 0],
      [no_properties(), no_properties()],
      0.0,
    ),
    content="[Some(20), Some(40)]",
  )
}

///|
test "raw factors outside builder constraints follow evaluator semantics" {
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::Exponential({ base_delay: 10, factor: 0.0, }),
      ]),
      [0, 0, 0],
      [no_properties(), no_properties(), no_properties()],
      0.0,
    ),
    content="[Some(10), Some(0), Some(0)]",
  )
  debug_inspect(
    step_delays(
      raw_policy([
        @retry.PolicyNode::Jitter({ factor: -0.1, inner: 1, }),
        @retry.PolicyNode::Periodic(7),
      ]),
      [0],
      [no_properties()],
      0.5,
    ),
    content="[Some(7)]",
  )
}

///|
test "retry decisions allow eventual success after the expected attempt count" {
  let policy = try! parse_policy(
    try! Policy::periodic(Duration::millis(1)).max_retries(4).to_raw(),
  )
  let mut state = policy.initial_state()
  let sleeps : Array[UInt64] = []
  let attempts : Array[Result[String, Int]] = [Err(1), Err(2), Ok("done")]
  let mut result : Result[String, Int] = Err(0)
  for _, attempt in attempts {
    match attempt {
      Ok(value) => {
        result = Ok(value)
        break
      }
      Err(error) => {
        result = Err(error)
        let (next, verdict) = try! policy.step(state, 0, [], () => 0.0)
        state = next
        match verdict {
          Retry(delay) => sleeps.push(delay)
          GiveUp => break
        }
      }
    }
  }
  debug_inspect(result, content="Ok(\"done\")")
  debug_inspect(sleeps, content="[1000000, 1000000]")
}

///|
test "final give up preserves the latest operation error" {
  let policy = try! parse_policy(
    try! Policy::immediate().max_retries(2).to_raw(),
  )
  let mut state = policy.initial_state()
  let errors = ["failure-1", "failure-2", "failure-3"]
  let mut final_error = ""
  let mut attempts = 0
  for _, error in errors {
    attempts += 1
    final_error = error
    let (next, verdict) = try! policy.step(state, 0, [], () => 0.0)
    state = next
    if verdict is GiveUp {
      break
    }
  }
  inspect(final_error, content="failure-3")
  inspect(attempts, content="3")
}
