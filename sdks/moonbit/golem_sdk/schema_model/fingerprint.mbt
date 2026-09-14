///|
/// A BLAKE3-256 fingerprint of the deterministic-CBOR encoding of a schema
/// closure. The byte representation is always 32 bytes.
pub(all) struct SchemaFingerprintV1(FixedArray[Byte]) derive(Eq, Debug)

///|
pub suberror SchemaFingerprintError {
  InvalidSchema(String)
  DuplicateSetValue(field~ : String, value~ : String)
} derive(Eq, Debug)

///|
pub fn SchemaFingerprintV1::as_bytes(
  self : SchemaFingerprintV1,
) -> FixedArray[Byte] {
  self.0.copy()
}

///|
pub fn SchemaFingerprintV1::to_hex(self : SchemaFingerprintV1) -> String {
  let digits = "0123456789abcdef"
  let out = StringBuilder(size_hint=64)
  for byte in self.0 {
    out.write_char(
      digits.code_unit_at((byte.to_int() >> 4) & 15).to_int().unsafe_to_char(),
    )
    out.write_char(
      digits.code_unit_at(byte.to_int() & 15).to_int().unsafe_to_char(),
    )
  }
  out.to_string()
}

///|
/// Compute SchemaFingerprintV1. `None` denotes a Component Model unit stream
/// element. Only definitions reachable from `element` are included.
pub fn schema_fingerprint_v1(
  graph : SchemaGraph,
  element : SchemaType?,
) -> SchemaFingerprintV1 raise SchemaFingerprintError {
  SchemaFingerprintV1(blake3_256(canonical_schema_bytes_v1(graph, element)))
}

///|
/// Produce the normative deterministic-CBOR hash input for SchemaFingerprintV1.
pub fn canonical_schema_bytes_v1(
  graph : SchemaGraph,
  element : SchemaType?,
) -> FixedArray[Byte] raise SchemaFingerprintError {
  reject_duplicate_definitions(graph)
  let root = match element {
    Some(root) => root
    None => schema_type(Tuple([]))
  }
  let defs = reachable_definitions(graph, root)
  let projected = { defs, root, }
  let errors = validate_graph(projected)
  if !errors.is_empty() {
    raise InvalidSchema(errors.map(error => repr(error)).join("; "))
  }
  let encoder = CborEncoder::new()
  encoder.array(4)
  encoder.text("golem-schema-fingerprint")
  encoder.unsigned(1UL)
  match element {
    None => {
      encoder.array(2)
      encoder.unsigned(0UL)
      encode_metadata(encoder, empty_metadata())
    }
    Some(root) => encode_type(encoder, root)
  }
  encoder.array(defs.length())
  for def in defs {
    encoder.array(3)
    encoder.text(def.id)
    encoder.optional_text(def.name)
    encode_type(encoder, def.body)
  }
  encoder.finish()
}

///|
fn reject_duplicate_definitions(
  graph : SchemaGraph,
) -> Unit raise SchemaFingerprintError {
  let seen : Map[String, Unit] = Map([])
  for def in graph.defs {
    if seen.contains(def.id) {
      raise InvalidSchema("duplicate type id `\{def.id}`")
    }
    seen[def.id] = ()
  }
}

///|
fn reachable_definitions(
  graph : SchemaGraph,
  root : SchemaType,
) -> Array[SchemaTypeDef] {
  let wanted : Map[String, Unit] = Map([])
  collect_refs(root, wanted)
  let result : Array[SchemaTypeDef] = []
  let mut changed = true
  while changed {
    changed = false
    for def in graph.defs {
      if wanted.contains(def.id) && !result.any(item => item.id == def.id) {
        result.push(def)
        collect_refs(def.body, wanted)
        changed = true
      }
    }
  }
  result.sort_by((a, b) => utf8_compare(a.id, b.id))
  result
}

