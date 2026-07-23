// Phase 3g tests for the `AgentError::CustomError` end-to-end path.
//
// Unlike the quota/rpc helpers, none of these touch a WASM host import: the
// schema encode/decode codec is pure, so the full construct → carrier → decode
// round-trip runs host-free.

///|
test "custom_error round-trips a string message" {
  let err = custom_error("boom")
  guard err is CustomError(_) else { fail("expected CustomError") }
  let decoded : String? = decode_custom_error(err)
  inspect(decoded, content="Some(boom)")
}

///|
test "internal_error prefixes and round-trips" {
  let err = internal_error("disk full")
  let decoded : String? = decode_custom_error(err)
  inspect(
    decoded,
    content=(
      #|Some(Internal error: disk full)
    ),
  )
}

///|
test "custom_error_of round-trips a typed (non-string) value" {
  let err = custom_error_of((42 : Int))
  guard err is CustomError(_) else { fail("expected CustomError") }
  let decoded : Int? = decode_custom_error(err)
  inspect(decoded, content="Some(42)")
}

///|
test "custom_error carries a self-describing schema graph" {
  let err = custom_error("x")
  guard err is CustomError(typed) else { fail("expected CustomError") }
  // The payload must carry a non-empty schema graph alongside the value tree
  // (guards against a "value only, missing schema" encoding regression).
  inspect(typed.graph.type_nodes.length() > 0, content="true")
}

///|
test "invalid_* helpers build the matching variants" {
  inspect(
    invalid_input_error("bad"),
    content=(
      #|InvalidInput(bad)
    ),
  )
  inspect(
    invalid_method_error("nope"),
    content=(
      #|InvalidMethod(nope)
    ),
  )
  inspect(
    invalid_type_error("wrong"),
    content=(
      #|InvalidType(wrong)
    ),
  )
}

///|
test "decode_custom_error returns None for non-custom errors" {
  let decoded : String? = decode_custom_error(invalid_input_error("x"))
  inspect(decoded, content="None")
}

///|
test "decode_custom_error raises on a type mismatch" {
  let err = custom_error("not a number")
  let r : Result[Int?, _] = try? decode_custom_error(err)
  guard r is Err(_) else { fail("expected a decode error") }
}
