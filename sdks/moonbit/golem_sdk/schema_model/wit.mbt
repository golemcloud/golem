// Codecs between the recursive in-memory schema model (model.mbt) and the flat
// `golem:core/types@2.0.0` WIT carriers (`schema-graph`, `schema-value-tree`,
// `typed-schema-value`).
//
// Flattening assigns a `type-node` / `value-node` index to every node and
// records definitions in their given first-seen / commit order (the same order
// `SchemaBuilder` and `merge_agent_graphs` produce — never re-sorted, matching
// the Rust `golem-schema` encoder). `Ref` bodies flatten to
// `ref-type(def-index)`, where the def index is the definition's position in
// that order. Unflattening walks indices back into the recursive form, guarding
// against out-of-range and cyclic indices.
//
// This file is the ONLY place in the SDK that touches the generated flat WIT
// value/type carriers (`@types.Schema*`).

// ============================================================
// Value invariants (shared by encode + decode boundaries)
// ============================================================
//
// The recursive model is `pub(all)`, so out-of-range narrow integers and
// invalid datetimes are constructible in memory. These checks enforce the WIT
// contract at the encode boundary (raising `EncodeError`) and reject malformed
// inbound carriers at the decode boundary (raising `DecodeError`), mirroring the
// Rust `golem-schema` typed model (`i8`/`i16`/`u16` + validated datetimes).

///|
fn datetime_valid(dt : @types.Datetime) -> Bool {
  dt.nanoseconds < 1_000_000_000U
}

///|
fn s8_in_range(i : Int) -> Bool {
  i >= -128 && i <= 127
}

///|
fn s16_in_range(i : Int) -> Bool {
  i >= -32768 && i <= 32767
}

///|
fn u16_in_range(u : UInt) -> Bool {
  u <= 65535
}

// ============================================================
// Schema type / graph — encode (recursive -> flat)
// ============================================================

///|
/// Incremental encoder for a single flat `@types.SchemaGraph` that holds
/// several independent root types in one shared `type_nodes` pool. Mirrors the
/// Rust `golem-schema` `GraphEncoder`.
///
/// Seed the encoder with the agent's merged named definitions, then call
/// `encode_type` once per inline root (constructor / method / config),
/// collecting the returned `type-node-index` values, and finally `finish` to
/// obtain the graph with a placeholder root. The graph's own `root` is a
/// structurally-required placeholder (an empty record); agent-layer carriers
/// never consult it — the real roots are the returned indices.
///
/// Do NOT encode each agent root via `schema_graph_to_wit` and reuse those
/// indices: per-graph node indices are only valid within a single encoding.
pub struct GraphEncoder {
  type_nodes : Array[@types.SchemaTypeNode]
  def_index_by_id : Map[TypeId, Int]
  wit_defs : Array[@types.SchemaTypeDef]
}

///|
/// Create an encoder seeded with `defs`. Definitions keep their given order
/// (first-seen / commit order — never sorted): `def-index` values follow that
/// order and are reserved up front so `ref-type` resolves both forward and
/// backward references, and then each body is encoded into the shared pool.
pub fn GraphEncoder::new(
  defs : Array[SchemaTypeDef],
) -> GraphEncoder raise SchemaModelError {
  let def_index_by_id : Map[TypeId, Int] = Map::new()
  for i, def in defs {
    if def_index_by_id.contains(def.id) {
      raise EncodeError("duplicate def id '\{def.id}' in schema graph")
    }
    def_index_by_id[def.id] = i
  }
  let enc : GraphEncoder = { type_nodes: [], def_index_by_id, wit_defs: [] }
  // Encode each def body into the shared pool. All def indices are already
  // registered, so a `Ref` to any def (forward or back) resolves correctly.
  let bodies : Array[Int] = []
  for def in defs {
    bodies.push(enc.encode_type(def.body))
  }
  for i, def in defs {
    enc.wit_defs.push({ id: def.id, name: def.name, body: bodies[i] })
  }
  enc
}

