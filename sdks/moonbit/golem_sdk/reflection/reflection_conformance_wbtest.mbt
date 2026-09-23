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
fn corpus_field(
  name : String,
  body : @model.SchemaType,
) -> @model.NamedFieldType {
  { name, body, metadata: @model.empty_metadata(), }
}

///|
fn conformance_fixture(name : String) -> SchemaRef raise {
  let root = match name {
    "s64" => @model.schema_type(S64(None))
    "u64" => @model.schema_type(U64(None))
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
    "constrained-u32" =>
      @model.schema_type(
        U32(Some({ min: None, max: Some(@types.Unsigned(10UL)), unit: None, })),
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
    other => fail("unknown conformance fixture " + other)
  }
  SchemaRef::new({ defs: [], root, })
}

///|
fn conformance_at_pointer(value : Json, pointer : String) -> Json raise {
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
    other => fail("unsupported conformance JSON pointer " + other)
  }
}

///|
fn assert_conformance_subset(actual : Json, expected : Json) -> Unit raise {
  match expected {
    Object(expected_fields) => {
      let actual_fields = corpus_object(actual)
      for entry in expected_fields {
        let (name, expected_value) = entry
        let actual_value = match actual_fields.get(name) {
          Some(value) => value
          None => fail("missing conformance JSON field " + name)
        }
        assert_conformance_subset(actual_value, expected_value)
      }
    }
    _ => assert_eq(actual, expected)
  }
}

///|
fn assert_conformance_semantic(
  name : String,
  expected : Json,
  corpus : Json,
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
      assert_eq(
        types.length(),
        corpus_count(expected_fields.get("count").unwrap()),
      )
      for body in types {
        assert_conformance_subset(
          test_schema(body).to_json_schema(include_draft_marker=false),
          expected_fields.get("schema").unwrap(),
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
      assert_eq(supported, expected_names)
      assert_eq(
        corpus_array(corpus_fields.get("schemaKinds").unwrap()).map(
          corpus_string,
        ),
        expected_names,
      )
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
      assert_eq(supported, expected_names)
      assert_eq(
        corpus_array(corpus_fields.get("restrictionKinds").unwrap()).map(
          corpus_string,
        ),
        expected_names,
      )
    }
    "graph" => {
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
      assert_eq(
        conformance_fixture("optional-record").pack_json(Json::object({})),
        inline.pack_json(Json::object({})),
      )
    }
    other => fail("unknown semantic conformance fixture " + other)
  }
}

///|
test "reflection conformance corpus executes the complete case-ID set" {
  let corpus = @json.parse(reflection_conformance_corpus_json)
  let root = corpus_object(corpus)
  assert_eq(corpus_string(root.get("version").unwrap()), "1.0.0")
  let executed : Array[String] = []
  for test_case_json in corpus_array(root.get("cases").unwrap()) {
    let test_case = corpus_object(test_case_json)
    let id = corpus_string(test_case.get("id").unwrap())
    guard !executed.any(value => value == id) else {
      fail("duplicate conformance case ID " + id)
    }
    executed.push(id)
    let name = corpus_string(test_case.get("fixture").unwrap())
    match corpus_string(test_case.get("operation").unwrap()) {
      "roundtrip" => {
        let schema = conformance_fixture(name)
        let packed = schema.pack_json(test_case.get("input").unwrap())
        assert_eq(
          schema.unpack_json(packed),
          test_case.get("expected").unwrap(),
        )
      }
      "reject" =>
        try
          conformance_fixture(name).pack_json(test_case.get("input").unwrap())
        catch {
          _ => ()
        } noraise {
          _ => fail("conformance case was accepted: " + id)
        }
      "json-schema" =>
        assert_conformance_subset(
          conformance_at_pointer(
            conformance_fixture(name).to_json_schema(include_draft_marker=false),
            corpus_string(test_case.get("path").unwrap()),
          ),
          test_case.get("expected").unwrap(),
        )
      "semantic" =>
        assert_conformance_semantic(
          name,
          test_case.get("expected").unwrap(),
          corpus,
        )
      operation =>
        fail("unknown conformance operation " + operation + " for " + id)
    }
  }
  let declared = corpus_array(root.get("caseIds").unwrap()).map(corpus_string)
  assert_eq(executed.length(), declared.length())
  for id in declared {
    assert_true(executed.any(value => value == id))
  }
}