///|
fn collect_refs(ty : SchemaType, refs : Map[String, Unit]) -> Unit {
  match ty.body {
    Ref(id) => refs[id] = ()
    Record(fields) =>
      for field in fields {
        collect_refs(field.body, refs)
      }
    Variant(cases) =>
      for case in cases {
        match case.payload {
          Some(value) => collect_refs(value, refs)
          None => ()
        }
      }
    Tuple(types) =>
      for item in types {
        collect_refs(item, refs)
      }
    List(inner) | FixedList(inner, _) | Option(inner) =>
      collect_refs(inner, refs)
    Map(key, value) => {
      collect_refs(key, refs)
      collect_refs(value, refs)
    }
    Result(ok, err) => {
      match ok {
        Some(value) => collect_refs(value, refs)
        None => ()
      }
      match err {
        Some(value) => collect_refs(value, refs)
        None => ()
      }
    }
    Union(branches) =>
      for branch in branches {
        collect_refs(branch.body, refs)
      }
    Secret(spec) => collect_refs(spec.inner, refs)
    Future(inner) | Stream(inner) =>
      match inner {
        Some(value) => collect_refs(value, refs)
        None => ()
      }
    _ => ()
  }
}

///|
fn encode_type(
  e : CborEncoder,
  ty : SchemaType,
) -> Unit raise SchemaFingerprintError {
  match ty.body {
    Ref(id) => {
      e.array(3)
      e.unsigned(1UL)
      e.text(id)
      encode_metadata(e, ty.metadata)
    }
    Bool => encode_leaf(e, 2, ty.metadata)
    S8(r) => encode_numeric(e, 3, r, ty.metadata)
    S16(r) => encode_numeric(e, 4, r, ty.metadata)
    S32(r) => encode_numeric(e, 5, r, ty.metadata)
    S64(r) => encode_numeric(e, 6, r, ty.metadata)
    U8(r) => encode_numeric(e, 7, r, ty.metadata)
    U16(r) => encode_numeric(e, 8, r, ty.metadata)
    U32(r) => encode_numeric(e, 9, r, ty.metadata)
    U64(r) => encode_numeric(e, 10, r, ty.metadata)
    F32(r) => encode_numeric(e, 11, r, ty.metadata)
    F64(r) => encode_numeric(e, 12, r, ty.metadata)
    Char => encode_leaf(e, 13, ty.metadata)
    String => encode_leaf(e, 14, ty.metadata)
    Record(fields) => {
      e.array(3)
      e.unsigned(15UL)
      e.array(fields.length())
      for field in fields {
        e.array(3)
        e.text(field.name)
        encode_type(e, field.body)
        encode_metadata(e, field.metadata)
      }
      encode_metadata(e, ty.metadata)
    }
    Variant(cases) => {
      e.array(3)
      e.unsigned(16UL)
      e.array(cases.length())
      for case in cases {
        e.array(3)
        e.text(case.name)
        encode_optional_type(e, case.payload)
        encode_metadata(e, case.metadata)
      }
      encode_metadata(e, ty.metadata)
    }
    Enum(names) => encode_names(e, 17, names, ty.metadata)
    Flags(names) => encode_names(e, 18, names, ty.metadata)
    Tuple(items) => {
      e.array(3)
      e.unsigned(19UL)
      e.array(items.length())
      for item in items {
        encode_type(e, item)
      }
      encode_metadata(e, ty.metadata)
    }
    List(inner) => encode_unary(e, 20, inner, ty.metadata)
    FixedList(inner, length) => {
      e.array(4)
      e.unsigned(21UL)
      encode_type(e, inner)
      e.unsigned(length.to_uint64())
      encode_metadata(e, ty.metadata)
    }
    Map(key, value) => {
      e.array(4)
      e.unsigned(22UL)
      encode_type(e, key)
      encode_type(e, value)
      encode_metadata(e, ty.metadata)
    }
    Option(inner) => encode_unary(e, 23, inner, ty.metadata)
    Result(ok, err) => {
      e.array(4)
      e.unsigned(24UL)
      encode_optional_type(e, ok)
      encode_optional_type(e, err)
      encode_metadata(e, ty.metadata)
    }
    Text(r) => {
      e.array(6)
      e.unsigned(25UL)
      encode_optional_set(e, "text.languages", r.languages)
      e.optional_uint(r.min_length)
      e.optional_uint(r.max_length)
      e.optional_text(r.regex)
      encode_metadata(e, ty.metadata)
    }
    Binary(r) => {
      e.array(5)
      e.unsigned(26UL)
      encode_optional_set(e, "binary.mime_types", r.mime_types)
      e.optional_uint(r.min_bytes)
      e.optional_uint(r.max_bytes)
      encode_metadata(e, ty.metadata)
    }
    Path(spec) => {
      e.array(6)
      e.unsigned(27UL)
      e.unsigned(spec.direction.ordinal().to_uint64())
      e.unsigned(spec.kind.ordinal().to_uint64())
      encode_optional_set(e, "path.allowed_mime_types", spec.allowed_mime_types)
      encode_optional_set(e, "path.allowed_extensions", spec.allowed_extensions)
      encode_metadata(e, ty.metadata)
    }
    Url(r) => {
      e.array(4)
      e.unsigned(28UL)
      encode_optional_set(e, "url.allowed_schemes", r.allowed_schemes)
      encode_optional_set(e, "url.allowed_hosts", r.allowed_hosts)
      encode_metadata(e, ty.metadata)
    }
    Datetime => encode_leaf(e, 29, ty.metadata)
    Duration => encode_leaf(e, 30, ty.metadata)
    Quantity(spec) => {
      e.array(6)
      e.unsigned(31UL)
      e.text(spec.base_unit)
      e.array(spec.allowed_suffixes.length())
      for suffix in spec.allowed_suffixes {
        e.text(suffix)
      }
      encode_optional_quantity(e, spec.min)
      encode_optional_quantity(e, spec.max)
      encode_metadata(e, ty.metadata)
    }
    Union(branches) => {
      e.array(3)
      e.unsigned(32UL)
      e.array(branches.length())
      for branch in branches {
        e.array(4)
        e.text(branch.tag)
        encode_type(e, branch.body)
        encode_discriminator(e, branch.discriminator)
        encode_metadata(e, branch.metadata)
      }
      encode_metadata(e, ty.metadata)
    }
    Secret(spec) => {
      e.array(4)
      e.unsigned(33UL)
      encode_type(e, spec.inner)
      e.optional_text(spec.category)
      encode_metadata(e, ty.metadata)
    }
    QuotaToken(spec) => {
      e.array(3)
      e.unsigned(34UL)
      e.optional_text(spec.resource_name)
      encode_metadata(e, ty.metadata)
    }
    Future(inner) => {
      e.array(3)
      e.unsigned(35UL)
      encode_optional_type(e, inner)
      encode_metadata(e, ty.metadata)
    }
    Stream(inner) => {
      e.array(3)
      e.unsigned(36UL)
      encode_optional_type(e, inner)
      encode_metadata(e, ty.metadata)
    }
    PermissionCard(spec) => {
      e.array(3)
      e.unsigned(37UL)
      e.boolean(spec.polymorphic)
      encode_metadata(e, ty.metadata)
    }
  }
}

