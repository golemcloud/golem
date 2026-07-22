// Phase 2b tests for graph well-formedness (`validate_graph`). One per rejection
// rule, plus the cycle-aware map-key / nullable cases, the all-errors-collected
// ordering, and a well-formed graph that passes clean.

///|
fn vstr() -> SchemaType {
  schema_type(String)
}

///|
fn def(id : String, body : SchemaType) -> SchemaTypeDef {
  { id, name: None, body }
}

///|
fn branch(
  tag : String,
  body : SchemaType,
  discriminator : @types.DiscriminatorRule,
) -> UnionBranch {
  { tag, body, discriminator, metadata: empty_metadata() }
}

// ============================================================
// Well-formed graphs pass
// ============================================================

///|
test "well-formed record graph passes" {
  let rec = schema_type(
    Record([field("name", vstr()), field("age", schema_type(S32(None)))]),
  )
  let g : SchemaGraph = {
    defs: [def("Person", rec)],
    root: schema_type(Ref("Person")),
  }
  inspect(validate_graph(g).length(), content="0")
}

///|
test "map with primitive key resolved through a ref passes" {
  let g : SchemaGraph = {
    defs: [def("Key", vstr())],
    root: schema_type(Map(schema_type(Ref("Key")), vstr())),
  }
  inspect(validate_graph(g).length(), content="0")
}

// ============================================================
// Duplicate / dangling defs
// ============================================================

///|
test "duplicate type id" {
  let g : SchemaGraph = {
    defs: [def("a", vstr()), def("a", schema_type(S32(None)))],
    root: schema_type(Ref("a")),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[DuplicateTypeId(a)]
    ),
  )
}

///|
test "dangling ref" {
  let g : SchemaGraph = { defs: [], root: schema_type(Ref("missing")) }
  inspect(
    validate_graph(g),
    content=(
      #|[DanglingRef(missing)]
    ),
  )
}

// ============================================================
// Empty sums
// ============================================================

///|
test "empty variant" {
  let g : SchemaGraph = { defs: [], root: schema_type(Variant([])) }
  inspect(
    validate_graph(g),
    content=(
      #|[EmptyVariant]
    ),
  )
}

///|
test "empty enum" {
  let g : SchemaGraph = { defs: [], root: schema_type(Enum([])) }
  inspect(
    validate_graph(g),
    content=(
      #|[EmptyEnum]
    ),
  )
}

///|
test "empty flags" {
  let g : SchemaGraph = { defs: [], root: schema_type(Flags([])) }
  inspect(
    validate_graph(g),
    content=(
      #|[EmptyFlags]
    ),
  )
}

///|
test "empty union" {
  let g : SchemaGraph = { defs: [], root: schema_type(Union([])) }
  inspect(
    validate_graph(g),
    content=(
      #|[EmptyUnion]
    ),
  )
}

// ============================================================
// Duplicate names
// ============================================================

///|
test "duplicate field name" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(Record([field("x", vstr()), field("x", vstr())])),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[DuplicateFieldName(x)]
    ),
  )
}

///|
test "duplicate variant case" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(Variant([variant_case("a"), variant_case("a")])),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[DuplicateVariantCase(a)]
    ),
  )
}

///|
test "duplicate enum case" {
  let g : SchemaGraph = { defs: [], root: schema_type(Enum(["a", "a"])) }
  inspect(
    validate_graph(g),
    content=(
      #|[DuplicateEnumCase(a)]
    ),
  )
}

///|
test "duplicate flag name" {
  let g : SchemaGraph = { defs: [], root: schema_type(Flags(["f", "f"])) }
  inspect(
    validate_graph(g),
    content=(
      #|[DuplicateFlagName(f)]
    ),
  )
}

///|
test "duplicate union tag" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("t", vstr(), @types.Prefix("a")),
        branch("t", vstr(), @types.Prefix("b")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[DuplicateUnionTag(t)]
    ),
  )
}

// ============================================================
// Map key / fixed list
// ============================================================

///|
test "map key not primitive (structural key)" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(Map(schema_type(List(vstr())), vstr())),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[MapKeyNotPrimitive]
    ),
  )
}

///|
test "map key not primitive (self-recursive ref chain)" {
  let g : SchemaGraph = {
    defs: [def("a", schema_type(Ref("a")))],
    root: schema_type(Map(schema_type(Ref("a")), vstr())),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[MapKeyNotPrimitive]
    ),
  )
}

