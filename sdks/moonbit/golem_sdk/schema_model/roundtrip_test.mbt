// Phase 1 tests for the recursive schema model and its WIT codecs:
//   * recursive <-> flat round-trips for types, graphs, and values,
//   * `Ref(TypeId)` <-> `ref-type(def-index)` mapping and deterministic def
//     ordering,
//   * recursive / mutually-recursive types closed via the builder's
//     reserve/commit protocol,
//   * malformed flat carriers rejected (out-of-range index, duplicate def id,
//     uncommitted reservation, structural cycle).

///|
/// Round-trip a standalone type by wrapping it in a defs-less graph.
fn rt_type(st : SchemaType) -> SchemaType raise {
  schema_graph_from_wit(schema_graph_to_wit({ defs: [], root: st })).root
}

///|
fn rt_value(v : SchemaValue) -> SchemaValue raise {
  schema_value_from_wit(schema_value_to_wit(v))
}

///|
fn rt_graph(g : SchemaGraph) -> SchemaGraph raise {
  schema_graph_from_wit(schema_graph_to_wit(g))
}

///|
fn encoded_body(st : SchemaType) -> @types.SchemaTypeBody raise {
  let wit = schema_graph_to_wit({ defs: [], root: st })
  wit.type_nodes[wit.root].body
}

///|
fn ub(v : UInt64) -> @types.NumericBound {
  @types.Unsigned(v)
}

///|
fn sb(v : Int64) -> @types.NumericBound {
  @types.Signed(v)
}

///|
fn fb(bits : UInt64) -> @types.NumericBound {
  @types.FloatBits(bits)
}

///|
fn nr(
  min? : @types.NumericBound,
  max? : @types.NumericBound,
  unit? : String,
) -> @types.NumericRestrictions {
  { min, max, unit }
}

///|
test "primitive types round-trip" {
  let prims : Array[SchemaTypeBody] = [
    Bool,
    S8(None),
    S16(None),
    S32(None),
    S64(None),
    U8(None),
    U16(None),
    U32(None),
    U64(None),
    F32(None),
    F64(None),
    Char,
    String,
    Datetime,
    Duration,
  ]
  for body in prims {
    let st = schema_type(body)
    assert_eq(rt_type(st), st)
  }
}

///|
test "numeric restrictions golden vectors round-trip" {
  let i64_max = 9_223_372_036_854_775_807L
  let u64_max = 18_446_744_073_709_551_615UL
  let cases : Array[SchemaType] = [
    schema_type(U32(None)),
    schema_type(U32(Some(nr(min=ub(1UL))))),
    schema_type(U32(Some(nr(min=ub(1UL), unit="items")))),
    schema_type(U32(Some(nr(min=ub(0UL), max=ub(100UL))))),
    schema_type(U32(Some(nr(min=ub(0UL), max=ub(100UL), unit="percent")))),
    schema_type(S64(Some(nr(min=sb(0L), max=sb(i64_max))))),
    schema_type(S64(Some(nr(min=sb(0L), max=sb(i64_max), unit="ns")))),
    schema_type(U64(Some(nr(min=ub(u64_max - 1UL), max=ub(u64_max))))),
    schema_type(
      U64(Some(nr(min=ub(u64_max - 1UL), max=ub(u64_max), unit="bytes"))),
    ),
    schema_type(F64(Some(nr(min=fb(0UL))))),
    schema_type(F64(Some(nr(min=fb(0UL), unit="seconds")))),
    schema_type(S8(Some(nr(min=sb(-1L), max=sb(1L))))),
    schema_type(F32(Some(nr(max=fb(4_609_434_218_613_702_656UL))))),
  ]
  inspect(
    cases.map(rt_type),
    content=(
      #|[{body: U32(None), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: U32(Some({min: Some(Unsigned(1)), max: None, unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: U32(Some({min: Some(Unsigned(1)), max: None, unit: Some(items)})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: U32(Some({min: Some(Unsigned(0)), max: Some(Unsigned(100)), unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: U32(Some({min: Some(Unsigned(0)), max: Some(Unsigned(100)), unit: Some(percent)})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: S64(Some({min: Some(Signed(0)), max: Some(Signed(9223372036854775807)), unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: S64(Some({min: Some(Signed(0)), max: Some(Signed(9223372036854775807)), unit: Some(ns)})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: U64(Some({min: Some(Unsigned(18446744073709551614)), max: Some(Unsigned(18446744073709551615)), unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: U64(Some({min: Some(Unsigned(18446744073709551614)), max: Some(Unsigned(18446744073709551615)), unit: Some(bytes)})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: F64(Some({min: Some(FloatBits(0)), max: None, unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: F64(Some({min: Some(FloatBits(0)), max: None, unit: Some(seconds)})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: S8(Some({min: Some(Signed(-1)), max: Some(Signed(1)), unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}, {body: F32(Some({min: None, max: Some(FloatBits(4609434218613702656)), unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}]
    ),
  )
}