///|
fn encode_leaf(
  e : CborEncoder,
  tag : Int,
  metadata : @types.MetadataEnvelope,
) -> Unit raise SchemaFingerprintError {
  e.array(2)
  e.unsigned(tag.to_uint64())
  encode_metadata(e, metadata)
}

///|
fn encode_unary(
  e : CborEncoder,
  tag : Int,
  inner : SchemaType,
  metadata : @types.MetadataEnvelope,
) -> Unit raise SchemaFingerprintError {
  e.array(3)
  e.unsigned(tag.to_uint64())
  encode_type(e, inner)
  encode_metadata(e, metadata)
}

///|
fn encode_names(
  e : CborEncoder,
  tag : Int,
  names : Array[String],
  metadata : @types.MetadataEnvelope,
) -> Unit raise SchemaFingerprintError {
  e.array(3)
  e.unsigned(tag.to_uint64())
  e.array(names.length())
  for name in names {
    e.text(name)
  }
  encode_metadata(e, metadata)
}

///|
fn encode_optional_type(
  e : CborEncoder,
  ty : SchemaType?,
) -> Unit raise SchemaFingerprintError {
  match ty {
    Some(value) => encode_type(e, value)
    None => e.null()
  }
}

///|
fn encode_numeric(
  e : CborEncoder,
  tag : Int,
  r : @types.NumericRestrictions?,
  metadata : @types.MetadataEnvelope,
) -> Unit raise SchemaFingerprintError {
  e.array(3)
  e.unsigned(tag.to_uint64())
  match r {
    None => e.null()
    Some(r) if r.min is None &&
      r.max is None &&
      (r.unit is None || r.unit == Some("")) => e.null()
    Some(r) => {
      e.array(3)
      encode_bound(e, r.min)
      encode_bound(e, r.max)
      e.optional_text(
        match r.unit {
          Some("") => None
          other => other
        },
      )
    }
  }
  encode_metadata(e, metadata)
}

