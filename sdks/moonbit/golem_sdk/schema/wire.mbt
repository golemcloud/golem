///|
/// Direct conversion failure for the schema-value wire carrier.
pub(all) suberror WireError {
  Invalid(String)
} derive(Debug, Eq)

///|
pub(all) struct WireEncoder {
  nodes : Array[@types.SchemaValueNode]
  priv checks : Array[() -> Bool]
  priv transfers : Array[() -> Unit]
  priv streams : Array[(Int, @model.GuestSchemaValueStreamHandle)]
}

///|
pub fn WireEncoder::new() -> WireEncoder {
  { nodes: [], checks: [], transfers: [], streams: [], }
}

///|
pub fn WireEncoder::push(
  self : WireEncoder,
  node : @types.SchemaValueNode,
) -> Int {
  let index = self.nodes.length()
  self.nodes.push(node)
  index
}

///|
pub(open) trait IntoWire {
  fn append_wire(Self, WireEncoder) -> Int raise WireError
  fn append_result_payload(Self, WireEncoder) -> Int? raise WireError = _
  fn release_wire(Self, WireReleaser) -> Unit = _
}

///|
impl IntoWire with fn release_wire(_self, _releaser) {
  ()
}

///|
/// Collects the resources that a value releases. A rollback of a failed
/// invocation-input encode keeps permission cards with their holders, because
/// the host never took them; every other release drops them.
pub struct WireReleaser {
  resources : Array[@types.SchemaValueNode]
  native_streams : Array[@async-core.Stream[@types.SchemaValueTree]]
  keeps_permission_cards : Bool
}

///|
fn WireReleaser::resource(
  self : WireReleaser,
  node : @types.SchemaValueNode,
) -> Unit {
  if !self.resources.contains(node) {
    self.resources.push(node)
  }
}

///|
/// Releases concrete values without encoding them or allocating a schema model.
pub fn[T : IntoWire] release_wire(value : T) -> Unit {
  let releaser : WireReleaser = {
    resources: [],
    native_streams: [],
    keeps_permission_cards: false,
  }
  T::release_wire(value, releaser)
  for node in releaser.resources {
    drop_wire_resource(node)
  }
  for stream in releaser.native_streams {
    stream.drop_sync()
  }
}

///|
pub(open) trait FromWire {
  fn read_wire(WireReader, Int) -> Self raise WireError
  fn read_result_payload(WireReader, Int?) -> Self raise WireError = _
}

///|
impl IntoWire with fn append_result_payload(self, encoder) {
  Some(Self::append_wire(self, encoder))
}

///|
impl FromWire with fn read_result_payload(reader, index) {
  Self::read_wire(reader, wire_payload(index))
}

///|
pub fn wire_payload(index : Int?) -> Int raise WireError {
  match index {
    Some(index) => index
    None => raise WireError::Invalid("missing required payload")
  }
}

///|
/// Rejects a string that holds an unpaired surrogate, because it is not valid
/// Unicode text.
fn wire_text(value : String, what : String) -> String raise WireError {
  guard @model.is_unicode_text(value) else {
    raise WireError::Invalid("\{what} value contains an unpaired surrogate")
  }
  value
}

///|
pub fn wire_no_payload(index : Int?) -> Unit raise WireError {
  guard index is None else { raise WireError::Invalid("unexpected payload") }
}

///|
/// A decode transaction owns the entire carrier until every field has been
/// decoded and finish succeeds. Abort invalidates lifted holders before
/// dropping each distinct raw capability, including unreachable ones. Finish
/// drops the capabilities of discarded subtrees.
pub struct WireReader {
  tree : @types.SchemaValueTree
  visited : Array[Bool]
  resources : Array[@types.SchemaValueNode]
  discarded : Array[@types.SchemaValueNode]
  rollback : Array[() -> Unit]
  drop_resource : (@types.SchemaValueNode) -> Unit
  mut settled : Bool
}

///|
fn wire_is_resource(node : @types.SchemaValueNode) -> Bool {
  match node {
    @types.SecretValue(_)
    | @types.QuotaTokenHandle(_)
    | @types.PermissionCardHandle(_)
    | @types.StreamValue(_) => true
    _ => false
  }
}

///|
fn drop_wire_resource(node : @types.SchemaValueNode) -> Unit {
  match node {
    @types.SecretValue(raw) => raw.drop()
    @types.QuotaTokenHandle(raw) => raw.drop()
    @types.PermissionCardHandle(raw) => @card.drop_unless_held(raw)
    @types.StreamValue(raw) => raw.drop()
    _ => ()
  }
}

///|
pub fn WireReader::new(
  tree : @types.SchemaValueTree,
  drop_resource? : (@types.SchemaValueNode) -> Unit = drop_wire_resource,
) -> WireReader {
  {
    tree,
    visited: Array::make(tree.value_nodes.length(), false),
    resources: [],
    discarded: [],
    rollback: [],
    drop_resource,
    settled: false,
  }
}