///|
test "numeric restrictions canonicalization" {
  inspect(
    encoded_body(schema_type(U32(Some(nr())))),
    content=(
      #|U32Type(None)
    ),
  )
  inspect(
    rt_type(schema_type(U32(Some(nr(unit=""))))),
    content=(
      #|{body: U32(None), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}
    ),
  )
  inspect(
    rt_type(schema_type(U32(Some(nr(min=ub(1UL), unit=""))))),
    content=(
      #|{body: U32(Some({min: Some(Unsigned(1)), max: None, unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}
    ),
  )
  inspect(
    rt_type(schema_type(F64(Some(nr(min=fb(9_223_372_036_854_775_808UL)))))),
    content=(
      #|{body: F64(Some({min: Some(FloatBits(0)), max: None, unit: None})), metadata: {doc: None, aliases: [], examples: [], deprecated: None, role: None}}
    ),
  )
}

///|
test "composite types round-trip" {
  let inner = schema_type(
    Record([
      field("id", schema_type(S64(None))),
      field("name", schema_type(String)),
      field("tags", schema_type(List(schema_type(String)))),
      field("maybe", schema_type(Option(schema_type(Bool)))),
      field(
        "outcome",
        schema_type(
          Result(Some(schema_type(S32(None))), Some(schema_type(String))),
        ),
      ),
      field(
        "pairs",
        schema_type(Map(schema_type(String), schema_type(U32(None)))),
      ),
      field("fixed", schema_type(FixedList(schema_type(U8(None)), 4))),
      field(
        "triple",
        schema_type(
          Tuple([schema_type(Bool), schema_type(Char), schema_type(F64(None))]),
        ),
      ),
      field("flags", schema_type(Flags(["a", "b", "c"]))),
      field("color", schema_type(Enum(["red", "green", "blue"]))),
      field(
        "shape",
        schema_type(
          Variant([
            variant_case("circle", payload=schema_type(F64(None))),
            variant_case("point"),
          ]),
        ),
      ),
    ]),
  )
  assert_eq(rt_type(inner), inner)
}

///|
test "values round-trip (primitives, composites, semantic, capability)" {
  let v : SchemaValue = Tuple([
    Bool(true),
    S8(-5),
    S16(-300),
    S32(-70000),
    S64(-5_000_000_000L),
    U8(200),
    U16(60000),
    U32(4_000_000_000U),
    U64(18_000_000_000_000_000_000UL),
    F32(1.5),
    F64(-2.25),
    Char('λ'),
    String("héllo"),
    Record([S32(1), String("two")]),
    Variant(1U, Some(String("payload"))),
    Variant(0U, None),
    Enum(2U),
    Flags([true, false, true]),
    List([S32(1), S32(2), S32(3)]),
    FixedList([U8(1), U8(2)]),
    Map([
      { key: String("a"), value: S32(1) },
      { key: String("b"), value: S32(2) },
    ]),
    Option(Some(Bool(false))),
    Option(None),
    ResultOk(Some(S32(7))),
    ResultErr(None),
    Text("bonjour", Some("fr")),
    Text("plain", None),
    Binary(([0, 1, 255] : FixedArray[Byte]), Some("image/png")),
    Path("/tmp/x"),
    Url("https://golem.cloud"),
    Datetime({ seconds: 100L, nanoseconds: 5U }),
    Duration(-123_456L),
    Quantity({ mantissa: 1234L, scale: 2, unit: "kg" }),
    Union("circle", S32(42)),
    Secret("secret-ref-abc"),
  ])
  assert_eq(rt_value(v), v)
}