///|
fn encode_bound(
  e : CborEncoder,
  bound : @types.NumericBound?,
) -> Unit raise SchemaFingerprintError {
  match bound {
    None => e.null()
    Some(@types.Signed(value)) => {
      e.array(2)
      e.unsigned(0UL)
      e.signed(value)
    }
    Some(@types.Unsigned(value)) => {
      e.array(2)
      e.unsigned(1UL)
      e.unsigned(value)
    }
    Some(@types.FloatBits(bits)) => {
      let value = bits.reinterpret_as_double()
      if value.is_nan() || value.is_inf() {
        raise InvalidSchema("numeric float bound must be finite")
      }
      e.array(2)
      e.unsigned(2UL)
      e.unsigned(if value == 0.0 { 0UL } else { bits })
    }
  }
}

///|
fn encode_metadata(
  e : CborEncoder,
  metadata : @types.MetadataEnvelope,
) -> Unit raise SchemaFingerprintError {
  e.array(5)
  e.optional_text(metadata.doc)
  encode_set(e, "metadata.aliases", metadata.aliases)
  e.array(metadata.examples.length())
  for example in metadata.examples {
    e.text(example)
  }
  e.optional_text(metadata.deprecated)
  match metadata.role {
    None => e.null()
    Some(@types.Multimodal) => {
      e.array(1)
      e.unsigned(0UL)
    }
    Some(@types.UnstructuredText) => {
      e.array(1)
      e.unsigned(1UL)
    }
    Some(@types.UnstructuredBinary) => {
      e.array(1)
      e.unsigned(2UL)
    }
    Some(@types.Other(value)) => {
      e.array(2)
      e.unsigned(3UL)
      e.text(value)
    }
  }
}

///|
fn encode_optional_set(
  e : CborEncoder,
  field : String,
  values : Array[String]?,
) -> Unit raise SchemaFingerprintError {
  match values {
    None => e.null()
    Some(values) => encode_set(e, field, values)
  }
}

///|
fn encode_set(
  e : CborEncoder,
  field : String,
  values : Array[String],
) -> Unit raise SchemaFingerprintError {
  let sorted = values.copy()
  sorted.sort_by(utf8_compare)
  for i in 1..<sorted.length() {
    if sorted[i - 1] == sorted[i] {
      raise DuplicateSetValue(field~, value=sorted[i])
    }
  }
  e.array(sorted.length())
  for value in sorted {
    e.text(value)
  }
}

///|
fn utf8_compare(a : String, b : String) -> Int {
  @utf8.encode(a).lexical_compare(@utf8.encode(b))
}

///|
fn encode_optional_quantity(
  e : CborEncoder,
  value : @types.QuantityValue?,
) -> Unit {
  match value {
    None => e.null()
    Some(value) => {
      e.array(3)
      e.signed(value.mantissa)
      e.signed(value.scale.to_int64())
      e.text(value.unit)
    }
  }
}

///|
fn encode_discriminator(
  e : CborEncoder,
  value : @types.DiscriminatorRule,
) -> Unit {
  match value {
    @types.Prefix(s) => {
      e.array(2)
      e.unsigned(0UL)
      e.text(s)
    }
    @types.Suffix(s) => {
      e.array(2)
      e.unsigned(1UL)
      e.text(s)
    }
    @types.Contains(s) => {
      e.array(2)
      e.unsigned(2UL)
      e.text(s)
    }
    @types.Regex(s) => {
      e.array(2)
      e.unsigned(3UL)
      e.text(s)
    }
    @types.FieldEquals(f) => {
      e.array(3)
      e.unsigned(4UL)
      e.text(f.field_name)
      e.optional_text(f.literal)
    }
    @types.FieldAbsent(s) => {
      e.array(2)
      e.unsigned(5UL)
      e.text(s)
    }
  }
}

///|
priv struct CborEncoder {
  bytes : Array[Byte]
}

///|
fn CborEncoder::new() -> CborEncoder {
  { bytes: [], }
}

///|
fn CborEncoder::finish(self : CborEncoder) -> FixedArray[Byte] {
  FixedArray::makei(self.bytes.length(), i => self.bytes[i])
}