///|
/// Flatten one (possibly recursive) schema type into the shared pool and return
/// its `type-node-index`.
pub fn GraphEncoder::encode_type(
  self : GraphEncoder,
  st : SchemaType,
) -> Int raise SchemaModelError {
  let body = self.encode_body(st.body)
  self.type_nodes.push({ body, metadata: st.metadata })
  self.type_nodes.length() - 1
}

///|
fn GraphEncoder::encode_opt(
  self : GraphEncoder,
  o : SchemaType?,
) -> Int? raise SchemaModelError {
  match o {
    Some(t) => Some(self.encode_type(t))
    None => None
  }
}

///|
fn GraphEncoder::encode_body(
  self : GraphEncoder,
  body : SchemaTypeBody,
) -> @types.SchemaTypeBody raise SchemaModelError {
  match body {
    Ref(id) =>
      match self.def_index_by_id.get(id) {
        Some(di) => @types.RefType(di)
        None =>
          raise EncodeError("schema graph references unknown type id '\{id}'")
      }
    Bool => @types.BoolType
    S8 => @types.S8Type
    S16 => @types.S16Type
    S32 => @types.S32Type
    S64 => @types.S64Type
    U8 => @types.U8Type
    U16 => @types.U16Type
    U32 => @types.U32Type
    U64 => @types.U64Type
    F32 => @types.F32Type
    F64 => @types.F64Type
    Char => @types.CharType
    String => @types.StringType
    Record(fields) =>
      @types.RecordType(
        fields.map(f => {
          (
            {
              name: f.name,
              body: self.encode_type(f.body),
              metadata: f.metadata,
            } : @types.NamedFieldType)
        }),
      )
    Variant(cases) =>
      @types.VariantType(
        cases.map(c => {
          (
            {
              name: c.name,
              payload: self.encode_opt(c.payload),
              metadata: c.metadata,
            } : @types.VariantCaseType)
        }),
      )
    Enum(cases) => @types.EnumType(cases)
    Flags(names) => @types.FlagsType(names)
    Tuple(elements) => @types.TupleType(elements.map(e => self.encode_type(e)))
    List(element) => @types.ListType(self.encode_type(element))
    FixedList(element, length) =>
      @types.FixedListType({ element: self.encode_type(element), length })
    Map(key, value) =>
      @types.MapType({
        key: self.encode_type(key),
        value: self.encode_type(value),
      })
    Option(element) => @types.OptionType(self.encode_type(element))
    Result(ok, err) =>
      @types.ResultType({ ok: self.encode_opt(ok), err: self.encode_opt(err) })
    Text(restrictions) => @types.TextType(restrictions)
    Binary(restrictions) => @types.BinaryType(restrictions)
    Path(spec) => @types.PathType(spec)
    Url(restrictions) => @types.UrlType(restrictions)
    Datetime => @types.DatetimeType
    Duration => @types.DurationType
    Quantity(spec) => @types.QuantityType(spec)
    Union(branches) =>
      @types.UnionType({
        branches: branches.map(br => {
          (
            {
              tag: br.tag,
              body: self.encode_type(br.body),
              discriminator: br.discriminator,
              metadata: br.metadata,
            } : @types.UnionBranch)
        }),
      })
    Secret(spec) => @types.SecretType(spec)
    QuotaToken(spec) => @types.QuotaTokenType(spec)
    Future(element) => @types.FutureType(self.encode_opt(element))
    Stream(element) => @types.StreamType(self.encode_opt(element))
  }
}

///|
/// Encode `root` as the graph root and return the finished flat graph.
pub fn GraphEncoder::encode_graph_root(
  self : GraphEncoder,
  root : SchemaType,
) -> @types.SchemaGraph raise SchemaModelError {
  let root_idx = self.encode_type(root)
  { type_nodes: self.type_nodes, defs: self.wit_defs, root: root_idx }
}

///|
/// Finish the graph with a placeholder empty-record root. Use after collecting
/// the real root indices via `encode_type`.
pub fn GraphEncoder::finish(
  self : GraphEncoder,
) -> @types.SchemaGraph raise SchemaModelError {
  let root = self.encode_type(schema_type(Record([])))
  { type_nodes: self.type_nodes, defs: self.wit_defs, root }
}