///|
pub fn WireReader::abort(self : WireReader) -> Unit {
  if self.settled {
    return
  }
  self.settled = true
  for undo in self.rollback {
    undo()
  }
  self.rollback.clear()
  let dropped : Array[@types.SchemaValueNode] = []
  for node in self.tree.value_nodes {
    if wire_is_resource(node) && !dropped.contains(node) {
      dropped.push(node)
      (self.drop_resource)(node)
    }
  }
}

///|
pub fn WireReader::finish(self : WireReader) -> Unit raise WireError {
  guard !self.settled else {
    raise WireError::Invalid("wire reader has already settled")
  }
  errdefer self.abort()
  for i, node in self.tree.value_nodes {
    guard self.visited[i] || !wire_is_resource(node) else {
      raise WireError::Invalid("unreachable resource at wire node \{i}")
    }
  }
  self.settled = true
  self.rollback.clear()
  for node in self.discarded {
    (self.drop_resource)(node)
  }
  self.discarded.clear()
}

///|
pub fn[T : IntoWire] into_wire(
  value : T,
) -> @types.SchemaValueTree raise WireError {
  let encoder = WireEncoder::new()
  let root = T::append_wire(value, encoder)
  encoder.finish(root)
}

///|
pub async fn[T : IntoWire] into_wire_async(value : T) -> @types.SchemaValueTree {
  let encoder = WireEncoder::new()
  let root = T::append_wire(value, encoder)
  encoder.finish_async(root)
}

///|
/// Encode heterogeneous invocation input fields as one ownership transaction.
/// A failed encode rolls the fields back with `rollback_wire_fields`.
pub fn into_wire_record(
  values : Array[&IntoWire],
) -> @types.SchemaValueTree raise WireError {
  errdefer rollback_wire_fields(values)
  let encoder = WireEncoder::new()
  let fields = values.map(value => value.append_wire(encoder))
  let root = encoder.push(@types.RecordValue(fields))
  encoder.finish(root)
}

///|
pub async fn into_wire_record_async(
  values : Array[&IntoWire],
) -> @types.SchemaValueTree {
  errdefer rollback_wire_fields(values)
  let encoder = WireEncoder::new()
  let fields = values.map(value => value.append_wire(encoder))
  let root = encoder.push(@types.RecordValue(fields))
  encoder.finish_async(root)
}

///|
/// Encodes the fields of an output payload, such as a multi-field tool error,
/// as one ownership transaction. A failed encode releases every field
/// resource, permission cards included, because nothing else owns the
/// discarded payload.
pub async fn into_wire_payload_record_async(
  values : Array[&IntoWire],
) -> @types.SchemaValueTree {
  errdefer release_wire_values(values, false)
  let encoder = WireEncoder::new()
  let fields = values.map(value => value.append_wire(encoder))
  let root = encoder.push(@types.RecordValue(fields))
  encoder.finish_async(root)
}

///|
/// Rolls back invocation input fields that were not sent: releases their
/// resources except permission cards, which stay with their holders because
/// the host never took them.
pub fn rollback_wire_fields(values : Array[&IntoWire]) -> Unit {
  release_wire_values(values, true)
}

///|
fn release_wire_values(
  values : Array[&IntoWire],
  keeps_permission_cards : Bool,
) -> Unit {
  let releaser : WireReleaser = {
    resources: [],
    native_streams: [],
    keeps_permission_cards,
  }
  for value in values {
    value.release_wire(releaser)
  }
  for node in releaser.resources {
    drop_wire_resource(node)
  }
  for stream in releaser.native_streams {
    stream.drop_sync()
  }
}

///|
async fn WireEncoder::finish_async(
  encoder : WireEncoder,
  root : Int,
) -> @types.SchemaValueTree {
  encoder.preflight()
  for entry in encoder.streams {
    let (_, stream) = entry
    stream.prepare_outgoing()
  }
  encoder.finish(root)
}

///|
fn WireEncoder::preflight(encoder : WireEncoder) -> Unit raise WireError {
  let resources : Map[(Int, Int), Bool] = Map([])
  for node in encoder.nodes {
    let identity = match node {
      @types.SecretValue(@types.Secret::Secret(raw)) => Some((0, raw))
      @types.QuotaTokenHandle(@types.QuotaToken::QuotaToken(raw)) =>
        Some((1, raw))
      @types.PermissionCardHandle(@types.PermissionCard::PermissionCard(raw)) =>
        Some((2, raw))
      @types.StreamValue(@types.SchemaValueStream::SchemaValueStream(raw)) =>
        Some((3, raw))
      _ => None
    }
    if identity is Some(key) {
      guard !resources.contains(key) else {
        raise WireError::Invalid(
          "owned resource appears in multiple wire nodes",
        )
      }
      resources[key] = true
    }
  }
  for check in encoder.checks {
    guard check() else {
      raise WireError::Invalid("resource has already been transferred")
    }
  }
  let wrapped_streams : Map[Int, Bool] = Map([])
  for i, entry in encoder.streams {
    let (_, stream) = entry
    guard stream.is_present() else {
      raise WireError::Invalid("stream has already been transferred")
    }
    match stream.wrapped() {
      Some(@types.SchemaValueStream::SchemaValueStream(raw)) => {
        guard !wrapped_streams.contains(raw) else {
          raise WireError::Invalid("owned stream appears more than once")
        }
        wrapped_streams[raw] = true
      }
      None => preflight_native_stream(encoder.streams, i, stream)
    }
  }
}