///|
fn CborEncoder::null(self : CborEncoder) -> Unit {
  self.bytes.push(0xf6)
}

///|
fn CborEncoder::array(self : CborEncoder, length : Int) -> Unit {
  self.major(4, length.to_uint64())
}

///|
fn CborEncoder::text(self : CborEncoder, value : String) -> Unit {
  let bytes = @utf8.encode(value)
  self.major(3, bytes.length().to_uint64())
  for byte in bytes {
    self.bytes.push(byte)
  }
}

///|
fn CborEncoder::optional_text(self : CborEncoder, value : String?) -> Unit {
  match value {
    Some(value) => self.text(value)
    None => self.null()
  }
}

///|
fn CborEncoder::optional_uint(self : CborEncoder, value : UInt?) -> Unit {
  match value {
    Some(value) => self.unsigned(value.to_uint64())
    None => self.null()
  }
}

///|
fn CborEncoder::boolean(self : CborEncoder, value : Bool) -> Unit {
  self.bytes.push(if value { 0xf5 } else { 0xf4 })
}

///|
fn CborEncoder::unsigned(self : CborEncoder, value : UInt64) -> Unit {
  self.major(0, value)
}

///|
fn CborEncoder::signed(self : CborEncoder, value : Int64) -> Unit {
  if value >= 0L {
    self.unsigned(value.reinterpret_as_uint64())
  } else {
    self.major(1, (-1L - value).reinterpret_as_uint64())
  }
}

///|
fn CborEncoder::major(self : CborEncoder, major : Int, value : UInt64) -> Unit {
  let prefix = (major << 5).to_byte()
  if value <= 23UL {
    self.bytes.push(prefix | value.to_byte())
  } else if value <= 0xffUL {
    self.bytes.push(prefix | 24)
    self.bytes.push(value.to_byte())
  } else if value <= 0xffffUL {
    self.bytes.push(prefix | 25)
    self.push_be(value, 2)
  } else if value <= 0xffffffffUL {
    self.bytes.push(prefix | 26)
    self.push_be(value, 4)
  } else {
    self.bytes.push(prefix | 27)
    self.push_be(value, 8)
  }
}

///|
fn CborEncoder::push_be(
  self : CborEncoder,
  value : UInt64,
  count : Int,
) -> Unit {
  for i in 0..<count {
    self.bytes.push((value >> ((count - i - 1) * 8)).to_byte())
  }
}

// BLAKE3, including chunk-tree reduction. Kept local to avoid a native-only
// crypto dependency in this WASM SDK package.

///|
let blake3_iv : FixedArray[UInt] = [
  0x6A09E667, 0xBB67AE85, 0x3C6EF372, 0xA54FF53A, 0x510E527F, 0x9B05688C, 0x1F83D9AB,
  0x5BE0CD19,
]

///|
let message_permutation : FixedArray[Int] = [
  2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8,
]

///|
priv struct BlakeOutput {
  cv : FixedArray[UInt]
  block : FixedArray[UInt]
  counter : UInt64
  block_len : Int
  flags : UInt
}

///|
fn rotr(x : UInt, n : Int) -> UInt {
  (x >> n) | (x << (32 - n))
}

///|
fn g(
  v : FixedArray[UInt],
  a : Int,
  b : Int,
  c : Int,
  d : Int,
  x : UInt,
  y : UInt,
) -> Unit {
  v[a] = v[a] + v[b] + x
  v[d] = rotr(v[d] ^ v[a], 16)
  v[c] = v[c] + v[d]
  v[b] = rotr(v[b] ^ v[c], 12)
  v[a] = v[a] + v[b] + y
  v[d] = rotr(v[d] ^ v[a], 8)
  v[c] = v[c] + v[d]
  v[b] = rotr(v[b] ^ v[c], 7)
}