///|
test "Ref maps to ref-type by first-seen def index" {
  // Definitions keep first-seen order: Zulu=0, Bravo=1 (not sorted).
  let g : SchemaGraph = {
    defs: [
      { id: "Zulu", name: None, body: schema_type(String) },
      { id: "Bravo", name: None, body: schema_type(S32(None)) },
    ],
    root: schema_type(
      Tuple([schema_type(Ref("Zulu")), schema_type(Ref("Bravo"))]),
    ),
  }
  let wit = schema_graph_to_wit(g)
  // Defs are emitted in first-seen order.
  inspect(wit.defs.map(d => d.id), content="[Zulu, Bravo]")
  // The tuple's first element references Zulu (def-index 0), second Bravo (1).
  let refs = wit.type_nodes
    .iter()
    .filter_map(n => {
      match n.body {
        @types.RefType(di) => Some(di)
        _ => None
      }
    })
    .collect()
  inspect(refs, content="[0, 1]")
  // Full structural round-trip preserves the Refs.
  assert_eq(rt_graph(g), g)
}

///|
test "self-recursive type closes via reserve/commit" {
  let b = SchemaBuilder::new()
  let cons = b.register(
    "IntList",
    () => {
      schema_type(
        Variant([
          variant_case("nil"),
          variant_case(
            "cons",
            payload=schema_type(
              Record([
                field("head", schema_type(S32(None))),
                field("tail", b.ref_("IntList")),
              ]),
            ),
          ),
        ]),
      )
    },
    None,
  )
  let g = b.build_graph(cons)
  assert_eq(rt_graph(g), g)
}

///|
test "mutually-recursive types close via reserve/commit" {
  let b = SchemaBuilder::new()
  // A { b: B }  and  B { a: Option<A> }
  let a_ref = b.register(
    "A",
    () => schema_type(Record([field("b", b.ref_("B"))])),
    None,
  )
  let _ = b.register(
    "B",
    () => schema_type(Record([field("a", schema_type(Option(b.ref_("A"))))])),
    None,
  )
  let g = b.build_graph(a_ref)
  assert_eq(rt_graph(g), g)
}

///|
test "encode rejects Ref to unknown type id" {
  let g : SchemaGraph = { defs: [], root: schema_type(Ref("Ghost")) }
  let r = try? schema_graph_to_wit(g)
  guard r is Err(EncodeError(msg)) else {
    fail("expected EncodeError, got \{r}")
  }
  assert_true(msg.contains("unknown type id"))
}

///|
test "decode rejects out-of-range root index" {
  let bad : @types.SchemaGraph = { type_nodes: [], defs: [], root: 0 }
  let r = try? schema_graph_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("out of range"))
}

///|
test "decode rejects out-of-range child index" {
  let bad : @types.SchemaGraph = {
    type_nodes: [{ body: @types.ListType(9), metadata: empty_metadata() }],
    defs: [],
    root: 0,
  }
  let r = try? schema_graph_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("out of range"))
}

///|
test "decode rejects duplicate def id" {
  let node : @types.SchemaTypeNode = {
    body: @types.BoolType,
    metadata: empty_metadata(),
  }
  let bad : @types.SchemaGraph = {
    type_nodes: [node],
    defs: [
      { id: "Dup", name: None, body: 0 },
      { id: "Dup", name: None, body: 0 },
    ],
    root: 0,
  }
  let r = try? schema_graph_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("duplicate def id"))
}

///|
test "decode rejects a structural cycle in type nodes" {
  // Node 0 is a list whose element is node 0 (a self back-edge).
  let bad : @types.SchemaGraph = {
    type_nodes: [{ body: @types.ListType(0), metadata: empty_metadata() }],
    defs: [],
    root: 0,
  }
  let r = try? schema_graph_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("cyclic"))
}

///|
test "decode rejects out-of-range value node index" {
  let bad : @types.SchemaValueTree = { value_nodes: [], root: 3 }
  let r = try? schema_value_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("out of range"))
}

///|
test "builder finish rejects an uncommitted reservation" {
  let b = SchemaBuilder::new()
  b.reserve("Pending")
  let r = try? b.finish()
  guard r is Err(BuilderError(msg)) else {
    fail("expected BuilderError, got \{r}")
  }
  assert_true(msg.contains("never committed"))
}