///|
test "fixed list zero length" {
  let g : SchemaGraph = { defs: [], root: schema_type(FixedList(vstr(), 0)) }
  inspect(
    validate_graph(g),
    content=(
      #|[FixedListZeroLength]
    ),
  )
}

// ============================================================
// Scalar restriction ranges
// ============================================================

///|
test "text length range inverted" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Text({
        languages: None,
        min_length: Some(5U),
        max_length: Some(2U),
        regex: None,
      }),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[TextLengthRangeInverted]
    ),
  )
}

///|
test "binary byte range inverted" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Binary({ mime_types: None, min_bytes: Some(5U), max_bytes: Some(2U) }),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[BinaryByteRangeInverted]
    ),
  )
}

// ============================================================
// Nullable nesting
// ============================================================

///|
test "nullable nesting option of option" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(Option(schema_type(Option(vstr())))),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[NullableNesting(inner=option<_>)]
    ),
  )
}

///|
test "nullable nesting option of ref to nullable" {
  let g : SchemaGraph = {
    defs: [def("a", schema_type(Option(vstr())))],
    root: schema_type(Option(schema_type(Ref("a")))),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[NullableNesting(inner=ref `a`)]
    ),
  )
}

///|
test "nullable nesting option of union with nullable branch" {
  // The nullable (Option) branch body is neither string- nor record-shaped, so
  // the union also reports a shape error — both are collected.
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Option(
        schema_type(
          Union([
            branch("t", schema_type(Option(vstr())), @types.FieldAbsent("x")),
          ]),
        ),
      ),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[NullableNesting(inner=union), UnionFieldRuleOnNonRecordBody(tag=t)]
    ),
  )
}

// ============================================================
// Quantity unit mismatch
// ============================================================

///|
test "quantity min unit mismatch" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Quantity({
        base_unit: "m",
        allowed_suffixes: [],
        min: Some({ mantissa: 0L, scale: 0, unit: "cm" }),
        max: None,
      }),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[QuantityMinUnitMismatch(base_unit=m, min_unit=cm)]
    ),
  )
}

// ============================================================
// Union discriminator shape checks
// ============================================================

///|
test "union string rule on non-string body" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([branch("t", schema_type(S32(None)), @types.Prefix("a"))]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionStringRuleOnNonStringBody(tag=t)]
    ),
  )
}

///|
test "union field rule on non-record body" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch(
          "t",
          vstr(),
          @types.FieldEquals({ field_name: "x", literal: None }),
        ),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionFieldRuleOnNonRecordBody(tag=t)]
    ),
  )
}

///|
test "union field rule missing field" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch(
          "t",
          schema_type(Record([field("y", vstr())])),
          @types.FieldEquals({ field_name: "x", literal: None }),
        ),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionFieldRuleMissingField(tag=t, field_name=x)]
    ),
  )
}

///|
test "union field-equals literal on non-string field" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch(
          "t",
          schema_type(Record([field("x", schema_type(S32(None)))])),
          @types.FieldEquals({ field_name: "x", literal: Some("v") }),
        ),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionFieldEqualsLiteralOnNonStringField(tag=t, field_name=x)]
    ),
  )
}

///|
test "union unsatisfiable field absent" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch(
          "t",
          schema_type(Record([field("x", vstr())])),
          @types.FieldAbsent("x"),
        ),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionUnsatisfiableFieldAbsent(tag=t, field_name=x)]
    ),
  )
}

///|
test "union empty regex" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(Union([branch("t", vstr(), @types.Regex(""))])),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[InvalidRegex(tag=t, regex=, message=regex must be non-empty)]
    ),
  )
}

// ============================================================
// Union pairwise ambiguity
// ============================================================

///|
test "union ambiguous discriminators (prefix overlap)" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Prefix("ab")),
        branch("b", vstr(), @types.Prefix("a")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=prefix `ab` and prefix `a` overlap)]
    ),
  )
}

// ============================================================
// All errors collected, in discovery order
// ============================================================

///|
test "multiple errors collected in discovery order" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Record([field("x", schema_type(Ref("missing"))), field("x", vstr())]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[DanglingRef(missing), DuplicateFieldName(x)]
    ),
  )
}

///|
fn rec1(name : String, body : SchemaType) -> SchemaType {
  schema_type(Record([field(name, body)]))
}

// ============================================================
// Pairwise discriminator ambiguity — full matrix
// ============================================================

///|
test "ambiguity: suffix overlap" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Suffix("ing")),
        branch("b", vstr(), @types.Suffix("ng")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=suffix `ing` and suffix `ng` overlap)]
    ),
  )
}