///|
/// Encode a complete recursive `SchemaGraph` into its flat carrier.
pub fn schema_graph_to_wit(
  graph : SchemaGraph,
) -> @types.SchemaGraph raise SchemaModelError {
  GraphEncoder::new(graph.defs).encode_graph_root(graph.root)
}

///|
/// A flat agent-layer graph: one shared `@types.SchemaGraph` (with a sentinel
/// placeholder root) plus the `type-node-index` of every real root, in input
/// order. The agent layer installs these indices into `NamedField.schema`,
/// `OutputSchema::Single`, config decls, etc.
pub(all) struct EncodedAgentGraph {
  graph : @types.SchemaGraph
  root_indices : Array[Int]
}

///|
/// Encode a `MergedAgentGraph` into one shared flat graph: all roots share the
/// same `type_nodes` / `defs` pool, so the returned indices are valid against
/// `graph`. (Encoding each root with `schema_graph_to_wit` separately would
/// produce indices that are only valid within their own graph — never do that
/// for agent-layer carriers.)
pub fn encode_merged_agent_graph(
  merged : MergedAgentGraph,
) -> EncodedAgentGraph raise SchemaModelError {
  let enc = GraphEncoder::new(merged.defs)
  let root_indices = merged.roots.map(r => enc.encode_type(r))
  { graph: enc.finish(), root_indices }
}

// ============================================================
// Schema type / graph — decode (flat -> recursive)
// ============================================================

///|
/// Incremental decoder for a flat `@types.SchemaGraph`. Use it directly for
/// multi-root agent-layer carriers: build it once, `decode_defs` once, then
/// `decode_type_at` for each stored root `type-node-index`. Mirrors the Rust
/// `golem-schema` `GraphDecoder`.
pub struct GraphDecoder {
  nodes : Array[@types.SchemaTypeNode]
  wit_defs : Array[@types.SchemaTypeDef]
  // Cycle guard: `true` = node currently on the DFS path. Legitimate recursion
  // travels through `ref-type` (resolved without recursing here), so only a
  // structural back-edge in raw type-node indices is reported as a cycle.
  on_path : Array[Bool]
}

///|
/// Create a decoder, rejecting duplicate def ids up front (before any body is
/// decoded), matching the Rust decoder's error precedence.
pub fn GraphDecoder::new(
  wit : @types.SchemaGraph,
) -> GraphDecoder raise SchemaModelError {
  let seen : Map[TypeId, Unit] = Map::new()
  for d in wit.defs {
    if seen.contains(d.id) {
      raise DecodeError("duplicate def id '\{d.id}' in schema graph")
    }
    seen[d.id] = ()
  }
  {
    nodes: wit.type_nodes,
    wit_defs: wit.defs,
    on_path: Array::make(wit.type_nodes.length(), false),
  }
}

///|
/// Decode all named definitions, preserving their order.
pub fn GraphDecoder::decode_defs(
  self : GraphDecoder,
) -> Array[SchemaTypeDef] raise SchemaModelError {
  let defs : Array[SchemaTypeDef] = []
  for d in self.wit_defs {
    defs.push({ id: d.id, name: d.name, body: self.type_at(d.body) })
  }
  defs
}

///|
/// Decode a single root type by its `type-node-index`.
pub fn GraphDecoder::decode_type_at(
  self : GraphDecoder,
  idx : Int,
) -> SchemaType raise SchemaModelError {
  self.type_at(idx)
}

///|
fn GraphDecoder::def_id(
  self : GraphDecoder,
  di : Int,
) -> TypeId raise SchemaModelError {
  if di < 0 || di >= self.wit_defs.length() {
    raise DecodeError(
      "def index out of range: \{di} (defs: \{self.wit_defs.length()})",
    )
  }
  self.wit_defs[di].id
}