///|
/// Native readers expose endpoint equality but no resource identity key. Keep
/// the pairwise loop separate from hash-backed preflight for Wasm optimization.
fn preflight_native_stream(
  streams : Array[(Int, @model.GuestSchemaValueStreamHandle)],
  before : Int,
  stream : @model.GuestSchemaValueStreamHandle,
) -> Unit raise WireError {
  for j in 0..<before {
    guard !stream.same_endpoint(streams[j].1) else {
      raise WireError::Invalid("owned stream appears more than once")
    }
  }
}

///|
fn WireEncoder::finish(
  encoder : WireEncoder,
  root : Int,
) -> @types.SchemaValueTree raise WireError {
  for entry in encoder.streams {
    let (index, stream) = entry
    guard stream.wrapped() is Some(raw) else {
      raise WireError::Invalid("native stream requires asynchronous encoding")
    }
    encoder.nodes[index] = @types.StreamValue(raw)
  }
  encoder.preflight()
  for transfer in encoder.transfers {
    transfer()
  }
  for entry in encoder.streams {
    let (_, stream) = entry
    ignore(stream.take_wrapped())
  }
  { value_nodes: encoder.nodes, root, }
}

///|
pub fn[T : FromWire] from_wire(
  tree : @types.SchemaValueTree,
) -> T raise WireError {
  let reader = WireReader::new(tree)
  errdefer reader.abort()
  let value = T::read_wire(reader, tree.root)
  reader.finish()
  value
}

///|
pub fn wire_node(
  tree : WireReader,
  index : Int,
) -> @types.SchemaValueNode raise WireError {
  guard !tree.settled else {
    raise WireError::Invalid("wire reader has already settled")
  }
  match tree.tree.value_nodes.get(index) {
    Some(node) => {
      guard !tree.visited[index] else {
        raise WireError::Invalid("wire node \{index} referenced more than once")
      }
      tree.visited[index] = true
      if wire_is_resource(node) {
        guard !tree.resources.contains(node) else {
          raise WireError::Invalid(
            "owned resource appears in multiple wire nodes",
          )
        }
        tree.resources.push(node)
      }
      node
    }
    None =>
      raise WireError::Invalid("wire node index \{index} is out of bounds")
  }
}

///|
/// Claims the subtree at `index` without decoding it. Its owned resources are
/// dropped by finish, or by abort like every other resource of the carrier.
fn WireReader::discard(self : WireReader, index : Int) -> Unit raise WireError {
  let node = wire_node(self, index)
  if wire_is_resource(node) {
    self.discarded.push(node)
  }
  match node {
    @types.SchemaValueNode::RecordValue(children)
    | @types.SchemaValueNode::TupleValue(children)
    | @types.SchemaValueNode::ListValue(children)
    | @types.SchemaValueNode::FixedListValue(children) =>
      for child in children {
        self.discard(child)
      }
    @types.SchemaValueNode::MapValue(entries) =>
      for entry in entries {
        self.discard(entry.key)
        self.discard(entry.value)
      }
    @types.SchemaValueNode::VariantValue({ payload: Some(child), .. })
    | @types.SchemaValueNode::OptionValue(Some(child))
    | @types.SchemaValueNode::ResultValue(
      @types.ResultValuePayload::OkValue(Some(child))
    )
    | @types.SchemaValueNode::ResultValue(
      @types.ResultValuePayload::ErrValue(Some(child))
    )
    | @types.SchemaValueNode::UnionValue({ body: child, .. }) =>
      self.discard(child)
    _ => ()
  }
}

///|
pub fn wire_record(
  tree : WireReader,
  index : Int,
  arity : Int,
  context : String,
) -> Array[Int] raise WireError {
  match wire_node(tree, index) {
    @types.SchemaValueNode::RecordValue(fields) if fields.length() == arity =>
      fields
    @types.SchemaValueNode::RecordValue(fields) =>
      raise WireError::Invalid(
        "\{context}: expected \{arity} field(s), got \{fields.length()}",
      )
    _ => raise WireError::Invalid("\{context}: expected record value")
  }
}