///|
test "merge_agent_graphs dedups shared defs and rejects conflicts" {
  let shared : SchemaTypeDef = {
    id: "Shared",
    name: None,
    body: schema_type(S32(None)),
  }
  let g1 : SchemaGraph = { defs: [shared], root: schema_type(Ref("Shared")) }
  let g2 : SchemaGraph = {
    defs: [shared],
    root: schema_type(List(schema_type(Ref("Shared")))),
  }
  let merged = merge_agent_graphs([g1, g2])
  inspect(merged.defs.length(), content="1")
  inspect(merged.roots.length(), content="2")

  // A conflicting body for the same id is rejected.
  let g3 : SchemaGraph = {
    defs: [{ id: "Shared", name: None, body: schema_type(String) }],
    root: schema_type(Ref("Shared")),
  }
  let r = try? merge_agent_graphs([g1, g3])
  guard r is Err(MergeConflict(id)) else {
    fail("expected MergeConflict, got \{r}")
  }
  assert_eq(id, "Shared")
}

///|
test "semantic and capability types round-trip" {
  let text_r : @types.TextRestrictions = {
    languages: Some(["en", "fr"]),
    min_length: Some(1),
    max_length: Some(64),
    regex: Some("^[a-z]+$"),
  }
  let bin_r : @types.BinaryRestrictions = {
    mime_types: Some(["image/png"]),
    min_bytes: None,
    max_bytes: Some(1024),
  }
  let path_s : @types.PathSpec = {
    direction: @types.INPUT,
    kind: @types.FILE,
    allowed_mime_types: None,
    allowed_extensions: Some([".txt"]),
  }
  let url_r : @types.UrlRestrictions = {
    allowed_schemes: Some(["https"]),
    allowed_hosts: None,
  }
  let qty_s : @types.QuantitySpec = {
    base_unit: "m",
    allowed_suffixes: ["km", "cm"],
    min: None,
    max: None,
  }
  let secret_s : @types.SecretSpec = { category: Some("api-key") }
  let quota_s : @types.QuotaTokenSpec = { resource_name: Some("cpu") }
  let branch_a : UnionBranch = {
    tag: "a",
    body: schema_type(S32(None)),
    discriminator: @types.DiscriminatorRule::Prefix("a:"),
    metadata: empty_metadata(),
  }
  let branch_b : UnionBranch = {
    tag: "b",
    body: schema_type(String),
    discriminator: @types.DiscriminatorRule::Suffix(":b"),
    metadata: empty_metadata(),
  }
  let t = schema_type(
    Record([
      field("text", schema_type(Text(text_r))),
      field("binary", schema_type(Binary(bin_r))),
      field("path", schema_type(Path(path_s))),
      field("url", schema_type(Url(url_r))),
      field("quantity", schema_type(Quantity(qty_s))),
      field("secret", schema_type(Secret(secret_s))),
      field("quota", schema_type(QuotaToken(quota_s))),
      field("future_some", schema_type(Future(Some(schema_type(S32(None)))))),
      field("future_none", schema_type(Future(None))),
      field("stream_some", schema_type(Stream(Some(schema_type(String))))),
      field("stream_none", schema_type(Stream(None))),
      field("union", schema_type(Union([branch_a, branch_b]))),
    ]),
  )
  assert_eq(rt_type(t), t)
}

///|
test "encode_merged_agent_graph shares one pool across roots" {
  let shared : SchemaTypeDef = {
    id: "Shared",
    name: None,
    body: schema_type(S32(None)),
  }
  let g1 : SchemaGraph = { defs: [shared], root: schema_type(Ref("Shared")) }
  let g2 : SchemaGraph = {
    defs: [shared],
    root: schema_type(List(schema_type(Ref("Shared")))),
  }
  let merged = merge_agent_graphs([g1, g2])
  let enc = encode_merged_agent_graph(merged)
  // Two roots, in input order; shared def appears once.
  inspect(enc.root_indices.length(), content="2")
  inspect(enc.graph.defs.length(), content="1")
  // All root indices are valid type-node indices into the shared pool.
  for idx in enc.root_indices {
    assert_true(idx >= 0 && idx < enc.graph.type_nodes.length())
  }
  // The graph's own root is a sentinel empty record (placeholder).
  guard enc.graph.type_nodes[enc.graph.root].body is @types.RecordType(fields) else {
    fail("expected sentinel empty-record root")
  }
  inspect(fields.length(), content="0")
  // Decode defs once, then each real root against the shared graph (public
  // multi-root decoder API).
  let dec = GraphDecoder::new(enc.graph)
  let defs = dec.decode_defs()
  inspect(defs.length(), content="1")
  assert_eq(dec.decode_type_at(enc.root_indices[0]), g1.root)
  assert_eq(dec.decode_type_at(enc.root_indices[1]), g2.root)
}

