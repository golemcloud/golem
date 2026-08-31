// Structural well-formedness checks for a `SchemaGraph`.
//
// A faithful MoonBit port of the Rust `golem-schema`
// `schema/validation/well_formedness.rs`. This is a pure in-memory pass over
// the recursive model (`model.mbt`) plus the generated `@types` leaf
// restriction structs — it never touches the flat WIT carriers and has no
// dependency on the trait/conversion layer (`schema/`).
//
// `validate_graph` collects ALL structural errors (deterministic Rust order:
// the duplicate-id pass first, then each def body in graph order, then the
// root; within a type in traversal order; for unions branch-local checks while
// walking branches, then pairwise ambiguity). `schema/into_schema_graph` runs
// this after building the graph and raises the first error, mirroring the Rust
// `try_into_schema_graph`.
//
// Deliberate deviations from the Rust reference (documented; MoonBit lacks the
// supporting engines, so these are left to host/server validation):
//   * regex *compile-validity* is not checked (no regex engine). Empty *text*
//     regex is accepted (Rust's `regex` crate accepts it); only an empty
//     *union* discriminator regex is rejected (a cheap structural check that
//     Rust also performs).
//   * quantity min>max / comparison-overflow are not checked: Rust uses checked
//     `i128` decimal rescaling, which an `Int64` approximation cannot match
//     without rejecting schemas Rust accepts. Only the cheap exact unit-mismatch
//     checks are ported.

///|
/// All structural errors that can be raised by `validate_graph`. Mirrors the
/// ported subset of the Rust `SchemaError` (it intentionally omits
/// `QuantityMinGreaterThanMax`, `QuantityComparisonOverflow`, and
/// `InvalidTextRegex` — see the file header for the rationale).
pub suberror SchemaError {
  DuplicateTypeId(TypeId)
  DanglingRef(TypeId)
  EmptyVariant
  EmptyEnum
  EmptyUnion
  EmptyFlags
  DuplicateFieldName(String)
  DuplicateVariantCase(String)
  DuplicateEnumCase(String)
  DuplicateFlagName(String)
  DuplicateUnionTag(String)
  MapKeyNotPrimitive
  FixedListZeroLength
  QuantityMinUnitMismatch(base_unit~ : String, min_unit~ : String)
  QuantityMaxUnitMismatch(base_unit~ : String, max_unit~ : String)
  UnionStringRuleOnNonStringBody(tag~ : String)
  UnionFieldRuleOnNonRecordBody(tag~ : String)
  UnionFieldEqualsLiteralOnNonStringField(tag~ : String, field_name~ : String)
  UnionFieldRuleMissingField(tag~ : String, field_name~ : String)
  UnionUnsatisfiableFieldAbsent(tag~ : String, field_name~ : String)
  UnionAmbiguousDiscriminators(
    tag_a~ : String,
    tag_b~ : String,
    reason~ : String
  )
  InvalidRegex(tag~ : String, regex~ : String, message~ : String)
  TextLengthRangeInverted
  BinaryByteRangeInverted
  /// An `Option<X>` was declared where `X` is itself nullable on the canonical
  /// JSON wire (option-of-option, option-of-union-with-nullable-branch,
  /// option-of-ref-resolving-to-nullable). `inner` is a short description.
  NullableNesting(inner~ : String)
} derive(Debug, Show, Eq)

// ============================================================
// Driver
// ============================================================

///|
/// Validate a `SchemaGraph` for structural well-formedness. Returns the full
/// list of collected errors in deterministic discovery order; an empty array
/// means the graph is well-formed.
pub fn validate_graph(graph : SchemaGraph) -> Array[SchemaError] {
  let errors : Array[SchemaError] = []

  // 1. duplicate type-id pass.
  let seen_ids : Map[TypeId, Unit] = Map::new()
  for def in graph.defs {
    if !set_insert(seen_ids, def.id) {
      errors.push(DuplicateTypeId(def.id))
    }
  }

  // 2. set of known ids (for dangling-ref detection).
  let known : Map[TypeId, Unit] = Map::new()
  for def in graph.defs {
    known[def.id] = ()
  }

  // 3. walk each def body (graph order), then the root.
  for def in graph.defs {
    check_type(graph, def.body, known, errors)
  }
  check_type(graph, graph.root, known, errors)
  errors
}