///|
pub fn wire_variant(
  tree : WireReader,
  index : Int,
  cases : Int,
  context : String,
) -> @types.VariantValuePayload raise WireError {
  match wire_node(tree, index) {
    @types.SchemaValueNode::VariantValue(value) if value.case <
      cases.reinterpret_as_uint() => value
    @types.SchemaValueNode::VariantValue(value) =>
      raise WireError::Invalid(
        "\{context}: variant case \{value.case} is out of range",
      )
    _ => raise WireError::Invalid("\{context}: expected variant value")
  }
}

///|
pub impl IntoWire for String with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::StringValue(wire_text(self, "string")))
}

///|
pub impl FromWire for String with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::StringValue(value) => value
    _ => raise WireError::Invalid("expected string value")
  }
}

///|
pub impl IntoWire for Bool with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::BoolValue(self))
}

///|
pub impl FromWire for Bool with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::BoolValue(value) => value
    _ => raise WireError::Invalid("expected bool value")
  }
}

///|
pub impl IntoWire for Int with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::S32Value(self))
}

///|
pub impl FromWire for Int with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::S32Value(value) => value
    _ => raise WireError::Invalid("expected s32 value")
  }
}

///|
pub impl IntoWire for Int16 with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::S16Value(self.to_int()))
}

///|
pub impl FromWire for Int16 with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::S16Value(value) if value >= -32768 && value <= 32767 =>
      Int16::from_int(value)
    _ => raise WireError::Invalid("expected s16 value")
  }
}

///|
pub impl IntoWire for UInt16 with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::U16Value(self.to_uint()))
}

///|
pub impl FromWire for UInt16 with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::U16Value(value) if value <= 65535U =>
      value.to_uint16()
    _ => raise WireError::Invalid("expected u16 value")
  }
}

///|
pub impl IntoWire for UInt with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::U32Value(self))
}

///|
pub impl FromWire for UInt with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::U32Value(value) => value
    _ => raise WireError::Invalid("expected u32 value")
  }
}

///|
pub impl IntoWire for Int64 with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::S64Value(self))
}

///|
pub impl FromWire for Int64 with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::S64Value(value) => value
    _ => raise WireError::Invalid("expected s64 value")
  }
}

///|
pub impl IntoWire for UInt64 with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::U64Value(self))
}

///|
pub impl FromWire for UInt64 with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::U64Value(value) => value
    _ => raise WireError::Invalid("expected u64 value")
  }
}

///|
pub impl IntoWire for Float with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::F32Value(self))
}

///|
pub impl FromWire for Float with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::F32Value(value) => value
    _ => raise WireError::Invalid("expected f32 value")
  }
}

///|
pub impl IntoWire for Double with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::F64Value(self))
}

///|
pub impl FromWire for Double with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::F64Value(value) => value
    _ => raise WireError::Invalid("expected f64 value")
  }
}

///|
pub impl IntoWire for Byte with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::U8Value(self))
}

///|
pub impl FromWire for Byte with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::U8Value(value) => value
    _ => raise WireError::Invalid("expected u8 value")
  }
}

///|
pub impl IntoWire for Char with fn append_wire(self, encoder) {
  guard @model.is_unicode_scalar(self) else {
    raise WireError::Invalid(
      "char value is not a Unicode scalar value: \{self.to_int()}",
    )
  }
  encoder.push(@types.SchemaValueNode::CharValue(self))
}

///|
pub impl FromWire for Char with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::CharValue(value) => value
    _ => raise WireError::Invalid("expected char value")
  }
}

///|
pub impl IntoWire for Unit with fn append_wire(_self, encoder) {
  encoder.push(@types.SchemaValueNode::TupleValue([]))
}

///|
pub impl IntoWire for Unit with fn append_result_payload(_self, _encoder) {
  None
}

///|
pub impl FromWire for Unit with fn read_result_payload(_reader, index) {
  wire_no_payload(index)
}

///|
pub impl FromWire for Unit with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::TupleValue([]) => ()
    _ => raise WireError::Invalid("expected unit value")
  }
}

///|
pub impl[T : IntoWire] IntoWire for T? with fn append_wire(self, encoder) {
  let payload = match self {
    Some(value) => Some(T::append_wire(value, encoder))
    None => None
  }
  encoder.push(@types.SchemaValueNode::OptionValue(payload))
}

///|
pub impl[T : FromWire] FromWire for T? with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::OptionValue(Some(payload)) =>
      Some(T::read_wire(tree, payload))
    @types.SchemaValueNode::OptionValue(None) => None
    _ => raise WireError::Invalid("expected option value")
  }
}

///|
pub impl[T : IntoWire] IntoWire for Array[T] with fn append_wire(self, encoder) {
  let values = self.map(value => T::append_wire(value, encoder))
  encoder.push(@types.SchemaValueNode::ListValue(values))
}

