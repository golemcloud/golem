///|
fn decode_with_drop_counts(
  tree : @types.SchemaValueTree,
) -> (Result[SchemaValue, SchemaModelError], Array[Int]) {
  let counts = [0, 0, 0]
  let drops : SchemaValueResourceDrops = {
    drop_secret: fn(_) { counts[0] += 1 },
    drop_quota_token: fn(_) { counts[1] += 1 },
    drop_stream: fn(_) { counts[2] += 1 },
    drop_native_stream: fn(_) {  },
  }
  let result = try? schema_value_from_wit(tree, drops)
  (result, counts)
}

///|
test "malformed duplicate raw handle is dropped exactly once" {
  let raw = @types.QuotaToken::QuotaToken(12)
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.QuotaTokenHandle(raw),
      @types.QuotaTokenHandle(raw),
      @types.TupleValue([0, 1]),
    ],
    root: 2,
  }
  let (result, drops) = decode_with_drop_counts(tree)
  guard result is Err(DecodeError(message)) else {
    fail("expected DecodeError, got \{result}")
  }
  assert_true(message.contains("more than one handle node"))
  assert_eq(drops, [0, 1, 0])
}

///|
test "multiply referenced handle is dropped when preflight rejects the tree" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(3)),
      @types.TupleValue([0, 0]),
    ],
    root: 1,
  }
  let (result, drops) = decode_with_drop_counts(tree)
  guard result is Err(DecodeError(message)) else {
    fail("expected DecodeError, got \{result}")
  }
  assert_true(message.contains("more than once"))
  assert_eq(drops, [0, 1, 0])
}

///|
test "unreachable handle is dropped when the whole tree is rejected" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.TupleValue([]),
      @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(4)),
    ],
    root: 0,
  }
  let (result, drops) = decode_with_drop_counts(tree)
  guard result is Err(DecodeError(message)) else {
    fail("expected DecodeError, got \{result}")
  }
  assert_true(message.contains("not referenced from the root"))
  assert_eq(drops, [0, 1, 0])
}

///|
test "preflight drops a stream when a later node is malformed" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(5)),
      @types.S8Value(128),
      @types.TupleValue([0, 1]),
    ],
    root: 2,
  }
  let (result, drops) = decode_with_drop_counts(tree)
  guard result is Err(DecodeError(_)) else {
    fail("expected DecodeError, got \{result}")
  }
  assert_eq(drops, [0, 0, 1])
}

///|
test "malformed tree drops each distinct owned resource exactly once" {
  let secret = @types.Secret::Secret(6)
  let quota_token = @types.QuotaToken::QuotaToken(7)
  let stream = @types.SchemaValueStream::SchemaValueStream(8)
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.SecretValue(secret),
      @types.SecretValue(secret),
      @types.QuotaTokenHandle(quota_token),
      @types.QuotaTokenHandle(quota_token),
      @types.StreamValue(stream),
      @types.StreamValue(stream),
    ],
    root: 99,
  }
  let (result, drops) = decode_with_drop_counts(tree)
  guard result is Err(DecodeError(_)) else {
    fail("expected DecodeError, got \{result}")
  }
  assert_eq(drops, [1, 1, 1])
}

///|
test "successful decode transfers resources without dropping them" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.SecretValue(@types.Secret::Secret(9)),
      @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(10)),
      @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(11)),
      @types.TupleValue([0, 1, 2]),
    ],
    root: 3,
  }
  let (result, drops) = decode_with_drop_counts(tree)
  guard result is Ok(Tuple([Secret(_), QuotaToken(_), Stream(_)])) else {
    fail("expected successful capability tuple decode, got \{result}")
  }
  assert_eq(drops, [0, 0, 0])
}

///|
test "decoded values release every resource still owned exactly once" {
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.SecretValue(@types.Secret::Secret(20)),
      @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(21)),
      @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(22)),
      @types.TupleValue([0, 1, 2]),
    ],
    root: 3,
  }
  let counts = [0, 0, 0]
  let drops : SchemaValueResourceDrops = {
    drop_secret: fn(_) { counts[0] += 1 },
    drop_quota_token: fn(_) { counts[1] += 1 },
    drop_stream: fn(_) { counts[2] += 1 },
    drop_native_stream: fn(_) {  },
  }
  let value = schema_value_from_wit(tree, drops)
  value.release_resources(drops)
  value.release_resources(drops)
  assert_eq(counts, [1, 1, 1])
}

///|
test "typed decode drops value resources when its graph is malformed" {
  let counts = [0, 0, 0]
  let drops : SchemaValueResourceDrops = {
    drop_secret: fn(_) { counts[0] += 1 },
    drop_quota_token: fn(_) { counts[1] += 1 },
    drop_stream: fn(_) { counts[2] += 1 },
    drop_native_stream: fn(_) {  },
  }
  let typed : @types.TypedSchemaValue = {
    graph: { type_nodes: [], root: 0, defs: [] },
    value: {
      value_nodes: [
        @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(12)),
      ],
      root: 0,
    },
  }
  let result = try? typed_schema_value_from_wit(typed, drops)
  guard result is Err(DecodeError(_)) else {
    fail("expected DecodeError, got \{result}")
  }
  assert_eq(counts, [0, 0, 1])
}