// ============================================================
// Helpers
// ============================================================

///|
/// Insert `key` into the set. Returns `true` if it was newly inserted, `false`
/// if it was already present (mirrors Rust `HashSet::insert`).
fn set_insert(set : Map[TypeId, Unit], key : TypeId) -> Bool {
  if set.contains(key) {
    false
  } else {
    set[key] = ()
    true
  }
}

///|
/// Resolve a named definition's body by id.
fn lookup_def(graph : SchemaGraph, id : TypeId) -> SchemaType? {
  for d in graph.defs {
    if d.id == id {
      return Some(d.body)
    }
  }
  None
}

// ============================================================
// Recursive type check
// ============================================================

///|
fn check_type(
  graph : SchemaGraph,
  ty : SchemaType,
  known : Map[TypeId, Unit],
  errors : Array[SchemaError],
) -> Unit {
  match ty.body {
    Ref(id) => if !known.contains(id) { errors.push(DanglingRef(id)) }
    Record(fields) => {
      let seen : Map[String, Unit] = Map::new()
      for f in fields {
        if !set_insert(seen, f.name) {
          errors.push(DuplicateFieldName(f.name))
        }
        check_type(graph, f.body, known, errors)
      }
    }
    Variant(cases) => {
      if cases.is_empty() {
        errors.push(EmptyVariant)
      }
      let seen : Map[String, Unit] = Map::new()
      for c in cases {
        if !set_insert(seen, c.name) {
          errors.push(DuplicateVariantCase(c.name))
        }
        match c.payload {
          Some(p) => check_type(graph, p, known, errors)
          None => ()
        }
      }
    }
    Enum(cases) => {
      if cases.is_empty() {
        errors.push(EmptyEnum)
      }
      let seen : Map[String, Unit] = Map::new()
      for c in cases {
        if !set_insert(seen, c) {
          errors.push(DuplicateEnumCase(c))
        }
      }
    }
    Flags(flags) => {
      if flags.is_empty() {
        errors.push(EmptyFlags)
      }
      let seen : Map[String, Unit] = Map::new()
      for fl in flags {
        if !set_insert(seen, fl) {
          errors.push(DuplicateFlagName(fl))
        }
      }
    }
    Tuple(elements) =>
      for e in elements {
        check_type(graph, e, known, errors)
      }
    List(element) => check_type(graph, element, known, errors)
    FixedList(element, length) => {
      if length == 0 {
        errors.push(FixedListZeroLength)
      }
      check_type(graph, element, known, errors)
    }
    Map(key, value) => {
      if !is_primitive_key_resolved(graph, key) {
        errors.push(MapKeyNotPrimitive)
      }
      check_type(graph, key, known, errors)
      check_type(graph, value, known, errors)
    }
    Option(inner) => {
      if is_nullable(graph, inner, Map::new()) {
        errors.push(NullableNesting(inner=describe_nullable(inner)))
      }
      check_type(graph, inner, known, errors)
    }
    Result(ok, err) => {
      match ok {
        Some(t) => check_type(graph, t, known, errors)
        None => ()
      }
      match err {
        Some(t) => check_type(graph, t, known, errors)
        None => ()
      }
    }
    Quantity(spec) => check_quantity(spec, errors)
    Text(restrictions) => check_text_restrictions(restrictions, errors)
    Binary(restrictions) => check_binary_restrictions(restrictions, errors)
    Union(branches) => validate_union(graph, branches, known, errors)
    Secret(spec) => check_type(graph, spec.inner, known, errors)
    Future(inner) =>
      match inner {
        Some(t) => check_type(graph, t, known, errors)
        None => ()
      }
    Stream(inner) =>
      match inner {
        Some(t) => check_type(graph, t, known, errors)
        None => ()
      }
    // Leaves with no structural constraints: primitives, Char, String,
    // Datetime, Duration, Path, Url, QuotaToken.
    _ => ()
  }
}

// ============================================================
// Map key
// ============================================================