///|
fn GraphDecoder::type_at(
  self : GraphDecoder,
  idx : Int,
) -> SchemaType raise SchemaModelError {
  if idx < 0 || idx >= self.nodes.length() {
    raise DecodeError(
      "type node index out of range: \{idx} (nodes: \{self.nodes.length()})",
    )
  }
  if self.on_path[idx] {
    raise DecodeError("cyclic type node reference at index \{idx}")
  }
  self.on_path[idx] = true
  let node = self.nodes[idx]
  let result : SchemaType = {
    body: self.body_of(node.body),
    metadata: node.metadata,
  }
  self.on_path[idx] = false
  result
}

///|
fn GraphDecoder::decode_opt(
  self : GraphDecoder,
  o : Int?,
) -> SchemaType? raise SchemaModelError {
  match o {
    Some(i) => Some(self.type_at(i))
    None => None
  }
}

///|
fn GraphDecoder::body_of(
  self : GraphDecoder,
  body : @types.SchemaTypeBody,
) -> SchemaTypeBody raise SchemaModelError {
  match body {
    @types.RefType(di) => Ref(self.def_id(di))
    @types.BoolType => Bool
    @types.S8Type => S8
    @types.S16Type => S16
    @types.S32Type => S32
    @types.S64Type => S64
    @types.U8Type => U8
    @types.U16Type => U16
    @types.U32Type => U32
    @types.U64Type => U64
    @types.F32Type => F32
    @types.F64Type => F64
    @types.CharType => Char
    @types.StringType => String
    @types.RecordType(fields) =>
      Record(
        fields.map(f => {
          (
            { name: f.name, body: self.type_at(f.body), metadata: f.metadata } :
            NamedFieldType)
        }),
      )
    @types.VariantType(cases) =>
      Variant(
        cases.map(c => {
          (
            {
              name: c.name,
              payload: self.decode_opt(c.payload),
              metadata: c.metadata,
            } : VariantCaseType)
        }),
      )
    @types.EnumType(cases) => Enum(cases)
    @types.FlagsType(names) => Flags(names)
    @types.TupleType(elements) => Tuple(elements.map(i => self.type_at(i)))
    @types.ListType(element) => List(self.type_at(element))
    @types.FixedListType(spec) =>
      FixedList(self.type_at(spec.element), spec.length)
    @types.MapType(spec) =>
      Map(self.type_at(spec.key), self.type_at(spec.value))
    @types.OptionType(element) => Option(self.type_at(element))
    @types.ResultType(spec) =>
      Result(self.decode_opt(spec.ok), self.decode_opt(spec.err))
    @types.TextType(restrictions) => Text(restrictions)
    @types.BinaryType(restrictions) => Binary(restrictions)
    @types.PathType(spec) => Path(spec)
    @types.UrlType(restrictions) => Url(restrictions)
    @types.DatetimeType => Datetime
    @types.DurationType => Duration
    @types.QuantityType(spec) => Quantity(spec)
    @types.UnionType(spec) =>
      Union(
        spec.branches.map(br => {
          (
            {
              tag: br.tag,
              body: self.type_at(br.body),
              discriminator: br.discriminator,
              metadata: br.metadata,
            } : UnionBranch)
        }),
      )
    @types.SecretType(spec) => Secret(spec)
    @types.QuotaTokenType(spec) => QuotaToken(spec)
    @types.FutureType(element) => Future(self.decode_opt(element))
    @types.StreamType(element) => Stream(self.decode_opt(element))
  }
}

///|
/// Decode a flat `@types.SchemaGraph` into the recursive `SchemaGraph`,
/// preserving definition order.
pub fn schema_graph_from_wit(
  wit : @types.SchemaGraph,
) -> SchemaGraph raise SchemaModelError {
  let dec = GraphDecoder::new(wit)
  let defs = dec.decode_defs()
  let root = dec.decode_type_at(wit.root)
  { defs, root }
}

// ============================================================
// Schema value — encode (recursive -> flat)
// ============================================================

///|
priv struct ValueEncoder {
  value_nodes : Array[@types.SchemaValueNode]
}

///|
fn ValueEncoder::new() -> ValueEncoder {
  { value_nodes: [] }
}

///|
fn ValueEncoder::emit(
  self : ValueEncoder,
  v : SchemaValue,
) -> Int raise SchemaModelError {
  self.value_nodes.push(self.emit_node(v))
  self.value_nodes.length() - 1
}

