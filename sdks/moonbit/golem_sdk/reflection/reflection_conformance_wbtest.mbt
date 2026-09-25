///|
fn corpus_object(value : Json) -> Map[String, Json] raise {
  match value {
    Object(fields) => fields
    _ => fail("expected conformance JSON object")
  }
}

///|
fn corpus_array(value : Json) -> Array[Json] raise {
  match value {
    Array(values) => values
    _ => fail("expected conformance JSON array")
  }
}

///|
fn corpus_string(value : Json) -> String raise {
  match value {
    String(value) => value
    _ => fail("expected conformance JSON string")
  }
}

///|
fn corpus_count(value : Json) -> Int raise {
  match value {
    Number(value, ..) => value.to_int()
    _ => fail("expected conformance JSON number")
  }
}

///|
fn corpus_bool(value : Json) -> Bool raise {
  match value {
    True => true
    False => false
    _ => fail("expected conformance JSON boolean")
  }
}

///|
fn corpus_field(
  name : String,
  body : @model.SchemaType,
) -> @model.NamedFieldType {
  { name, body, metadata: @model.empty_metadata(), }
}

///|
fn conformance_fixture(name : String, id : String) -> SchemaRef raise {
  let root = match name {
    "s64" => @model.schema_type(S64(None))
    "constrained-s64" =>
      @model.schema_type(
        S64(
          Some({
            min: Some(@types.Signed(-9007199254740993L)),
            max: Some(@types.Signed(9007199254740993L)),
            unit: None,
          }),
        ),
      )
    "u64" => @model.schema_type(U64(None))
    "binary" =>
      @model.schema_type(
        Binary({ mime_types: None, min_bytes: None, max_bytes: None, }),
      )
    "duration" => @model.schema_type(Duration)
    "quantity" =>
      @model.schema_type(
        Quantity({ base_unit: "m", allowed_suffixes: [], min: None, max: None, }),
      )
    "tool-input" =>
      @model.schema_type(
        Record([
          corpus_field("pattern", @model.schema_type(String)),
          corpus_field(
            "paths",
            @model.schema_type(List(@model.schema_type(String))),
          ),
          corpus_field(
            "ignoreCase",
            @model.schema_type(Option(@model.schema_type(Bool))),
          ),
        ]),
      )
    "config-entry" =>
      @model.schema_type(
        Record([
          corpus_field(
            "path",
            @model.schema_type(List(@model.schema_type(String))),
          ),
          corpus_field("value", @model.schema_type(S64(None))),
        ]),
      )
    "constrained-u32" =>
      @model.schema_type(
        U32(
          Some({
            min: Some(@types.Unsigned(2UL)),
            max: Some(@types.Unsigned(10UL)),
            unit: None,
          }),
        ),
      )
    "constrained-f64" =>
      @model.schema_type(
        F64(
          Some({
            min: Some(@types.FloatBits((-1.5).reinterpret_as_uint64())),
            max: Some(@types.FloatBits(2.5.reinterpret_as_uint64())),
            unit: None,
          }),
        ),
      )
    "constrained-text" =>
      @model.schema_type(
        Text({
          languages: Some(["en", "de"]),
          min_length: Some(2U),
          max_length: Some(8U),
          regex: Some("^[a-z]+$"),
        }),
      )
    "constrained-binary" =>
      @model.schema_type(
        Binary({
          mime_types: Some(["image/png", "application/octet-stream"]),
          min_bytes: Some(2U),
          max_bytes: Some(4U),
        }),
      )
    "result" =>
      @model.schema_type(
        Result(
          Some(@model.schema_type(String)),
          Some(@model.schema_type(U32(None))),
        ),
      )
    "custom-error" =>
      @model.schema_type(
        Result(
          Some(@model.schema_type(String)),
          Some(
            @model.schema_type(
              Record([
                corpus_field("code", @model.schema_type(String)),
                corpus_field("retryable", @model.schema_type(Bool)),
              ]),
            ),
          ),
        ),
      )
    "optional-record" => {
      let optional = @model.schema_type(Option(@model.schema_type(String)))
      return SchemaRef::new({
        defs: [{ id: "conformance.optional", name: None, body: optional, }],
        root: @model.schema_type(
          Record([
            corpus_field("direct", optional),
            corpus_field(
              "referenced",
              @model.schema_type(Ref("conformance.optional")),
            ),
          ]),
        ),
      })
    }
    other => fail(id + ": unknown conformance fixture " + other)
  }
  SchemaRef::new({ defs: [], root, })
}