///|
/// Whether `ty` resolves (through a cycle-aware ref chain) to a primitive type
/// accepted as a map key.
fn is_primitive_key_resolved(graph : SchemaGraph, ty : SchemaType) -> Bool {
  let visited : Map[TypeId, Unit] = Map::new()
  loop ty {
    cur =>
      match cur.body {
        Ref(id) =>
          if visited.contains(id) {
            break false
          } else {
            visited[id] = ()
            match lookup_def(graph, id) {
              Some(b) => continue b
              None => break false
            }
          }
        other => break is_primitive_key(other)
      }
  }
}

///|
fn is_primitive_key(body : SchemaTypeBody) -> Bool {
  match body {
    Bool
    | S8(_)
    | S16(_)
    | S32(_)
    | S64(_)
    | U8(_)
    | U16(_)
    | U32(_)
    | U64(_)
    | F32(_)
    | F64(_)
    | Char
    | String => true
    _ => false
  }
}

// ============================================================
// Nullable nesting
// ============================================================

///|
/// Whether `ty` can be encoded as JSON `null` on the canonical wire. True for
/// `Option`, a `Union` with any nullable branch, or a `Ref` resolving (cycle-
/// aware) to a nullable type. Re-entered ids terminate as non-nullable.
fn is_nullable(
  graph : SchemaGraph,
  ty : SchemaType,
  visited : Map[TypeId, Unit],
) -> Bool {
  match ty.body {
    Option(_) => true
    Union(branches) => {
      for b in branches {
        if is_nullable(graph, b.body, visited) {
          return true
        }
      }
      false
    }
    Ref(id) => {
      if visited.contains(id) {
        return false
      }
      visited[id] = ()
      let result = match lookup_def(graph, id) {
        Some(b) => is_nullable(graph, b, visited)
        None => false
      }
      visited.remove(id)
      result
    }
    _ => false
  }
}

///|
fn describe_nullable(ty : SchemaType) -> String {
  match ty.body {
    Option(_) => "option<_>"
    Union(_) => "union"
    Ref(id) => "ref `\{id}`"
    _ => "nullable"
  }
}

// ============================================================
// Scalar constraints
// ============================================================

///|
fn check_quantity(
  spec : @types.QuantitySpec,
  errors : Array[SchemaError],
) -> Unit {
  match spec.min {
    Some(min) =>
      if min.unit != spec.base_unit {
        errors.push(
          QuantityMinUnitMismatch(base_unit=spec.base_unit, min_unit=min.unit),
        )
      }
    None => ()
  }
  match spec.max {
    Some(max) =>
      if max.unit != spec.base_unit {
        errors.push(
          QuantityMaxUnitMismatch(base_unit=spec.base_unit, max_unit=max.unit),
        )
      }
    None => ()
  }
  // min>max / comparison-overflow are intentionally not checked here: Rust uses
  // checked i128 decimal rescaling, which an Int64 approximation cannot match
  // without rejecting schemas Rust accepts.
}

///|
fn check_text_restrictions(
  restrictions : @types.TextRestrictions,
  errors : Array[SchemaError],
) -> Unit {
  match (restrictions.min_length, restrictions.max_length) {
    (Some(min), Some(max)) =>
      if min > max {
        errors.push(TextLengthRangeInverted)
      }
    _ => ()
  }
  // regex compile-validity is not checked: MoonBit has no regex engine.
}

///|
fn check_binary_restrictions(
  restrictions : @types.BinaryRestrictions,
  errors : Array[SchemaError],
) -> Unit {
  match (restrictions.min_bytes, restrictions.max_bytes) {
    (Some(min), Some(max)) =>
      if min > max {
        errors.push(BinaryByteRangeInverted)
      }
    _ => ()
  }
}

// ============================================================
// Union
// ============================================================

///|
fn validate_union(
  graph : SchemaGraph,
  branches : Array[UnionBranch],
  known : Map[TypeId, Unit],
  errors : Array[SchemaError],
) -> Unit {
  if branches.is_empty() {
    errors.push(EmptyUnion)
  }
  let seen : Map[String, Unit] = Map::new()
  for branch in branches {
    if !set_insert(seen, branch.tag) {
      errors.push(DuplicateUnionTag(branch.tag))
    }
    check_union_branch(graph, branch, errors)
    check_type(graph, branch.body, known, errors)
  }

  // Pairwise discriminator ambiguity.
  for i in 0..<branches.length() {
    for j in (i + 1)..<branches.length() {
      let a = branches[i]
      let b = branches[j]
      match classify_discriminator_pair(a.discriminator, b.discriminator) {
        Reject(reason) =>
          errors.push(
            UnionAmbiguousDiscriminators(tag_a=a.tag, tag_b=b.tag, reason~),
          )
        Disjoint | Indeterminate => ()
      }
    }
  }
}