///|
fn ValueEncoder::emit_opt(
  self : ValueEncoder,
  o : SchemaValue?,
) -> Int? raise SchemaModelError {
  match o {
    Some(v) => Some(self.emit(v))
    None => None
  }
}

///|
fn ValueEncoder::emit_node(
  self : ValueEncoder,
  v : SchemaValue,
) -> @types.SchemaValueNode raise SchemaModelError {
  match v {
    Bool(b) => @types.BoolValue(b)
    S8(i) => {
      guard s8_in_range(i) else {
        raise EncodeError("s8 value out of range: \{i}")
      }
      @types.S8Value(i)
    }
    S16(i) => {
      guard s16_in_range(i) else {
        raise EncodeError("s16 value out of range: \{i}")
      }
      @types.S16Value(i)
    }
    S32(i) => @types.S32Value(i)
    S64(i) => @types.S64Value(i)
    U8(b) => @types.U8Value(b)
    U16(u) => {
      guard u16_in_range(u) else {
        raise EncodeError("u16 value out of range: \{u}")
      }
      @types.U16Value(u)
    }
    U32(u) => @types.U32Value(u)
    U64(u) => @types.U64Value(u)
    F32(f) => @types.F32Value(f)
    F64(d) => @types.F64Value(d)
    Char(c) => @types.CharValue(c)
    String(s) => @types.StringValue(s)
    Record(fields) => @types.RecordValue(fields.map(f => self.emit(f)))
    Variant(case_, payload) =>
      @types.VariantValue({ case: case_, payload: self.emit_opt(payload) })
    Enum(case_) => @types.EnumValue(case_)
    Flags(flags) => @types.FlagsValue(flags)
    Tuple(elements) => @types.TupleValue(elements.map(e => self.emit(e)))
    List(elements) => @types.ListValue(elements.map(e => self.emit(e)))
    FixedList(elements) =>
      @types.FixedListValue(elements.map(e => self.emit(e)))
    Map(entries) =>
      @types.MapValue(
        entries.map(e => {
          (
            { key: self.emit(e.key), value: self.emit(e.value) } :
            @types.MapEntry)
        }),
      )
    Option(o) => @types.OptionValue(self.emit_opt(o))
    ResultOk(o) => @types.ResultValue(@types.OkValue(self.emit_opt(o)))
    ResultErr(o) => @types.ResultValue(@types.ErrValue(self.emit_opt(o)))
    Text(text, language) => @types.TextValue({ text, language })
    Binary(bytes, mime_type) => @types.BinaryValue({ bytes, mime_type })
    Path(s) => @types.PathValue(s)
    Url(s) => @types.UrlValue(s)
    Datetime(dt) => {
      guard datetime_valid(dt) else {
        raise EncodeError(
          "invalid datetime value: nanoseconds must be < 1_000_000_000, got \{dt.nanoseconds}",
        )
      }
      @types.DatetimeValue(dt)
    }
    Duration(ns) => @types.DurationValue({ nanoseconds: ns })
    Quantity(q) => @types.QuantityValueNode(q)
    Union(tag, body) => @types.UnionValue({ tag, body: self.emit(body) })
    Secret(_) =>
      raise EncodeError(
        "secret values can only be encoded through a secret resolver-aware path",
      )
    QuotaToken(handle) =>
      // Move the owned `quota-token` resource out of the opaque handle exactly
      // once. `schema_value_to_wit` preflights that every handle is present and
      // unique, so this take always succeeds for a well-formed value tree.
      match handle.take() {
        Some(raw) => @types.QuotaTokenHandle(raw)
        None =>
          raise EncodeError(
            "quota-token handle was already transferred; an owned quota-token can only be sent once",
          )
      }
  }
}