///|
pub impl[T : FromWire] FromWire for Array[T] with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::ListValue(values) =>
      values.map(value => T::read_wire(tree, value))
    _ => raise WireError::Invalid("expected list value")
  }
}

///|
pub impl[T : IntoWire, E : IntoWire] IntoWire for Result[T, E] with fn append_wire(
  self,
  encoder,
) {
  let payload = match self {
    Ok(value) =>
      @types.ResultValuePayload::OkValue(
        T::append_result_payload(value, encoder),
      )
    Err(error) =>
      @types.ResultValuePayload::ErrValue(
        E::append_result_payload(error, encoder),
      )
  }
  encoder.push(@types.SchemaValueNode::ResultValue(payload))
}

///|
pub impl[T : FromWire, E : FromWire] FromWire for Result[T, E] with fn read_wire(
  tree,
  index,
) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::ResultValue(
      @types.ResultValuePayload::OkValue(value)
    ) => Ok(T::read_result_payload(tree, value))
    @types.SchemaValueNode::ResultValue(
      @types.ResultValuePayload::ErrValue(value)
    ) => Err(E::read_result_payload(tree, value))
    _ => raise WireError::Invalid("expected result value")
  }
}

///|
pub impl[K : IntoWire, V : IntoWire] IntoWire for Map[K, V] with fn append_wire(
  self,
  encoder,
) {
  let entries : Array[@types.MapEntry] = []
  for key, value in self {
    entries.push({
      key: K::append_wire(key, encoder),
      value: V::append_wire(value, encoder),
    })
  }
  encoder.push(@types.SchemaValueNode::MapValue(entries))
}

///|
/// A repeated key keeps the value of its last entry at the position of its
/// first entry; the values of earlier entries are not decoded.
pub impl[K : FromWire + Hash + Eq, V : FromWire] FromWire for Map[K, V] with fn read_wire(
  reader,
  index,
) {
  match wire_node(reader, index) {
    @types.SchemaValueNode::MapValue(entries) => {
      let keys : Array[K] = []
      let last : Map[K, Int] = Map([])
      for i, entry in entries {
        let key : K = FromWire::read_wire(reader, entry.key)
        keys.push(key)
        last[key] = i
      }
      let result : Map[K, V] = Map([])
      for i, key in keys {
        guard last.get(key) is Some(winner) else { continue }
        if !result.contains(key) {
          result[key] = V::read_wire(reader, entries[winner].value)
        }
        if winner != i {
          reader.discard(entries[i].value)
        }
      }
      result
    }
    _ => raise WireError::Invalid("expected map value")
  }
}

///|
pub impl IntoWire for Bytes with fn append_wire(self, encoder) {
  encoder.push(
    @types.SchemaValueNode::BinaryValue({
      bytes: self.to_fixedarray(),
      mime_type: None,
    }),
  )
}

///|
pub impl FromWire for Bytes with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::BinaryValue(value) =>
      Bytes::from_array(value.bytes[:])
    _ => raise WireError::Invalid("expected binary value")
  }
}

///|
pub impl IntoWire for Uuid with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::UuidValue(self.to_wit()))
}

///|
pub impl FromWire for Uuid with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::UuidValue(value) => Uuid::from_wit(value)
    _ => raise WireError::Invalid("expected uuid value")
  }
}

///|
pub impl IntoWire for Path with fn append_wire(self, encoder) {
  encoder.push(@types.SchemaValueNode::PathValue(wire_text(self.value, "path")))
}

///|
pub impl FromWire for Path with fn read_wire(tree, index) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::PathValue(value) => Path::new(value)
    _ => raise WireError::Invalid("expected path value")
  }
}

///|
pub impl FromWire for @model.GuestSecretHandle with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.SecretValue(raw) => {
      let handle = @model.GuestSecretHandle::new(raw)
      reader.rollback.push(() => ignore(handle.take()))
      handle
    }
    _ => raise WireError::Invalid("expected secret value")
  }
}

///|
pub impl IntoWire for @model.GuestSecretHandle with fn append_wire(
  self,
  encoder,
) {
  let raw = self.with_handle(raw => raw)
  guard raw is Some(raw) else {
    raise WireError::Invalid("secret has already been transferred")
  }
  encoder.checks.push(() => self.is_present())
  encoder.transfers.push(() => ignore(self.take()))
  encoder.push(@types.SecretValue(raw))
}

///|
pub impl FromWire for @model.GuestQuotaTokenHandle with fn read_wire(
  reader,
  index,
) {
  match wire_node(reader, index) {
    @types.QuotaTokenHandle(raw) => {
      let handle = @model.GuestQuotaTokenHandle::new(raw)
      reader.rollback.push(() => ignore(handle.take()))
      handle
    }
    _ => raise WireError::Invalid("expected quota token value")
  }
}