///|
fn conformance_at_pointer(
  value : Json,
  pointer : String,
  id : String,
) -> Json raise {
  match pointer {
    "" => value
    "/required" => corpus_object(value).get("required").unwrap()
    "/properties/nanoseconds" =>
      corpus_object(corpus_object(value).get("properties").unwrap())
      .get("nanoseconds")
      .unwrap()
    "/properties/mantissa" =>
      corpus_object(corpus_object(value).get("properties").unwrap())
      .get("mantissa")
      .unwrap()
    other => fail(id + ": unsupported conformance JSON pointer " + other)
  }
}

///|
fn assert_conformance_subset(
  actual : Json,
  expected : Json,
  id : String,
) -> Unit raise {
  match expected {
    Object(expected_fields) => {
      let actual_fields = corpus_object(actual)
      for entry in expected_fields {
        let (name, expected_value) = entry
        let actual_value = match actual_fields.get(name) {
          Some(value) => value
          None => fail(id + ": missing conformance JSON field " + name)
        }
        assert_conformance_subset(actual_value, expected_value, id)
      }
    }
    _ => {
      guard actual == expected else { fail(id + ": JSON values differ") }
    }
  }
}

///|
fn assert_conformance_semantic(
  name : String,
  expected : Json,
  corpus : Json,
  id : String,
) -> Unit raise {
  match name {
    "unsupported-leaves" => {
      let types = [
        @model.SchemaTypeBody::Secret({
          inner: @model.schema_type(String),
          category: None,
        }),
        @model.SchemaTypeBody::QuotaToken({ resource_name: None, }),
        @model.SchemaTypeBody::PermissionCard({ polymorphic: false, }),
        @model.SchemaTypeBody::Future(None),
        @model.SchemaTypeBody::Stream(None),
      ]
      let expected_fields = corpus_object(expected)
      guard types.length() ==
        corpus_count(expected_fields.get("count").unwrap()) else {
        fail(id + ": unsupported-leaf count differs")
      }
      for body in types {
        assert_conformance_subset(
          test_schema(body).to_json_schema(include_draft_marker=false),
          expected_fields.get("schema").unwrap(),
          id,
        )
      }
    }
    "all-kinds" => {
      let corpus_fields = corpus_object(corpus)
      let supported = [
        "ref", "bool", "s8", "s16", "s32", "s64", "u8", "u16", "u32", "u64", "f32",
        "f64", "char", "string", "record", "variant", "enum", "flags", "tuple", "list",
        "fixed-list", "map", "option", "result", "text", "binary", "path", "url",
        "datetime", "duration", "quantity", "union", "secret", "quota-token", "permission-card",
        "future", "stream",
      ]
      let expected_names = corpus_array(
        corpus_object(expected).get("names").unwrap(),
      ).map(corpus_string)
      guard supported == expected_names else {
        fail(id + ": supported schema kinds differ")
      }
      guard corpus_array(corpus_fields.get("schemaKinds").unwrap()).map(
          corpus_string,
        ) ==
        expected_names else {
        fail(id + ": declared schema kinds differ")
      }
    }
    "all-restrictions" => {
      let corpus_fields = corpus_object(corpus)
      let supported = [
        "numeric-minimum", "numeric-maximum", "numeric-unit", "text-languages", "text-min-length",
        "text-max-length", "text-regex", "binary-mime-types", "binary-min-bytes",
        "binary-max-bytes", "path-direction", "path-kind", "path-mime-types", "path-extensions",
        "url-schemes", "url-hosts", "quantity-base-unit", "quantity-suffixes", "quantity-minimum",
        "quantity-maximum", "union-prefix", "union-suffix", "union-regex", "union-field",
      ]
      let expected_names = corpus_array(
        corpus_object(expected).get("names").unwrap(),
      ).map(corpus_string)
      guard supported == expected_names else {
        fail(id + ": supported restriction kinds differ")
      }
      guard corpus_array(corpus_fields.get("restrictionKinds").unwrap()).map(
          corpus_string,
        ) ==
        expected_names else {
        fail(id + ": declared restriction kinds differ")
      }
    }
    "graph" => {
      let expected_fields = corpus_object(expected)
      let referenced = conformance_fixture("optional-record", id)
      let inline = test_schema(
        Record([
          corpus_field(
            "direct",
            @model.schema_type(Option(@model.schema_type(String))),
          ),
          corpus_field(
            "referenced",
            @model.schema_type(Option(@model.schema_type(String))),
          ),
        ]),
      )
      let valid = @model.validate_graph(referenced.graph()).length() == 0
      let equivalent = referenced.pack_json(Json::object({})) ==
        inline.pack_json(Json::object({}))
      guard valid == corpus_bool(expected_fields.get("valid").unwrap()) else {
        fail(id + ": graph validity differs")
      }
      guard equivalent ==
        corpus_bool(expected_fields.get("equivalent").unwrap()) else {
        fail(id + ": graph equivalence differs")
      }
    }
    other => fail(id + ": unknown semantic conformance fixture " + other)
  }
}