///|
fn compress(output : BlakeOutput, root : Bool) -> FixedArray[UInt] {
  let v = FixedArray::make(16, 0U)
  for i in 0..<8 {
    v[i] = output.cv[i]
  }
  for i in 0..<4 {
    v[i + 8] = blake3_iv[i]
  }
  v[12] = output.counter.to_uint()
  v[13] = (output.counter >> 32).to_uint()
  v[14] = output.block_len.reinterpret_as_uint()
  v[15] = output.flags | (if root { 8U } else { 0U })
  let schedule = FixedArray::makei(16, i => i)
  for _ in 0..<7 {
    g(v, 0, 4, 8, 12, output.block[schedule[0]], output.block[schedule[1]])
    g(v, 1, 5, 9, 13, output.block[schedule[2]], output.block[schedule[3]])
    g(v, 2, 6, 10, 14, output.block[schedule[4]], output.block[schedule[5]])
    g(v, 3, 7, 11, 15, output.block[schedule[6]], output.block[schedule[7]])
    g(v, 0, 5, 10, 15, output.block[schedule[8]], output.block[schedule[9]])
    g(v, 1, 6, 11, 12, output.block[schedule[10]], output.block[schedule[11]])
    g(v, 2, 7, 8, 13, output.block[schedule[12]], output.block[schedule[13]])
    g(v, 3, 4, 9, 14, output.block[schedule[14]], output.block[schedule[15]])
    let old = schedule.copy()
    for i in 0..<16 {
      schedule[i] = old[message_permutation[i]]
    }
  }
  let result = FixedArray::make(16, 0U)
  for i in 0..<8 {
    result[i] = v[i] ^ v[i + 8]
    result[i + 8] = v[i + 8] ^ output.cv[i]
  }
  result
}

///|
fn output_cv(output : BlakeOutput) -> FixedArray[UInt] {
  let full = compress(output, false)
  FixedArray::makei(8, i => full[i])
}

///|
fn words(
  bytes : FixedArray[Byte],
  start : Int,
  length : Int,
) -> FixedArray[UInt] {
  let result = FixedArray::make(16, 0U)
  for i in 0..<length {
    result[i / 4] = result[i / 4] | (bytes[start + i].to_uint() << (i % 4 * 8))
  }
  result
}

///|
fn chunk_output(
  bytes : FixedArray[Byte],
  start : Int,
  length : Int,
  counter : UInt64,
) -> BlakeOutput {
  let mut cv = blake3_iv.copy()
  let blocks = if length == 0 { 1 } else { (length + 63) / 64 }
  let mut out = {
    cv,
    block: FixedArray::make(16, 0U),
    counter,
    block_len: 0,
    flags: 0U,
  }
  for block in 0..<blocks {
    let offset = block * 64
    let n = Int::min(64, length - offset)
    out = {
      cv,
      block: words(bytes, start + offset, n),
      counter,
      block_len: n,
      flags: (if block == 0 { 1U } else { 0U }) |
      (if block == blocks - 1 { 2U } else { 0U }),
    }
    if block < blocks - 1 {
      cv = output_cv(out)
    }
  }
  out
}

///|
fn parent_output(
  left : FixedArray[UInt],
  right : FixedArray[UInt],
) -> BlakeOutput {
  let block = FixedArray::make(16, 0U)
  for i in 0..<8 {
    block[i] = left[i]
    block[i + 8] = right[i]
  }
  { cv: blake3_iv.copy(), block, counter: 0UL, block_len: 64, flags: 4U, }
}

///|
fn blake3_256(bytes : FixedArray[Byte]) -> FixedArray[Byte] {
  let chunks = Int::max(1, (bytes.length() + 1023) / 1024)
  let stack : Array[FixedArray[UInt]] = []
  let mut last = chunk_output(bytes, 0, Int::min(1024, bytes.length()), 0UL)
  for chunk in 0..<chunks {
    if chunk > 0 {
      last = chunk_output(
        bytes,
        chunk * 1024,
        Int::min(1024, bytes.length() - chunk * 1024),
        chunk.to_uint64(),
      )
    }
    if chunk < chunks - 1 {
      let mut cv = output_cv(last)
      let mut total = chunk + 1
      while (total & 1) == 0 {
        cv = output_cv(parent_output(stack.pop().unwrap(), cv))
        total = total >> 1
      }
      stack.push(cv)
    }
  }
  while !stack.is_empty() {
    last = parent_output(stack.pop().unwrap(), output_cv(last))
  }
  let root = compress(last, true)
  let result : FixedArray[Byte] = FixedArray::make(32, 0)
  for i in 0..<8 {
    for j in 0..<4 {
      result[i * 4 + j] = (root[i] >> (j * 8)).to_byte()
    }
  }
  result
}