///|
test "decode rejects out-of-range value child index" {
  let bad : @types.SchemaValueTree = {
    value_nodes: [@types.RecordValue([5])],
    root: 0,
  }
  let r = try? schema_value_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("out of range"))
}

///|
test "decode rejects a value structural cycle" {
  let bad : @types.SchemaValueTree = {
    value_nodes: [@types.ListValue([0])],
    root: 0,
  }
  let r = try? schema_value_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("cyclic"))
}

///|
test "datetime nanoseconds must be < 1e9 (encode + decode)" {
  let v : SchemaValue = Datetime({ seconds: 0L, nanoseconds: 1_000_000_000U })
  let re = try? schema_value_to_wit(v)
  guard re is Err(EncodeError(em)) else {
    fail("expected EncodeError, got \{re}")
  }
  assert_true(em.contains("datetime"))
  let bad : @types.SchemaValueTree = {
    value_nodes: [
      @types.DatetimeValue({ seconds: 0L, nanoseconds: 1_000_000_000U }),
    ],
    root: 0,
  }
  let rd = try? schema_value_from_wit(bad)
  guard rd is Err(DecodeError(dm)) else {
    fail("expected DecodeError, got \{rd}")
  }
  assert_true(dm.contains("datetime"))
}

///|
test "narrow integer values are range-checked" {
  let bad : Array[SchemaValue] = [S8(128), S8(-129), S16(32768), U16(65536U)]
  for v in bad {
    let r = try? schema_value_to_wit(v)
    guard r is Err(EncodeError(_)) else {
      fail("expected EncodeError for out-of-range value, got \{r}")
    }
  }
  // In-range boundary values still encode.
  assert_true((try? schema_value_to_wit(S8(127))) is Ok(_))
  assert_true((try? schema_value_to_wit(S8(-128))) is Ok(_))
  assert_true((try? schema_value_to_wit(S16(-32768))) is Ok(_))
  assert_true((try? schema_value_to_wit(U16(65535U))) is Ok(_))
}

///|
test "narrow integer values are range-checked on decode" {
  let bad : Array[@types.SchemaValueNode] = [
    @types.S8Value(128),
    @types.S8Value(-129),
    @types.S16Value(32768),
    @types.U16Value(65536U),
  ]
  for node in bad {
    let tree : @types.SchemaValueTree = { value_nodes: [node], root: 0 }
    let r = try? schema_value_from_wit(tree)
    guard r is Err(DecodeError(_)) else {
      fail("expected DecodeError for out-of-range value, got \{r}")
    }
  }
}

///|
/// Build an opaque quota-token handle wrapping a fake owned resource. The
/// generated `@types.QuotaToken` is a plain index wrapper, so a handle can be
/// fabricated for codec tests without a host call — as long as the resource is
/// never dropped or passed to a host import (which a happy-path codec round-trip
/// never does).
fn fake_quota_handle(index : Int) -> GuestQuotaTokenHandle {
  GuestQuotaTokenHandle::new(@types.QuotaToken::QuotaToken(index))
}

///|
test "quota-token value encodes to a handle node and decodes to a fresh handle" {
  let handle = fake_quota_handle(7)
  let v : SchemaValue = QuotaToken(handle)
  let wit = schema_value_to_wit(v)
  // Encoding moves the owned handle out of the original holder exactly once.
  assert_false(handle.is_present())
  // The wire node is the opaque handle carrier, not a structural record.
  guard wit.value_nodes[wit.root] is @types.QuotaTokenHandle(_) else {
    fail("expected a quota-token-handle wire node")
  }
  // Decoding lifts the owned handle back into a present holder.
  guard schema_value_from_wit(wit) is QuotaToken(decoded) else {
    fail("expected a decoded quota-token value")
  }
  assert_true(decoded.is_present())
}

