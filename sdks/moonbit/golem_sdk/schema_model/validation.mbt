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
} derive(Show, Eq)

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
    | S8
    | S16
    | S32
    | S64
    | U8
    | U16
    | U32
    | U64
    | F32
    | F64
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
      match discriminators_overlap(a.discriminator, b.discriminator) {
        Some(reason) =>
          errors.push(
            UnionAmbiguousDiscriminators(tag_a=a.tag, tag_b=b.tag, reason~),
          )
        None => ()
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
    String | Text(_) | Url(_) | Path(_) => StringShape
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
/// Returns `Some(reason)` if rules `a` and `b` can structurally overlap on at
/// least one value, `None` otherwise. Regex overlap is undecidable in general;
/// only byte-equal patterns are reported. Mirrors Rust `discriminators_overlap`.
fn discriminators_overlap(
  a : @types.DiscriminatorRule,
  b : @types.DiscriminatorRule,
) -> String? {
  match (a, b) {
    (@types.Prefix(pa), @types.Prefix(pb)) =>
      if pa.is_empty() && pb.is_empty() {
        Some("both prefixes are empty")
      } else if pa.is_empty() {
        Some("empty prefix overlaps any other prefix `\{pb}`")
      } else if pb.is_empty() {
        Some("empty prefix overlaps any other prefix `\{pa}`")
      } else if pa.has_prefix(pb) || pb.has_prefix(pa) {
        Some("prefix `\{pa}` and prefix `\{pb}` overlap")
      } else {
        None
      }
    (@types.Suffix(sa), @types.Suffix(sb)) =>
      if sa.is_empty() && sb.is_empty() {
        Some("both suffixes are empty")
      } else if sa.is_empty() {
        Some("empty suffix overlaps any other suffix `\{sb}`")
      } else if sb.is_empty() {
        Some("empty suffix overlaps any other suffix `\{sa}`")
      } else if sa.has_suffix(sb) || sb.has_suffix(sa) {
        Some("suffix `\{sa}` and suffix `\{sb}` overlap")
      } else {
        None
      }
    (@types.Contains(ca), @types.Contains(cb)) =>
      if ca.is_empty() || cb.is_empty() {
        Some("empty contains substring matches every string")
      } else {
        None
      }
    (@types.Regex(ra), @types.Regex(rb)) =>
      if ra == rb {
        Some("both branches share regex `\{ra}`")
      } else {
        None
      }
    (@types.FieldEquals(fa), @types.FieldEquals(fb)) =>
      if fa.field_name != fb.field_name {
        None
      } else {
        match (fa.literal, fb.literal) {
          (None, _) | (_, None) =>
            Some(
              "field-equals on `\{fa.field_name}` without literal overlaps another field-equals on the same field",
            )
          (Some(la), Some(lb)) =>
            if la == lb {
              Some(
                "two field-equals on `\{fa.field_name}` share literal `\{la}`",
              )
            } else {
              None
            }
        }
      }
    (@types.FieldAbsent(fa), @types.FieldAbsent(fb)) =>
      if fa == fb {
        Some("two field-absent rules share field `\{fa}`")
      } else {
        None
      }
    // Mixed pairs: an empty prefix/suffix/contains on either side matches every
    // string and therefore overlaps any other rule. Checked prefix→suffix→
    // contains to mirror the Rust arm order.
    _ =>
      if is_empty_prefix(a) || is_empty_prefix(b) {
        Some("empty prefix matches every string")
      } else if is_empty_suffix(a) || is_empty_suffix(b) {
        Some("empty suffix matches every string")
      } else if is_empty_contains(a) || is_empty_contains(b) {
        Some("empty contains substring matches every string")
      } else {
        None
      }
  }
}

///|
fn is_empty_prefix(r : @types.DiscriminatorRule) -> Bool {
  match r {
    @types.Prefix(p) => p.is_empty()
    _ => false
  }
}

///|
fn is_empty_suffix(r : @types.DiscriminatorRule) -> Bool {
  match r {
    @types.Suffix(s) => s.is_empty()
    _ => false
  }
}

///|
fn is_empty_contains(r : @types.DiscriminatorRule) -> Bool {
  match r {
    @types.Contains(c) => c.is_empty()
    _ => false
  }
}