///|
pub impl IntoWire for @model.GuestQuotaTokenHandle with fn append_wire(
  self,
  encoder,
) {
  let raw = self.with_handle(raw => raw)
  guard raw is Some(raw) else {
    raise WireError::Invalid("quota token has already been transferred")
  }
  encoder.checks.push(() => self.is_present())
  encoder.transfers.push(() => ignore(self.take()))
  encoder.push(@types.QuotaTokenHandle(raw))
}

///|
pub impl FromWire for @card.GuestPermissionCardHandle with fn read_wire(
  reader,
  index,
) {
  match wire_node(reader, index) {
    @types.PermissionCardHandle(raw) => {
      // Lift only a handle that no card holds, so that a tree built by user
      // code cannot alias a card.
      guard @card.lift(raw) is Some(handle) else {
        raise WireError::Invalid(
          "permission card handle is already held by another card",
        )
      }
      reader.rollback.push(() => ignore(@card.take(handle)))
      handle
    }
    _ => raise WireError::Invalid("expected permission card value")
  }
}

///|
pub impl IntoWire for @card.GuestPermissionCardHandle with fn append_wire(
  self,
  encoder,
) {
  guard @card.borrow(self) is Some(raw) else {
    raise WireError::Invalid("permission card has already been transferred")
  }
  encoder.checks.push(() => self.is_present())
  encoder.transfers.push(() => ignore(@card.take(self)))
  encoder.push(@types.PermissionCardHandle(raw))
}

///|
pub impl FromWire for @model.GuestSchemaValueStreamHandle with fn read_wire(
  reader,
  index,
) {
  match wire_node(reader, index) {
    @types.StreamValue(raw) => {
      let handle = @model.GuestSchemaValueStreamHandle::from_wrapped(raw)
      reader.rollback.push(() => ignore(handle.take_wrapped()))
      handle
    }
    _ => raise WireError::Invalid("expected stream value")
  }
}

///|
pub impl IntoWire for @model.GuestSchemaValueStreamHandle with fn append_wire(
  self,
  encoder,
) {
  let index = encoder.push(@types.TupleValue([]))
  encoder.streams.push((index, self))
  index
}

///|
pub impl IntoWire for @model.GuestSecretHandle with fn release_wire(
  self,
  releaser,
) {
  if self.take() is Some(raw) {
    releaser.resource(@types.SecretValue(raw))
  }
}

///|
pub impl IntoWire for @model.GuestQuotaTokenHandle with fn release_wire(
  self,
  releaser,
) {
  if self.take() is Some(raw) {
    releaser.resource(@types.QuotaTokenHandle(raw))
  }
}

///|
/// A rollback of a failed invocation-input encode keeps the card with its
/// holder; every other release drops it.
pub impl IntoWire for @card.GuestPermissionCardHandle with fn release_wire(
  self,
  releaser,
) {
  if releaser.keeps_permission_cards {
    return
  }
  if @card.take(self) is Some(raw) {
    releaser.resource(@types.PermissionCardHandle(raw))
  }
}

///|
pub impl IntoWire for @model.GuestSchemaValueStreamHandle with fn release_wire(
  self,
  releaser,
) {
  self.release(raw => releaser.resource(@types.StreamValue(raw)), stream => {
    if !releaser.native_streams.any(existing => physical_equal(existing, stream)) {
      releaser.native_streams.push(stream)
    }
  })
}

///|
pub impl[T : IntoWire] IntoWire for T? with fn release_wire(self, releaser) {
  if self is Some(value) {
    T::release_wire(value, releaser)
  }
}

///|
pub impl[T : IntoWire] IntoWire for Array[T] with fn release_wire(
  self,
  releaser,
) {
  for value in self {
    T::release_wire(value, releaser)
  }
}

///|
pub impl[T : IntoWire, E : IntoWire] IntoWire for Result[T, E] with fn release_wire(
  self,
  releaser,
) {
  match self {
    Ok(value) => T::release_wire(value, releaser)
    Err(error) => E::release_wire(error, releaser)
  }
}

///|
pub impl[K : IntoWire, V : IntoWire] IntoWire for Map[K, V] with fn release_wire(
  self,
  releaser,
) {
  for key, value in self {
    K::release_wire(key, releaser)
    V::release_wire(value, releaser)
  }
}

///|
pub impl[A : IntoWire, B : IntoWire] IntoWire for (A, B) with fn release_wire(
  self,
  r,
) {
  A::release_wire(self.0, r)
  B::release_wire(self.1, r)
}

///|
pub impl[A : IntoWire, B : IntoWire, C : IntoWire] IntoWire for (A, B, C) with fn release_wire(
  self,
  r,
) {
  A::release_wire(self.0, r)
  B::release_wire(self.1, r)
  C::release_wire(self.2, r)
}