///|
test "encode preflight rejects an already-transferred quota-token handle" {
  let handle = fake_quota_handle(1)
  let _ = handle.take() // consume it up front
  let r = try? schema_value_to_wit(QuotaToken(handle))
  guard r is Err(EncodeError(em)) else {
    fail("expected EncodeError, got \{r}")
  }
  assert_true(em.contains("quota-token"))
}

///|
test "encode preflight rejects the same quota-token handle used twice" {
  let handle = fake_quota_handle(2)
  let v : SchemaValue = Tuple([QuotaToken(handle), QuotaToken(handle)])
  let r = try? schema_value_to_wit(v)
  guard r is Err(EncodeError(em)) else {
    fail("expected EncodeError, got \{r}")
  }
  assert_true(em.contains("quota-token"))
  // The shared handle is left untouched when the preflight rejects the tree.
  assert_true(handle.is_present())
}

///|
test "encode error after a quota-token does not consume the handle" {
  // A later non-quota validation error (s8 out of range) must be raised by the
  // preflight before the quota-token handle is moved, so the token survives.
  let handle = fake_quota_handle(9)
  let v : SchemaValue = Tuple([QuotaToken(handle), S8(128)])
  let r = try? schema_value_to_wit(v)
  guard r is Err(EncodeError(_)) else { fail("expected EncodeError, got \{r}") }
  assert_true(handle.is_present())
}

///|
test "encode rejects two distinct holders wrapping the same raw handle" {
  let raw = @types.QuotaToken::QuotaToken(11)
  let a = GuestQuotaTokenHandle::new(raw)
  let b = GuestQuotaTokenHandle::new(raw)
  let r = try? schema_value_to_wit(Tuple([QuotaToken(a), QuotaToken(b)]))
  guard r is Err(EncodeError(em)) else {
    fail("expected EncodeError, got \{r}")
  }
  assert_true(em.contains("more than once"))
  // Neither handle was moved when the preflight rejected the tree.
  assert_true(a.is_present() && b.is_present())
}

///|
test "decode rejects the same raw handle in two different nodes" {
  let raw = @types.QuotaToken::QuotaToken(12)
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.QuotaTokenHandle(raw),
      @types.QuotaTokenHandle(raw),
      @types.TupleValue([0, 1]),
    ],
    root: 2,
  }
  let r = try? schema_value_from_wit(tree)
  guard r is Err(DecodeError(dm)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(dm.contains("more than one handle node"))
}

///|
test "decode rejects a handle node referenced from two places" {
  // A tuple whose two elements both point at the single handle node 0.
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(3)),
      @types.TupleValue([0, 0]),
    ],
    root: 1,
  }
  let r = try? schema_value_from_wit(tree)
  guard r is Err(DecodeError(dm)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(dm.contains("more than once"))
}

///|
test "decode rejects a handle node unreachable from the root" {
  // Root is a unit tuple; the handle node 1 is never referenced.
  let tree : @types.SchemaValueTree = {
    value_nodes: [
      @types.TupleValue([]),
      @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(4)),
    ],
    root: 0,
  }
  let r = try? schema_value_from_wit(tree)
  guard r is Err(DecodeError(dm)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(dm.contains("not referenced from the root"))
}

///|
test "duplicate def id is rejected before a bad body is decoded" {
  // Both a duplicate def id AND an out-of-range body index exist; the duplicate
  // must be reported first (GraphDecoder::new checks ids up front).
  let node : @types.SchemaTypeNode = {
    body: @types.ListType(99),
    metadata: empty_metadata(),
  }
  let bad : @types.SchemaGraph = {
    type_nodes: [node],
    defs: [
      { id: "Dup", name: None, body: 0 },
      { id: "Dup", name: None, body: 0 },
    ],
    root: 0,
  }
  let r = try? schema_graph_from_wit(bad)
  guard r is Err(DecodeError(msg)) else {
    fail("expected DecodeError, got \{r}")
  }
  assert_true(msg.contains("duplicate def id"))
}