///|
fn check_union_branch(
  graph : SchemaGraph,
  branch : UnionBranch,
  errors : Array[SchemaError],
) -> Unit {
  let shape = resolved_shape(graph, branch.body, Map::new())
  match branch.discriminator {
    @types.Prefix(_) | @types.Suffix(_) | @types.Contains(_) =>
      if !(shape is StringShape) {
        errors.push(UnionStringRuleOnNonStringBody(tag=branch.tag))
      }
    @types.Regex(regex) => {
      if !(shape is StringShape) {
        errors.push(UnionStringRuleOnNonStringBody(tag=branch.tag))
      }
      if regex.is_empty() {
        errors.push(
          InvalidRegex(
            tag=branch.tag,
            regex~,
            message="regex must be non-empty",
          ),
        )
      }
      // non-empty regex compile-validity is not checked (no regex engine).
    }
    @types.FieldEquals(fd) =>
      match shape {
        RecordShape(fields) =>
          match find_field_type(fields, fd.field_name) {
            None =>
              errors.push(
                UnionFieldRuleMissingField(
                  tag=branch.tag,
                  field_name=fd.field_name,
                ),
              )
            Some(field_ty) =>
              match fd.literal {
                Some(_) =>
                  if !(resolved_shape(graph, field_ty, Map::new())
                    is StringShape) {
                    errors.push(
                      UnionFieldEqualsLiteralOnNonStringField(
                        tag=branch.tag,
                        field_name=fd.field_name,
                      ),
                    )
                  }
                None => ()
              }
          }
        _ => errors.push(UnionFieldRuleOnNonRecordBody(tag=branch.tag))
      }
    @types.FieldAbsent(field_name) =>
      match shape {
        RecordShape(fields) =>
          if field_present(fields, field_name) {
            errors.push(
              UnionUnsatisfiableFieldAbsent(tag=branch.tag, field_name~),
            )
          }
        _ => errors.push(UnionFieldRuleOnNonRecordBody(tag=branch.tag))
      }
  }
}

///|
/// Resolved structural shape of a union branch body, for discriminator checks.
priv enum BodyShape {
  StringShape
  RecordShape(Array[(String, SchemaType)])
  OtherShape
}

///|
fn resolved_shape(
  graph : SchemaGraph,
  ty : SchemaType,
  visited : Map[TypeId, Unit],
) -> BodyShape {
  match ty.body {
    Ref(id) => {
      if visited.contains(id) {
        return OtherShape
      }
      visited[id] = ()
      match lookup_def(graph, id) {
        Some(b) => resolved_shape(graph, b, visited)
        None => OtherShape
      }
    }
    String | Url(_) | Path(_) => StringShape
    Record(fields) => RecordShape(fields.map(f => (f.name, f.body)))
    _ => OtherShape
  }
}

///|
fn find_field_type(
  fields : Array[(String, SchemaType)],
  name : String,
) -> SchemaType? {
  for f in fields {
    if f.0 == name {
      return Some(f.1)
    }
  }
  None
}

///|
fn field_present(fields : Array[(String, SchemaType)], name : String) -> Bool {
  for f in fields {
    if f.0 == name {
      return true
    }
  }
  false
}

// ============================================================
// Discriminator overlap
// ============================================================

///|
priv enum DiscriminatorPairClassification {
  Reject(String)
  Disjoint
  Indeterminate
} derive(Eq, Debug)