///|
pub impl[A : IntoWire, B : IntoWire, C : IntoWire, D : IntoWire] IntoWire for (
  A,
  B,
  C,
  D,
) with fn release_wire(self, r) {
  A::release_wire(self.0, r)
  B::release_wire(self.1, r)
  C::release_wire(self.2, r)
  D::release_wire(self.3, r)
}

///|
pub impl[A : IntoWire, B : IntoWire, C : IntoWire, D : IntoWire, E : IntoWire] IntoWire for (
  A,
  B,
  C,
  D,
  E,
) with fn release_wire(self, r) {
  A::release_wire(self.0, r)
  B::release_wire(self.1, r)
  C::release_wire(self.2, r)
  D::release_wire(self.3, r)
  E::release_wire(self.4, r)
}

///|
pub impl[
  A : IntoWire,
  B : IntoWire,
  C : IntoWire,
  D : IntoWire,
  E : IntoWire,
  F : IntoWire,
] IntoWire for (A, B, C, D, E, F) with fn release_wire(self, r) {
  A::release_wire(self.0, r)
  B::release_wire(self.1, r)
  C::release_wire(self.2, r)
  D::release_wire(self.3, r)
  E::release_wire(self.4, r)
  F::release_wire(self.5, r)
}

///|
pub impl[
  A : IntoWire,
  B : IntoWire,
  C : IntoWire,
  D : IntoWire,
  E : IntoWire,
  F : IntoWire,
  G : IntoWire,
] IntoWire for (A, B, C, D, E, F, G) with fn release_wire(self, r) {
  A::release_wire(self.0, r)
  B::release_wire(self.1, r)
  C::release_wire(self.2, r)
  D::release_wire(self.3, r)
  E::release_wire(self.4, r)
  F::release_wire(self.5, r)
  G::release_wire(self.6, r)
}

///|
pub impl[
  A : IntoWire,
  B : IntoWire,
  C : IntoWire,
  D : IntoWire,
  E : IntoWire,
  F : IntoWire,
  G : IntoWire,
  H : IntoWire,
] IntoWire for (A, B, C, D, E, F, G, H) with fn release_wire(self, r) {
  A::release_wire(self.0, r)
  B::release_wire(self.1, r)
  C::release_wire(self.2, r)
  D::release_wire(self.3, r)
  E::release_wire(self.4, r)
  F::release_wire(self.5, r)
  G::release_wire(self.6, r)
  H::release_wire(self.7, r)
}

///|
pub impl[A : IntoWire, B : IntoWire] IntoWire for (A, B) with fn append_wire(
  self,
  encoder,
) {
  let (a, b) = self
  encoder.push(
    @types.SchemaValueNode::TupleValue([
      A::append_wire(a, encoder),
      B::append_wire(b, encoder),
    ]),
  )
}

///|
pub impl[A : FromWire, B : FromWire] FromWire for (A, B) with fn read_wire(
  tree,
  index,
) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::TupleValue([a, b]) =>
      (A::read_wire(tree, a), B::read_wire(tree, b))
    _ => raise WireError::Invalid("expected 2-tuple value")
  }
}

///|
pub impl[A : IntoWire, B : IntoWire, C : IntoWire] IntoWire for (A, B, C) with fn append_wire(
  self,
  encoder,
) {
  let (a, b, c) = self
  encoder.push(
    @types.SchemaValueNode::TupleValue([
      A::append_wire(a, encoder),
      B::append_wire(b, encoder),
      C::append_wire(c, encoder),
    ]),
  )
}

///|
pub impl[A : FromWire, B : FromWire, C : FromWire] FromWire for (A, B, C) with fn read_wire(
  tree,
  index,
) {
  match wire_node(tree, index) {
    @types.SchemaValueNode::TupleValue([a, b, c]) =>
      (A::read_wire(tree, a), B::read_wire(tree, b), C::read_wire(tree, c))
    _ => raise WireError::Invalid("expected 3-tuple value")
  }
}

///|
pub impl[A : IntoWire, B : IntoWire, C : IntoWire, D : IntoWire] IntoWire for (
  A,
  B,
  C,
  D,
) with fn append_wire(self, encoder) {
  let (a, b, c, d) = self
  encoder.push(
    @types.TupleValue([
      A::append_wire(a, encoder),
      B::append_wire(b, encoder),
      C::append_wire(c, encoder),
      D::append_wire(d, encoder),
    ]),
  )
}

///|
pub impl[A : FromWire, B : FromWire, C : FromWire, D : FromWire] FromWire for (
  A,
  B,
  C,
  D,
) with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.TupleValue([a, b, c, d]) =>
      (
        A::read_wire(reader, a),
        B::read_wire(reader, b),
        C::read_wire(reader, c),
        D::read_wire(reader, d),
      )
    _ => raise WireError::Invalid("expected 4-tuple value")
  }
}