///|
/// Encode a recursive `SchemaValue` into its flat `@types.SchemaValueTree`.
///
/// Any owned `quota-token` handle in `value` is moved into the wire tree as a
/// `quota-token-handle` node (first encode wins); after this the originating
/// handle is empty. A full validation preflight runs first so that *every*
/// encode-time error — an aliased or already-transferred `quota-token`, or an
/// out-of-range narrow integer / invalid datetime elsewhere in the tree — is
/// raised *before* any handle is moved, keeping a failed encode from destroying
/// a still-valid token.
pub fn schema_value_to_wit(
  value : SchemaValue,
) -> @types.SchemaValueTree raise SchemaModelError {
  preflight_value(value)
  let enc = ValueEncoder::new()
  let root = enc.emit(value)
  { value_nodes: enc.value_nodes, root }
}

///|
/// Validate every encode-time invariant of `value` before `schema_value_to_wit`
/// moves any owned `quota-token` handle, so a rejected value tree never destroys
/// a still-valid token. This enforces the same narrow-integer and datetime
/// ranges `emit_node` does, plus that every `quota-token` is still present and
/// that no owned handle — compared by its underlying resource identity, not just
/// holder identity — appears more than once in the tree.
fn preflight_value(value : SchemaValue) -> Unit raise SchemaModelError {
  let seen : Array[@types.QuotaToken] = []
  preflight_walk(value, seen)
}

///|
fn preflight_walk(
  value : SchemaValue,
  seen : Array[@types.QuotaToken],
) -> Unit raise SchemaModelError {
  match value {
    S8(i) =>
      if not(s8_in_range(i)) {
        raise EncodeError("s8 value out of range: \{i}")
      }
    S16(i) =>
      if not(s16_in_range(i)) {
        raise EncodeError("s16 value out of range: \{i}")
      }
    U16(u) =>
      if not(u16_in_range(u)) {
        raise EncodeError("u16 value out of range: \{u}")
      }
    Datetime(dt) =>
      if not(datetime_valid(dt)) {
        raise EncodeError(
          "invalid datetime value: nanoseconds must be < 1_000_000_000, got \{dt.nanoseconds}",
        )
      }
    Secret(_) =>
      raise EncodeError(
        "secret values can only be encoded through a secret resolver-aware path",
      )
    QuotaToken(handle) => {
      // Peek (without consuming) the underlying owned resource so two distinct
      // holders wrapping the same raw `quota-token` are also rejected, not only
      // the same holder used twice.
      let raw = match handle.with_handle(r => r) {
        Some(raw) => raw
        None =>
          raise EncodeError(
            "quota-token handle was already transferred; an owned quota-token can only be sent once",
          )
      }
      for s in seen {
        if s == raw {
          raise EncodeError(
            "the same quota-token handle appeared more than once in one value tree",
          )
        }
      }
      seen.push(raw)
    }
    Record(fields) =>
      for f in fields {
        preflight_walk(f, seen)
      }
    Variant(_, payload) =>
      match payload {
        Some(p) => preflight_walk(p, seen)
        None => ()
      }
    Tuple(elements) =>
      for e in elements {
        preflight_walk(e, seen)
      }
    List(elements) =>
      for e in elements {
        preflight_walk(e, seen)
      }
    FixedList(elements) =>
      for e in elements {
        preflight_walk(e, seen)
      }
    Map(entries) =>
      for e in entries {
        preflight_walk(e.key, seen)
        preflight_walk(e.value, seen)
      }
    Option(o) =>
      match o {
        Some(x) => preflight_walk(x, seen)
        None => ()
      }
    ResultOk(o) =>
      match o {
        Some(x) => preflight_walk(x, seen)
        None => ()
      }
    ResultErr(o) =>
      match o {
        Some(x) => preflight_walk(x, seen)
        None => ()
      }
    Union(_, body) => preflight_walk(body, seen)
    _ => ()
  }
}

// ============================================================
// Schema value — decode (flat -> recursive)
// ============================================================

///|
priv struct ValueDecoder {
  nodes : Array[@types.SchemaValueNode]
  on_path : Array[Bool]
  // An owned `quota-token` handle node may be lifted into the value tree at most
  // once; track which handle nodes have already been claimed so a malformed tree
  // that references the same handle node twice cannot wrap one owned resource in
  // two handles.
  lifted : Array[Bool]
  // Underlying owned resources already lifted, so a malformed tree that carries
  // the same raw `quota-token` in two *different* handle nodes is also rejected.
  lifted_raws : Array[@types.QuotaToken]
}