///|
fn classify_discriminator_pair(
  a : @types.DiscriminatorRule,
  b : @types.DiscriminatorRule,
) -> DiscriminatorPairClassification {
  match (a, b) {
    (@types.Prefix(pa), @types.Prefix(pb)) =>
      if pa.has_prefix(pb) || pb.has_prefix(pa) {
        Reject("prefix `\{pa}` and prefix `\{pb}` overlap")
      } else {
        Disjoint
      }
    (@types.Suffix(sa), @types.Suffix(sb)) =>
      if sa.has_suffix(sb) || sb.has_suffix(sa) {
        Reject("suffix `\{sa}` and suffix `\{sb}` overlap")
      } else {
        Disjoint
      }
    (@types.Prefix(prefix), @types.Suffix(suffix))
    | (@types.Suffix(suffix), @types.Prefix(prefix)) =>
      Reject("prefix `\{prefix}` and suffix `\{suffix}` overlap")
    (@types.Prefix(prefix), @types.Contains(substring))
    | (@types.Contains(substring), @types.Prefix(prefix)) =>
      Reject("prefix `\{prefix}` and contains `\{substring}` overlap")
    (@types.Suffix(suffix), @types.Contains(substring))
    | (@types.Contains(substring), @types.Suffix(suffix)) =>
      Reject("suffix `\{suffix}` and contains `\{substring}` overlap")
    (@types.Contains(ca), @types.Contains(cb)) =>
      Reject("contains `\{ca}` and contains `\{cb}` overlap")
    (@types.Regex(ra), @types.Regex(rb)) =>
      if ra == rb {
        Reject("both branches share regex `\{ra}`")
      } else {
        Indeterminate
      }
    (@types.Regex(_), @types.Prefix(_) | @types.Suffix(_) | @types.Contains(_))
    | (@types.Prefix(_) | @types.Suffix(_) | @types.Contains(_), @types.Regex(_)
    ) => Indeterminate
    (
      @types.Prefix(_)
      | @types.Suffix(_)
      | @types.Contains(_)
      | @types.Regex(_),
      @types.FieldEquals(_)
      | @types.FieldAbsent(_),
    )
    | (
      @types.FieldEquals(_)
      | @types.FieldAbsent(_),
      @types.Prefix(_)
      | @types.Suffix(_)
      | @types.Contains(_)
      | @types.Regex(_),
    ) => Disjoint
    (@types.FieldEquals(fa), @types.FieldEquals(fb)) =>
      if fa.field_name != fb.field_name {
        Reject(
          "field-equals on `\{fa.field_name}` and `\{fb.field_name}` can match the same object",
        )
      } else {
        match (fa.literal, fb.literal) {
          (None, _) | (_, None) =>
            Reject(
              "field-equals on `\{fa.field_name}` without literal overlaps another field-equals on the same field",
            )
          (Some(la), Some(lb)) =>
            if la == lb {
              Reject(
                "two field-equals on `\{fa.field_name}` share literal `\{la}`",
              )
            } else {
              Disjoint
            }
        }
      }
    (@types.FieldAbsent(fa), @types.FieldAbsent(fb)) =>
      Reject("field-absent on `\{fa}` and `\{fb}` can match the same object")
    (@types.FieldEquals(field_equals), @types.FieldAbsent(field_absent))
    | (@types.FieldAbsent(field_absent), @types.FieldEquals(field_equals)) =>
      if field_equals.field_name == field_absent {
        Disjoint
      } else {
        Reject(
          "field-equals on `\{field_equals.field_name}` and field-absent on `\{field_absent}` can match the same object",
        )
      }
  }
}

///|
fn discriminator_classification_kind(
  value : DiscriminatorPairClassification,
) -> Int {
  match value {
    Reject(_) => 0
    Disjoint => 1
    Indeterminate => 2
  }
}