///|
pub impl[A : IntoWire, B : IntoWire, C : IntoWire, D : IntoWire, E : IntoWire] IntoWire for (
  A,
  B,
  C,
  D,
  E,
) with fn append_wire(self, encoder) {
  let (a, b, c, d, e) = self
  encoder.push(
    @types.TupleValue([
      A::append_wire(a, encoder),
      B::append_wire(b, encoder),
      C::append_wire(c, encoder),
      D::append_wire(d, encoder),
      E::append_wire(e, encoder),
    ]),
  )
}

///|
pub impl[A : FromWire, B : FromWire, C : FromWire, D : FromWire, E : FromWire] FromWire for (
  A,
  B,
  C,
  D,
  E,
) with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.TupleValue([a, b, c, d, e]) =>
      (
        A::read_wire(reader, a),
        B::read_wire(reader, b),
        C::read_wire(reader, c),
        D::read_wire(reader, d),
        E::read_wire(reader, e),
      )
    _ => raise WireError::Invalid("expected 5-tuple value")
  }
}

///|
pub impl[
  A : IntoWire,
  B : IntoWire,
  C : IntoWire,
  D : IntoWire,
  E : IntoWire,
  F : IntoWire,
] IntoWire for (A, B, C, D, E, F) with fn append_wire(self, encoder) {
  let (a, b, c, d, e, f) = self
  encoder.push(
    @types.TupleValue([
      A::append_wire(a, encoder),
      B::append_wire(b, encoder),
      C::append_wire(c, encoder),
      D::append_wire(d, encoder),
      E::append_wire(e, encoder),
      F::append_wire(f, encoder),
    ]),
  )
}

///|
pub impl[
  A : FromWire,
  B : FromWire,
  C : FromWire,
  D : FromWire,
  E : FromWire,
  F : FromWire,
] FromWire for (A, B, C, D, E, F) with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.TupleValue([a, b, c, d, e, f]) =>
      (
        A::read_wire(reader, a),
        B::read_wire(reader, b),
        C::read_wire(reader, c),
        D::read_wire(reader, d),
        E::read_wire(reader, e),
        F::read_wire(reader, f),
      )
    _ => raise WireError::Invalid("expected 6-tuple value")
  }
}

///|
pub impl[
  A : IntoWire,
  B : IntoWire,
  C : IntoWire,
  D : IntoWire,
  E : IntoWire,
  F : IntoWire,
  G : IntoWire,
] IntoWire for (A, B, C, D, E, F, G) with fn append_wire(self, encoder) {
  let (a, b, c, d, e, f, g) = self
  encoder.push(
    @types.TupleValue([
      A::append_wire(a, encoder),
      B::append_wire(b, encoder),
      C::append_wire(c, encoder),
      D::append_wire(d, encoder),
      E::append_wire(e, encoder),
      F::append_wire(f, encoder),
      G::append_wire(g, encoder),
    ]),
  )
}

///|
pub impl[
  A : FromWire,
  B : FromWire,
  C : FromWire,
  D : FromWire,
  E : FromWire,
  F : FromWire,
  G : FromWire,
] FromWire for (A, B, C, D, E, F, G) with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.TupleValue([a, b, c, d, e, f, g]) =>
      (
        A::read_wire(reader, a),
        B::read_wire(reader, b),
        C::read_wire(reader, c),
        D::read_wire(reader, d),
        E::read_wire(reader, e),
        F::read_wire(reader, f),
        G::read_wire(reader, g),
      )
    _ => raise WireError::Invalid("expected 7-tuple value")
  }
}

///|
pub impl[
  A : IntoWire,
  B : IntoWire,
  C : IntoWire,
  D : IntoWire,
  E : IntoWire,
  F : IntoWire,
  G : IntoWire,
  H : IntoWire,
] IntoWire for (A, B, C, D, E, F, G, H) with fn append_wire(self, encoder) {
  let (a, b, c, d, e, f, g, h) = self
  encoder.push(
    @types.TupleValue([
      A::append_wire(a, encoder),
      B::append_wire(b, encoder),
      C::append_wire(c, encoder),
      D::append_wire(d, encoder),
      E::append_wire(e, encoder),
      F::append_wire(f, encoder),
      G::append_wire(g, encoder),
      H::append_wire(h, encoder),
    ]),
  )
}

///|
pub impl[
  A : FromWire,
  B : FromWire,
  C : FromWire,
  D : FromWire,
  E : FromWire,
  F : FromWire,
  G : FromWire,
  H : FromWire,
] FromWire for (A, B, C, D, E, F, G, H) with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.TupleValue([a, b, c, d, e, f, g, h]) =>
      (
        A::read_wire(reader, a),
        B::read_wire(reader, b),
        C::read_wire(reader, c),
        D::read_wire(reader, d),
        E::read_wire(reader, e),
        F::read_wire(reader, f),
        G::read_wire(reader, g),
        H::read_wire(reader, h),
      )
    _ => raise WireError::Invalid("expected 8-tuple value")
  }
}