///|
fn conformance_reject_kind(
  schema : SchemaRef,
  input : Json,
  id : String,
) -> String raise {
  let packed = try schema.pack(schema.resolve(schema.root), input) catch {
    _ => None
  } noraise {
    value => Some(value)
  }
  match packed {
    None => "invalid-json"
    Some(value) =>
      try schema.validate_value(value) catch {
        _ => "constraint-violation"
      } noraise {
        _ => fail("conformance case was accepted: " + id)
      }
  }
}

///|
fn run_conformance_operation(operation : String) -> Unit raise {
  let corpus = @json.parse(reflection_conformance_corpus_json)
  let root = corpus_object(corpus)
  for test_case_json in corpus_array(root.get("cases").unwrap()) {
    let test_case = corpus_object(test_case_json)
    let id = corpus_string(test_case.get("id").unwrap())
    if corpus_string(test_case.get("operation").unwrap()) == operation {
      let name = corpus_string(test_case.get("fixture").unwrap())
      match operation {
        "roundtrip" => {
          let schema = conformance_fixture(name, id)
          let actual = schema.unpack_json(
            schema.pack_json(test_case.get("input").unwrap()),
          ) catch {
            error => fail(id + ": " + repr(error))
          }
          guard actual == test_case.get("expected").unwrap() else {
            fail(id + ": roundtrip output differs")
          }
        }
        "reject" => {
          let schema = conformance_fixture(name, id)
          let inputs = match test_case.get("inputs") {
            Some(values) => corpus_array(values)
            None => [test_case.get("input").unwrap()]
          }
          for input in inputs {
            let actual = conformance_reject_kind(schema, input, id)
            let expected = corpus_string(
              corpus_object(test_case.get("expected").unwrap())
              .get("kind")
              .unwrap(),
            )
            guard actual == expected else {
              fail(id + ": rejection category differs")
            }
          }
        }
        "json-schema" =>
          assert_conformance_subset(
            conformance_at_pointer(
              conformance_fixture(name, id).to_json_schema(
                include_draft_marker=false,
              ),
              corpus_string(test_case.get("path").unwrap()),
              id,
            ),
            test_case.get("expected").unwrap(),
            id,
          )
        "semantic" =>
          assert_conformance_semantic(
            name,
            test_case.get("expected").unwrap(),
            corpus,
            id,
          )
        other => fail("unknown requested conformance operation " + other)
      }
    }
  }
}

///|
test "reflection conformance corpus integrity" {
  let corpus = @json.parse(reflection_conformance_corpus_json)
  let root = corpus_object(corpus)
  guard corpus_string(root.get("version").unwrap()) == "1.0.0" else {
    fail("unexpected reflection conformance corpus version")
  }
  let executed : Array[String] = []
  let recognized = ["roundtrip", "reject", "json-schema", "semantic"]
  for test_case_json in corpus_array(root.get("cases").unwrap()) {
    let test_case = corpus_object(test_case_json)
    let id = corpus_string(test_case.get("id").unwrap())
    guard !executed.any(value => value == id) else {
      fail("duplicate conformance case ID " + id)
    }
    executed.push(id)
    let operation = corpus_string(test_case.get("operation").unwrap())
    guard recognized.any(value => value == operation) else {
      fail(id + ": unknown conformance operation " + operation)
    }
  }
  let declared = corpus_array(root.get("caseIds").unwrap()).map(corpus_string)
  guard executed.length() == declared.length() else {
    fail("declared conformance case count differs")
  }
  let declared_seen : Array[String] = []
  for id in declared {
    guard !declared_seen.any(value => value == id) else {
      fail("duplicate declared conformance case ID " + id)
    }
    declared_seen.push(id)
    guard executed.any(value => value == id) else {
      fail("undeclared conformance case ID " + id)
    }
  }
}

///|
test "reflection conformance roundtrip cases" {
  run_conformance_operation("roundtrip")
}

///|
test "reflection conformance rejection cases" {
  run_conformance_operation("reject")
}

///|
test "reflection conformance JSON Schema cases" {
  run_conformance_operation("json-schema")
}

///|
test "reflection conformance semantic cases" {
  run_conformance_operation("semantic")
}