///|
test "discriminator_pair_classification_matches_portable_matrix" {
  let fe = (name, literal) => @types.FieldEquals({ field_name: name, literal, })
  let cases : Array[
    (String, @types.DiscriminatorRule, @types.DiscriminatorRule, Int),
  ] = [
    ("prefix_prefix_nested_reject", @types.Prefix("a"), @types.Prefix("ab"), 0),
    ("prefix_prefix_disjoint", @types.Prefix("a"), @types.Prefix("b"), 1),
    ("empty_prefix_prefix_reject", @types.Prefix(""), @types.Prefix("a"), 0),
    (
      "suffix_suffix_nested_reject",
      @types.Suffix("ing"),
      @types.Suffix("ng"),
      0,
    ),
    ("suffix_suffix_disjoint", @types.Suffix("a"), @types.Suffix("b"), 1),
    ("empty_suffix_suffix_reject", @types.Suffix(""), @types.Suffix("a"), 0),
    ("prefix_suffix_reject", @types.Prefix("a"), @types.Suffix("b"), 0),
    ("prefix_contains_reject", @types.Prefix("a"), @types.Contains("b"), 0),
    ("suffix_contains_reject", @types.Suffix("a"), @types.Contains("b"), 0),
    ("contains_contains_reject", @types.Contains("a"), @types.Contains("b"), 0),
    (
      "regex_regex_identical_reject",
      @types.Regex("a.*"),
      @types.Regex("a.*"),
      0,
    ),
    (
      "regex_regex_distinct_indeterminate",
      @types.Regex("a.*"),
      @types.Regex(".*a"),
      2,
    ),
    ("regex_prefix_indeterminate", @types.Regex("^a"), @types.Prefix("a"), 2),
    (
      "regex_empty_prefix_indeterminate",
      @types.Regex("^a"),
      @types.Prefix(""),
      2,
    ),
    ("regex_suffix_indeterminate", @types.Regex("a$"), @types.Suffix("a"), 2),
    (
      "regex_empty_suffix_indeterminate",
      @types.Regex("a$"),
      @types.Suffix(""),
      2,
    ),
    ("regex_contains_indeterminate", @types.Regex("a"), @types.Contains("a"), 2),
    (
      "regex_empty_contains_indeterminate",
      @types.Regex("a"),
      @types.Contains(""),
      2,
    ),
    (
      "prefix_field_equals_disjoint",
      @types.Prefix("a"),
      fe("kind", Some("a")),
      1,
    ),
    (
      "prefix_field_absent_disjoint",
      @types.Prefix("a"),
      @types.FieldAbsent("kind"),
      1,
    ),
    (
      "suffix_field_equals_disjoint",
      @types.Suffix("a"),
      fe("kind", Some("a")),
      1,
    ),
    (
      "suffix_field_absent_disjoint",
      @types.Suffix("a"),
      @types.FieldAbsent("kind"),
      1,
    ),
    (
      "contains_field_equals_disjoint",
      @types.Contains("a"),
      fe("kind", Some("a")),
      1,
    ),
    (
      "contains_field_absent_disjoint",
      @types.Contains("a"),
      @types.FieldAbsent("kind"),
      1,
    ),
    ("regex_field_equals_disjoint", @types.Regex("a"), fe("kind", Some("a")), 1),
    (
      "regex_field_absent_disjoint",
      @types.Regex("a"),
      @types.FieldAbsent("kind"),
      1,
    ),
    (
      "field_equals_same_field_different_literals_disjoint",
      fe("kind", Some("a")),
      fe("kind", Some("b")),
      1,
    ),
    (
      "field_equals_same_field_same_literal_reject",
      fe("kind", Some("a")),
      fe("kind", Some("a")),
      0,
    ),
    (
      "field_equals_same_field_one_literal_absent_reject",
      fe("kind", None),
      fe("kind", Some("a")),
      0,
    ),
    (
      "field_equals_same_field_both_literals_absent_reject",
      fe("kind", None),
      fe("kind", None),
      0,
    ),
    (
      "field_equals_different_fields_reject",
      fe("left", Some("a")),
      fe("right", Some("b")),
      0,
    ),
    (
      "field_absent_same_field_reject",
      @types.FieldAbsent("kind"),
      @types.FieldAbsent("kind"),
      0,
    ),
    (
      "field_absent_different_fields_reject",
      @types.FieldAbsent("left"),
      @types.FieldAbsent("right"),
      0,
    ),
    (
      "field_equals_field_absent_same_field_disjoint",
      fe("kind", Some("a")),
      @types.FieldAbsent("kind"),
      1,
    ),
    (
      "field_equals_field_absent_different_fields_reject",
      fe("left", Some("a")),
      @types.FieldAbsent("right"),
      0,
    ),
  ]
  for case in cases {
    assert_eq(
      discriminator_classification_kind(
        classify_discriminator_pair(case.1, case.2),
      ),
      case.3,
      msg="\{case.0} forward",
    )
    assert_eq(
      discriminator_classification_kind(
        classify_discriminator_pair(case.2, case.1),
      ),
      case.3,
      msg="\{case.0} reverse",
    )
  }
}