///|
fn ValueDecoder::new(nodes : Array[@types.SchemaValueNode]) -> ValueDecoder {
  {
    nodes,
    on_path: Array::make(nodes.length(), false),
    lifted: Array::make(nodes.length(), false),
    lifted_raws: [],
  }
}

///|
/// Return the index of the first owned `quota-token` handle node that was never
/// lifted into the decoded value tree, or `None` if every handle node was
/// referenced exactly once from the root. Used after a successful decode to
/// reject a tree that carries handles unreachable from the root.
///
/// This only inspects the node tags; it does not release the owned resources.
/// MoonBit has no resource finalizer, so an owned `quota-token` left in a
/// rejected or partially-decoded tree is released only when the guest instance
/// is torn down. (Other SDKs lean on host/runtime GC for the same cleanup; a
/// guest-side host `resource-drop` call here would pull the `golem:core/types`
/// host import into this otherwise host-free package.)
fn ValueDecoder::first_unreferenced_handle(self : ValueDecoder) -> Int? {
  for i in 0..<self.nodes.length() {
    if self.nodes[i] is @types.QuotaTokenHandle(_) && not(self.lifted[i]) {
      return Some(i)
    }
  }
  None
}

///|
fn ValueDecoder::decode_opt(
  self : ValueDecoder,
  o : Int?,
) -> SchemaValue? raise SchemaModelError {
  match o {
    Some(i) => Some(self.from_idx(i))
    None => None
  }
}

///|
fn ValueDecoder::from_idx(
  self : ValueDecoder,
  idx : Int,
) -> SchemaValue raise SchemaModelError {
  if idx < 0 || idx >= self.nodes.length() {
    raise DecodeError(
      "value node index out of range: \{idx} (nodes: \{self.nodes.length()})",
    )
  }
  if self.on_path[idx] {
    raise DecodeError("cyclic value node reference at index \{idx}")
  }
  self.on_path[idx] = true
  let result = self.from_node(idx, self.nodes[idx])
  self.on_path[idx] = false
  result
}

