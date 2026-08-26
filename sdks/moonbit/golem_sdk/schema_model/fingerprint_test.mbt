///|
fn empty_graph() -> SchemaGraph {
  { defs: [], root: schema_type(String) }
}

///|
fn assert_vector(
  graph : SchemaGraph,
  element : SchemaType?,
  length : Int,
  digest : String,
) -> Unit raise {
  assert_eq(canonical_schema_bytes_v1(graph, element).length(), length)
  assert_eq(schema_fingerprint_v1(graph, element).to_hex(), digest)
}

///|
test "SchemaFingerprintV1 golden vectors" {
  let graph = empty_graph()
  assert_vector(
    graph,
    None,
    37,
    "b50494cf0f33961c703d5f6e6af3d3159e528c4d09c1d801172cdf8f022dcafa",
  )
  let string = schema_type(String)
  assert_vector(
    graph,
    Some(string),
    37,
    "61c50c0a3c6ffd63529621ada78afc0d4d8e5fe691f8b0993035f847c660a307",
  )
  assert_vector(
    graph,
    Some(schema_type(List(string))),
    45,
    "4939707f8ef97e9d4b31b568332eaf5a3011f2be7c358f7546966fadfb9416d4",
  )

  let node_ref = schema_type(Ref("example.node"))
  let recursive : SchemaGraph = {
    defs: [
      SchemaTypeDef::{
        id: "example.node",
        name: Some("Node"),
        body: schema_type(
          Record([
            field("value", string),
            field("next", schema_type(Option(node_ref))),
          ]),
        ),
      },
    ],
    root: node_ref,
  }
  assert_vector(
    recursive,
    Some(recursive.root),
    140,
    "3931585d2d02a2b7d5c99e3da1082ac8fe904c535e2700bd45e29a95ff2399fa",
  )

  let constrained = schema_type(
    Text({
      languages: Some(["fr", "en"]),
      min_length: Some(1),
      max_length: Some(64),
      regex: Some("^[a-z]+$"),
    }),
    metadata={
      doc: Some("text"),
      aliases: ["z", "a"],
      examples: ["\"hello\""],
      deprecated: Some("use-v2"),
      role: Some(@types.Other("prompt")),
    },
  )
  assert_vector(
    graph,
    Some(constrained),
    87,
    "b985cdb5445862be90e8dca06bbfa9c46b50cf40edc84ed34205bb3a214c5bb0",
  )
  let card_spec : @types.PermissionCardSpec = { polymorphic: true }
  assert_vector(
    graph,
    Some(schema_type(PermissionCard(card_spec))),
    39,
    "b7d3c09af5db4e56b527051f561689f451dfdb21213cd70aabd11618e244da8b",
  )
}

///|
test "SchemaFingerprintV1 canonical constrained text bytes" {
  let constrained = schema_type(
    Text({
      languages: Some(["fr", "en"]),
      min_length: Some(1),
      max_length: Some(64),
      regex: Some("^[a-z]+$"),
    }),
    metadata={
      doc: Some("text"),
      aliases: ["z", "a"],
      examples: ["\"hello\""],
      deprecated: Some("use-v2"),
      role: Some(@types.Other("prompt")),
    },
  )
  let bytes = canonical_schema_bytes_v1(empty_graph(), Some(constrained))
  let hex = StringBuilder::new(size_hint=bytes.length() * 2)
  let digits = "0123456789abcdef"
  for byte in bytes {
    hex.write_char(
      digits.code_unit_at((byte.to_int() >> 4) & 15).to_int().unsafe_to_char(),
    )
    hex.write_char(
      digits.code_unit_at(byte.to_int() & 15).to_int().unsafe_to_char(),
    )
  }
  assert_eq(
    hex.to_string(),
    "847818676f6c656d2d736368656d612d66696e6765727072696e74018618198262656e626672011840685e5b612d7a5d2b24856474657874826161617a81672268656c6c6f22667573652d763282036670726f6d707480",
  )
}

///|
test "set restrictions normalize and reject duplicates" {
  let left = schema_type(
    Text({
      languages: Some(["fr", "en"]),
      min_length: None,
      max_length: None,
      regex: None,
    }),
  )
  let right = schema_type(
    Text({
      languages: Some(["en", "fr"]),
      min_length: None,
      max_length: None,
      regex: None,
    }),
  )
  assert_eq(
    schema_fingerprint_v1(empty_graph(), Some(left)),
    schema_fingerprint_v1(empty_graph(), Some(right)),
  )
  let duplicate = schema_type(
    Text({
      languages: Some(["en", "en"]),
      min_length: None,
      max_length: None,
      regex: None,
    }),
  )
  try schema_fingerprint_v1(empty_graph(), Some(duplicate)) catch {
    DuplicateSetValue(..) => ()
    error => fail("unexpected error: \{error}")
  } noraise {
    _ => fail("expected duplicate set value error")
  }
}
