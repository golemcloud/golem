///|
fn check_round_trip(
  name : String,
  issuer : String,
  subject : String,
  text : String,
) -> Unit raise {
  let reference = PrincipalRef::new(issuer, subject)
  if reference.to_string() != text {
    fail("\{name}: printed \{reference.to_string()}, want \{text}")
  }
  let parsed = PrincipalRef::parse(text) catch {
    PrincipalRefParseError(message) => fail("\{name}: \{message}")
  }
  if !parsed.equals(reference) {
    fail("\{name}: parsed a different reference")
  }
}

///|
fn check_rejected(name : String, text : String) -> Unit raise {
  let _ = PrincipalRef::parse(text) catch { _ => return }
  fail("\{name}: parsed")
}

///|
fn pair(subject : String) -> @common.PrincipalRef {
  { issuer: "https://login.example", subject, }
}

///|
fn wit_user(subject : String, claims : String) -> @common.Principal {
  User({
    pair: pair(subject),
    claims,
    authenticated_at: { seconds: 1700000000L, nanoseconds: 5U, },
  })
}

///|
test "references compare byte for byte" {
  let base = PrincipalRef::new("https://login.example/", "Subject-1")
  assert_true(
    base.equals(PrincipalRef::new("https://login.example/", "Subject-1")),
  )
  assert_false(
    base.equals(PrincipalRef::new("https://login.example", "Subject-1")),
  )
  assert_false(
    base.equals(PrincipalRef::new("https://login.example/", "subject-1")),
  )
  assert_false(
    base.equals(PrincipalRef::new("https://login.example/", "Subject-1 ")),
  )
}

///|
test "a caller reads both roles and prints without claims" {
  let caller = Caller::from_wit({
    original: wit_user("u-1", "{\"role\":\"admin\",\"note\":\"secret\"}"),
    immediate: Tool({
      pair: pair("tool:4/report"),
      environment_id: { uuid: { high_bits: 4UL, low_bits: 7UL, }, },
      tool_name: "report",
    }),
    tokens: [
      {
        id: { high_bits: 9UL, low_bits: 7UL, },
        maker: Some(pair("account:1")),
      },
    ],
  })
  assert_eq(caller.original.kind(), PrincipalKind::User)
  assert_eq(caller.immediate.kind(), PrincipalKind::Tool)
  assert_eq(caller.tokens.length(), 1)
  assert_eq(
    caller.original.to_string(),
    "user (\"https://login.example\",\"u-1\")",
  )
  assert_false(caller.original.to_string().contains("secret"))
  assert_eq(Caller::anonymous().original.to_string(), "anonymous")
}

///|
test "principal equality is identity and anonymous equals nothing" {
  let a = Principal::from_wit(wit_user("u-1", "{}"))
  let same = Principal::from_wit(wit_user("u-1", "{\"iat\":2}"))
  let other = Principal::from_wit(wit_user("u-2", "{}"))
  let nobody = Principal::from_wit(Anonymous)
  assert_true(a.equals(same))
  assert_false(a.equals(other))
  assert_true(a.equals_ref(PrincipalRef::new("https://login.example", "u-1")))
  assert_false(nobody.equals(nobody))
  assert_false(nobody.equals(a))
  assert_false(a.equals(nobody))
}

///|
test "an invocation context keeps the caller it was made with" {
  let context = InvocationContext::from_caller(Caller::anonymous())
  assert_eq(context.caller().original.kind(), PrincipalKind::Anonymous)
  assert_eq(
    InvocationContext::anonymous().caller().immediate.kind(),
    PrincipalKind::Anonymous,
  )
}

///|
test "a context read during its call keeps that caller after the call ends" {
  let reads : Ref[Int32] = { val: 0, }
  let context : InvocationContext = {
    cached: None,
    ended: false,
    read: () => {
      reads.val += 1
      Caller::anonymous()
    },
  }
  assert_eq(reads.val, 0)
  match context.caller_or_ended() {
    Ok(caller) => assert_eq(caller.original.kind(), PrincipalKind::Anonymous)
    Err(message) => fail(message)
  }
  context.end()
  match context.caller_or_ended() {
    Ok(caller) => assert_eq(caller.original.kind(), PrincipalKind::Anonymous)
    Err(message) => fail(message)
  }
  assert_eq(reads.val, 1)
}

///|
test "a context first read after its call ended refuses" {
  let reads : Ref[Int32] = { val: 0, }
  let context : InvocationContext = {
    cached: None,
    ended: false,
    read: () => {
      reads.val += 1
      Caller::anonymous()
    },
  }
  context.end()
  match context.caller_or_ended() {
    Ok(_) => fail("a first read after the call ended must refuse")
    Err(message) => assert_eq(message, invocation_context_ended)
  }
  assert_eq(reads.val, 0)
}

///|
test "a context made from a caller never refuses" {
  let context = InvocationContext::anonymous()
  context.end()
  assert_eq(context.caller().original.kind(), PrincipalKind::Anonymous)
}