///|
fn ValueDecoder::from_node(
  self : ValueDecoder,
  idx : Int,
  n : @types.SchemaValueNode,
) -> SchemaValue raise SchemaModelError {
  match n {
    @types.BoolValue(b) => Bool(b)
    @types.S8Value(i) => {
      guard s8_in_range(i) else {
        raise DecodeError("s8 value out of range: \{i}")
      }
      S8(i)
    }
    @types.S16Value(i) => {
      guard s16_in_range(i) else {
        raise DecodeError("s16 value out of range: \{i}")
      }
      S16(i)
    }
    @types.S32Value(i) => S32(i)
    @types.S64Value(i) => S64(i)
    @types.U8Value(b) => U8(b)
    @types.U16Value(u) => {
      guard u16_in_range(u) else {
        raise DecodeError("u16 value out of range: \{u}")
      }
      U16(u)
    }
    @types.U32Value(u) => U32(u)
    @types.U64Value(u) => U64(u)
    @types.F32Value(f) => F32(f)
    @types.F64Value(d) => F64(d)
    @types.CharValue(c) => Char(c)
    @types.StringValue(s) => String(s)
    @types.RecordValue(fields) => Record(fields.map(i => self.from_idx(i)))
    @types.VariantValue(vp) => Variant(vp.case, self.decode_opt(vp.payload))
    @types.EnumValue(case_) => Enum(case_)
    @types.FlagsValue(flags) => Flags(flags)
    @types.TupleValue(elements) => Tuple(elements.map(i => self.from_idx(i)))
    @types.ListValue(elements) => List(elements.map(i => self.from_idx(i)))
    @types.FixedListValue(elements) =>
      FixedList(elements.map(i => self.from_idx(i)))
    @types.MapValue(entries) =>
      Map(
        entries.map(e => {
          (
            { key: self.from_idx(e.key), value: self.from_idx(e.value) } :
            SchemaMapEntry)
        }),
      )
    @types.OptionValue(o) => Option(self.decode_opt(o))
    @types.ResultValue(rp) =>
      match rp {
        @types.OkValue(o) => ResultOk(self.decode_opt(o))
        @types.ErrValue(o) => ResultErr(self.decode_opt(o))
      }
    @types.TextValue(tp) => Text(tp.text, tp.language)
    @types.BinaryValue(bp) => Binary(bp.bytes, bp.mime_type)
    @types.PathValue(s) => Path(s)
    @types.UrlValue(s) => Url(s)
    @types.DatetimeValue(dt) => {
      guard datetime_valid(dt) else {
        raise DecodeError(
          "invalid datetime value: nanoseconds must be < 1_000_000_000, got \{dt.nanoseconds}",
        )
      }
      Datetime(dt)
    }
    @types.DurationValue(dp) => Duration(dp.nanoseconds)
    @types.QuantityValueNode(q) => Quantity(q)
    @types.UnionValue(up) => Union(up.tag, self.from_idx(up.body))
    @types.SecretValue(_) =>
      raise DecodeError(
        "secret values can only be decoded through a secret resolver-aware path",
      )
    @types.QuotaTokenHandle(raw) => {
      // Lift the owned `quota-token` resource into an opaque take-once holder.
      // Track the originating node index as lifted so a malformed tree that
      // references the same handle node twice cannot wrap one owned resource in
      // two holders (the underlying `@types.QuotaToken` is a copyable value
      // wrapper, so affine ownership is enforced here, not by the language).
      guard not(self.lifted[idx]) else {
        raise DecodeError(
          "quota-token handle node referenced more than once at index \{idx}; an owned quota-token can only be lifted once",
        )
      }
      // Also reject the same underlying owned resource appearing in two distinct
      // handle nodes, which would otherwise wrap one resource in two holders.
      for r in self.lifted_raws {
        if r == raw {
          raise DecodeError(
            "the same owned quota-token resource appeared in more than one handle node",
          )
        }
      }
      self.lifted[idx] = true
      self.lifted_raws.push(raw)
      QuotaToken(GuestQuotaTokenHandle::new(raw))
    }
  }
}

///|
/// Decode a flat `@types.SchemaValueTree` into the recursive `SchemaValue`.
///
/// Owned `quota-token` handle nodes are lifted into opaque take-once holders as
/// the tree is walked: each handle node may be referenced at most once (a second
/// reference is rejected), and a valid tree must reference every handle node
/// exactly once from the root. After a successful decode any handle node left
/// unreferenced was unreachable and the tree is rejected as malformed.
///
/// Owned resources in a rejected or partially-decoded tree are not explicitly
/// released here (see `first_unreferenced_handle`); MoonBit has no resource
/// finalizer, so they are reclaimed when the guest instance is torn down.
pub fn schema_value_from_wit(
  wit : @types.SchemaValueTree,
) -> SchemaValue raise SchemaModelError {
  let decoder = ValueDecoder::new(wit.value_nodes)
  let result = decoder.from_idx(wit.root)
  match decoder.first_unreferenced_handle() {
    Some(idx) =>
      raise DecodeError(
        "quota-token handle node at index \{idx} is not referenced from the root",
      )
    None => result
  }
}

// ============================================================
// Typed schema value
// ============================================================

///|
/// Encode a recursive `TypedSchemaValue` into its flat carrier.
pub fn typed_schema_value_to_wit(
  tv : TypedSchemaValue,
) -> @types.TypedSchemaValue raise SchemaModelError {
  { graph: schema_graph_to_wit(tv.graph), value: schema_value_to_wit(tv.value) }
}

///|
/// Decode a flat `@types.TypedSchemaValue` into the recursive form.
pub fn typed_schema_value_from_wit(
  wit : @types.TypedSchemaValue,
) -> TypedSchemaValue raise SchemaModelError {
  {
    graph: schema_graph_from_wit(wit.graph),
    value: schema_value_from_wit(wit.value),
  }
}