///|
test "ambiguity: empty contains (same constructor)" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Contains("")),
        branch("b", vstr(), @types.Contains("x")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=empty contains substring matches every string)]
    ),
  )
}

///|
test "ambiguity: equal regex" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Regex("x")),
        branch("b", vstr(), @types.Regex("x")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=both branches share regex `x`)]
    ),
  )
}

///|
test "ambiguity: field-equals same field without literal" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch(
          "a",
          rec1("k", vstr()),
          @types.FieldEquals({ field_name: "k", literal: None }),
        ),
        branch(
          "b",
          rec1("k", vstr()),
          @types.FieldEquals({ field_name: "k", literal: None }),
        ),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=field-equals on `k` without literal overlaps another field-equals on the same field)]
    ),
  )
}

///|
test "ambiguity: field-equals same literal" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch(
          "a",
          rec1("k", vstr()),
          @types.FieldEquals({ field_name: "k", literal: Some("v") }),
        ),
        branch(
          "b",
          rec1("k", vstr()),
          @types.FieldEquals({ field_name: "k", literal: Some("v") }),
        ),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=two field-equals on `k` share literal `v`)]
    ),
  )
}

///|
test "ambiguity: field-absent same field" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", rec1("y", vstr()), @types.FieldAbsent("x")),
        branch("b", rec1("y", vstr()), @types.FieldAbsent("x")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=two field-absent rules share field `x`)]
    ),
  )
}

///|
test "ambiguity: mixed empty prefix vs suffix (fallthrough)" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Prefix("")),
        branch("b", vstr(), @types.Suffix("")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=empty prefix matches every string)]
    ),
  )
}

///|
test "ambiguity: mixed empty suffix vs contains (fallthrough)" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Suffix("")),
        branch("b", vstr(), @types.Contains("")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=empty suffix matches every string)]
    ),
  )
}

///|
test "ambiguity: mixed empty contains vs regex (fallthrough)" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Contains("")),
        branch("b", vstr(), @types.Regex("x")),
      ]),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[UnionAmbiguousDiscriminators(tag_a=a, tag_b=b, reason=empty contains substring matches every string)]
    ),
  )
}

///|
test "no ambiguity: distinct non-empty rules of different kinds" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Union([
        branch("a", vstr(), @types.Prefix("p")),
        branch(
          "b",
          rec1("k", vstr()),
          @types.FieldEquals({ field_name: "k", literal: Some("v") }),
        ),
      ]),
    ),
  }
  inspect(validate_graph(g).length(), content="0")
}

// ============================================================
// Resolver edge cases
// ============================================================

///|
test "map key primitive through a multi-ref chain passes" {
  let g : SchemaGraph = {
    defs: [def("a", schema_type(Ref("b"))), def("b", vstr())],
    root: schema_type(Map(schema_type(Ref("a")), vstr())),
  }
  inspect(validate_graph(g).length(), content="0")
}

///|
test "dangling map key reports both not-primitive and dangling" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(Map(schema_type(Ref("missing")), vstr())),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[MapKeyNotPrimitive, DanglingRef(missing)]
    ),
  )
}

///|
test "option of cyclic non-nullable ref is well-formed (no hang, no false nullable)" {
  // `a` is a record that references itself — recursive but NOT nullable.
  let g : SchemaGraph = {
    defs: [def("a", rec1("self", schema_type(Ref("a"))))],
    root: schema_type(Option(schema_type(Ref("a")))),
  }
  inspect(validate_graph(g).length(), content="0")
}

///|
test "resolved_shape through refs (string + record) is well-formed" {
  let g : SchemaGraph = {
    defs: [def("s", vstr()), def("rec", rec1("k", vstr()))],
    root: schema_type(
      Union([
        branch("a", schema_type(Ref("s")), @types.Prefix("p")),
        branch(
          "b",
          schema_type(Ref("rec")),
          @types.FieldEquals({ field_name: "k", literal: None }),
        ),
      ]),
    ),
  }
  inspect(validate_graph(g).length(), content="0")
}

// ============================================================
// Quantity max unit mismatch
// ============================================================

///|
test "quantity max unit mismatch" {
  let g : SchemaGraph = {
    defs: [],
    root: schema_type(
      Quantity({
        base_unit: "m",
        allowed_suffixes: [],
        min: None,
        max: Some({ mantissa: 0L, scale: 0, unit: "km" }),
      }),
    ),
  }
  inspect(
    validate_graph(g),
    content=(
      #|[QuantityMaxUnitMismatch(base_unit=m, max_unit=km)]
    ),
  )
}
