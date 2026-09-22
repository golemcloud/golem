///|
/// A graph-backed view of one reflected schema root.
pub struct SchemaRef {
  graph : @model.SchemaGraph
  root : @model.SchemaType
} derive(Debug, Eq)

///|
fn SchemaRef::new(graph : @model.SchemaGraph) -> SchemaRef {
  { root: graph.root, graph, }
}

///|
fn clone_metadata(
  metadata : @types.MetadataEnvelope,
) -> @types.MetadataEnvelope {
  {
    ..metadata,
    aliases: metadata.aliases.copy(),
    examples: metadata.examples.copy(),
  }
}

///|
fn clone_schema_type(schema : @model.SchemaType) -> @model.SchemaType {
  let body = match schema.body {
    Record(fields) =>
      @model.SchemaTypeBody::Record(
        fields.map(field => {
          ..field,
          body: clone_schema_type(field.body),
          metadata: clone_metadata(field.metadata),
        }),
      )
    Variant(cases) =>
      @model.SchemaTypeBody::Variant(
        cases.map(case => {
          ..case,
          payload: case.payload.map(clone_schema_type),
          metadata: clone_metadata(case.metadata),
        }),
      )
    Enum(cases) => @model.SchemaTypeBody::Enum(cases.copy())
    Flags(names) => @model.SchemaTypeBody::Flags(names.copy())
    Tuple(elements) =>
      @model.SchemaTypeBody::Tuple(elements.map(clone_schema_type))
    List(element) => @model.SchemaTypeBody::List(clone_schema_type(element))
    FixedList(element, length) =>
      @model.SchemaTypeBody::FixedList(clone_schema_type(element), length)
    Map(key, value) =>
      @model.SchemaTypeBody::Map(
        clone_schema_type(key),
        clone_schema_type(value),
      )
    Option(element) => @model.SchemaTypeBody::Option(clone_schema_type(element))
    Result(ok, err) =>
      @model.SchemaTypeBody::Result(
        ok.map(clone_schema_type),
        err.map(clone_schema_type),
      )
    Text(restrictions) =>
      @model.SchemaTypeBody::Text({
        ..restrictions,
        languages: restrictions.languages.map(values => values.copy()),
      })
    Binary(restrictions) =>
      @model.SchemaTypeBody::Binary({
        ..restrictions,
        mime_types: restrictions.mime_types.map(values => values.copy()),
      })
    Path(spec) =>
      @model.SchemaTypeBody::Path({
        ..spec,
        allowed_mime_types: spec.allowed_mime_types.map(values => values.copy()),
        allowed_extensions: spec.allowed_extensions.map(values => values.copy()),
      })
    Url(restrictions) =>
      @model.SchemaTypeBody::Url({
        allowed_schemes: restrictions.allowed_schemes.map(values => {
          values.copy()
        }),
        allowed_hosts: restrictions.allowed_hosts.map(values => values.copy()),
      })
    Quantity(spec) =>
      @model.SchemaTypeBody::Quantity({
        ..spec,
        allowed_suffixes: spec.allowed_suffixes.copy(),
      })
    Union(branches) =>
      @model.SchemaTypeBody::Union(
        branches.map(branch => {
          ..branch,
          body: clone_schema_type(branch.body),
          metadata: clone_metadata(branch.metadata),
        }),
      )
    Secret(spec) =>
      @model.SchemaTypeBody::Secret({
        ..spec,
        inner: clone_schema_type(spec.inner),
      })
    Future(element) =>
      @model.SchemaTypeBody::Future(element.map(clone_schema_type))
    Stream(element) =>
      @model.SchemaTypeBody::Stream(element.map(clone_schema_type))
    other => other
  }
  { body, metadata: clone_metadata(schema.metadata), }
}

///|
fn clone_schema_graph(graph : @model.SchemaGraph) -> @model.SchemaGraph {
  {
    defs: graph.defs.map(def => { ..def, body: clone_schema_type(def.body), }),
    root: clone_schema_type(graph.root),
  }
}

///|
fn SchemaRef::owned_graph(self : SchemaRef) -> @model.SchemaGraph {
  let closed = @model.schema_graph_closure({
    defs: self.graph.defs,
    root: self.root,
  })
  clone_schema_graph(closed)
}

///|
/// Return a self-contained defensive copy of this schema graph.
pub fn SchemaRef::graph(self : SchemaRef) -> @model.SchemaGraph {
  self.owned_graph()
}

///|
/// Return a defensive copy of this schema root.
pub fn SchemaRef::root(self : SchemaRef) -> @model.SchemaType {
  self.owned_graph().root
}

///|
/// Validate a value and pair it with a self-contained defensive graph copy for
/// fully dynamic invocation.
pub fn SchemaRef::typed_value(
  self : SchemaRef,
  value : @model.SchemaValue,
) -> @model.TypedSchemaValue raise ReflectError {
  self.validate_value(value)
  { graph: self.owned_graph(), value, }
}

///|
pub fn SchemaRef::validate_value(
  self : SchemaRef,
  value : @model.SchemaValue,
) -> Unit raise ReflectError {
  guard @tool_core.schema_value_matches_graph(self.graph, value) else {
    raise Schema("schema-native value does not match reflected schema")
  }
  self.validate_refinements(self.root, value)
}

///|
fn numeric_bound_value(bound : @types.NumericBound) -> Double {
  match bound {
    Signed(value) => value.to_double()
    Unsigned(value) => value.to_double()
    FloatBits(bits) => bits.reinterpret_as_double()
  }
}

///|
fn check_numeric_refinement(
  restrictions : @types.NumericRestrictions?,
  value : Double,
) -> Unit raise ReflectError {
  match restrictions {
    Some(restrictions) => {
      match restrictions.min {
        Some(bound) =>
          if value < numeric_bound_value(bound) {
            raise Schema("numeric value is below declared minimum")
          }
        None => ()
      }
      match restrictions.max {
        Some(bound) =>
          if value > numeric_bound_value(bound) {
            raise Schema("numeric value is above declared maximum")
          }
        None => ()
      }
    }
    None => ()
  }
}

///|
fn signed_below_bound(value : Int64, bound : @types.NumericBound) -> Bool {
  match bound {
    Signed(minimum) => value < minimum
    Unsigned(minimum) => value < 0L || value.reinterpret_as_uint64() < minimum
    FloatBits(bits) => value.to_double() < bits.reinterpret_as_double()
  }
}

///|
fn signed_above_bound(value : Int64, bound : @types.NumericBound) -> Bool {
  match bound {
    Signed(maximum) => value > maximum
    Unsigned(maximum) => value >= 0L && value.reinterpret_as_uint64() > maximum
    FloatBits(bits) => value.to_double() > bits.reinterpret_as_double()
  }
}

///|
fn unsigned_below_bound(value : UInt64, bound : @types.NumericBound) -> Bool {
  match bound {
    Signed(minimum) => minimum >= 0L && value < minimum.reinterpret_as_uint64()
    Unsigned(minimum) => value < minimum
    FloatBits(bits) => value.to_double() < bits.reinterpret_as_double()
  }
}

///|
fn unsigned_above_bound(value : UInt64, bound : @types.NumericBound) -> Bool {
  match bound {
    Signed(maximum) => maximum < 0L || value > maximum.reinterpret_as_uint64()
    Unsigned(maximum) => value > maximum
    FloatBits(bits) => value.to_double() > bits.reinterpret_as_double()
  }
}

///|
fn check_signed_refinement(
  restrictions : @types.NumericRestrictions?,
  value : Int64,
) -> Unit raise ReflectError {
  match restrictions {
    Some(restrictions) => {
      match restrictions.min {
        Some(bound) =>
          if signed_below_bound(value, bound) {
            raise Schema("numeric value is below declared minimum")
          }
        None => ()
      }
      match restrictions.max {
        Some(bound) =>
          if signed_above_bound(value, bound) {
            raise Schema("numeric value is above declared maximum")
          }
        None => ()
      }
    }
    None => ()
  }
}

///|
fn check_unsigned_refinement(
  restrictions : @types.NumericRestrictions?,
  value : UInt64,
) -> Unit raise ReflectError {
  match restrictions {
    Some(restrictions) => {
      match restrictions.min {
        Some(bound) =>
          if unsigned_below_bound(value, bound) {
            raise Schema("numeric value is below declared minimum")
          }
        None => ()
      }
      match restrictions.max {
        Some(bound) =>
          if unsigned_above_bound(value, bound) {
            raise Schema("numeric value is above declared maximum")
          }
        None => ()
      }
    }
    None => ()
  }
}

///|
fn SchemaRef::validate_refinements(
  self : SchemaRef,
  schema : @model.SchemaType,
  value : @model.SchemaValue,
) -> Unit raise ReflectError {
  match (schema.body, value) {
    (Ref(_), _) => self.validate_refinements(self.resolve(schema), value)
    (S8(r), S8(v)) | (S16(r), S16(v)) | (S32(r), S32(v)) =>
      check_numeric_refinement(r, v.to_double())
    (S64(r), S64(v)) => check_signed_refinement(r, v)
    (U8(r), U8(v)) => check_numeric_refinement(r, v.to_int().to_double())
    (U16(r), U16(v)) =>
      check_numeric_refinement(r, v.reinterpret_as_int().to_double())
    (U32(r), U32(v)) => check_numeric_refinement(r, v.to_double())
    (U64(r), U64(v)) => check_unsigned_refinement(r, v)
    (F32(r), F32(v)) => check_numeric_refinement(r, v.to_double())
    (F64(r), F64(v)) => check_numeric_refinement(r, v)
    (Record(fields), Record(values)) =>
      for index in 0..<fields.length() {
        self.validate_refinements(fields[index].body, values[index])
      }
    (Tuple(types), Tuple(values)) =>
      for index in 0..<types.length() {
        self.validate_refinements(types[index], values[index])
      }
    (List(element), List(values))
    | (FixedList(element, _), FixedList(values)) =>
      for entry in values {
        self.validate_refinements(element, entry)
      }
    (Map(key, entry), Map(values)) =>
      for item in values {
        self.validate_refinements(key, item.key)
        self.validate_refinements(entry, item.value)
      }
    (Option(element), Option(Some(inner))) =>
      self.validate_refinements(element, inner)
    (Variant(cases), Variant(index, Some(payload))) =>
      match cases[index.reinterpret_as_int()].payload {
        Some(inner) => self.validate_refinements(inner, payload)
        None => ()
      }
    (Result(ok, _), ResultOk(Some(payload))) =>
      match ok {
        Some(inner) => self.validate_refinements(inner, payload)
        None => ()
      }
    (Result(_, err), ResultErr(Some(payload))) =>
      match err {
        Some(inner) => self.validate_refinements(inner, payload)
        None => ()
      }
    (Union(branches), Union(tag, body)) =>
      match branches.iter().find_first(branch => branch.tag == tag) {
        Some(branch) => self.validate_refinements(branch.body, body)
        None => ()
      }
    (Text(r), Text(text, language)) => {
      match r.min_length {
        Some(min) =>
          if text.char_length().reinterpret_as_uint() < min {
            raise Schema("text is shorter than declared minimum")
          }
        None => ()
      }
      match r.max_length {
        Some(max) =>
          if text.char_length().reinterpret_as_uint() > max {
            raise Schema("text is longer than declared maximum")
          }
        None => ()
      }
      match r.languages {
        Some(allowed) =>
          if !(match language {
              Some(lang) => allowed.contains(lang)
              None => false
            }) {
            raise Schema("text language is not allowed")
          }
        None => ()
      }
      match r.regex {
        Some(pattern) => {
          let regex = @string.Regex::Regex(pattern) catch {
            error => raise Schema("invalid text regex: \{repr(error)}")
          }
          if regex.execute(text) is None {
            raise Schema("text does not match declared regex")
          }
        }
        None => ()
      }
    }
    (Binary(r), Binary(bytes, mime_type)) => {
      match r.min_bytes {
        Some(min) =>
          if bytes.length().reinterpret_as_uint() < min {
            raise Schema("binary value is shorter than declared minimum")
          }
        None => ()
      }
      match r.max_bytes {
        Some(max) =>
          if bytes.length().reinterpret_as_uint() > max {
            raise Schema("binary value is longer than declared maximum")
          }
        None => ()
      }
      match r.mime_types {
        Some(allowed) =>
          if !(match mime_type {
              Some(mime) => allowed.contains(mime)
              None => false
            }) {
            raise Schema("binary MIME type is not allowed")
          }
        None => ()
      }
    }
    (Path(spec), Path(path)) => {
      guard path != "" else { raise Schema("path is empty") }
      match spec.allowed_extensions {
        Some(allowed) =>
          match re"\.([^./]+)$".execute(path) {
            Some(m) =>
              if !allowed.contains(m.group(1).unwrap().to_owned()) {
                raise Schema("path extension is not allowed")
              }
            None => ()
          }
        None => ()
      }
    }
    (Url(restrictions), Url(url)) => {
      let parsed = re"^([A-Za-z][A-Za-z0-9+.\-]*):".execute(url)
      guard parsed is Some(parts) else { raise Schema("URL is invalid") }
      match restrictions.allowed_schemes {
        Some(allowed) =>
          if !allowed.any(scheme => {
              scheme.equal_ignore_ascii_case(parts.group(1).unwrap().to_owned())
            }) {
            raise Schema("URL scheme is not allowed")
          }
        None => ()
      }
      match restrictions.allowed_hosts {
        Some(allowed) =>
          match
            re"^[A-Za-z][A-Za-z0-9+.\-]*://(?:[^@/]*@)?([^:/?#]+)".execute(url) {
            Some(authority) => {
              let host = authority.group(1).unwrap().to_owned()
              if !allowed.any(item => item.equal_ignore_ascii_case(host)) {
                raise Schema("URL host is not allowed")
              }
            }
            None => raise Schema("URL host is missing")
          }
        None => ()
      }
    }
    (Quantity(spec), Quantity(quantity)) => {
      let unit_allowed = if spec.allowed_suffixes.length() == 0 {
        quantity.unit == spec.base_unit
      } else {
        spec.allowed_suffixes.contains(quantity.unit)
      }
      guard unit_allowed else { raise Schema("quantity unit is not allowed") }
      match spec.min {
        Some(min) =>
          if quantity_below(quantity, min) {
            raise Schema("quantity is below declared minimum")
          }
        None => ()
      }
      match spec.max {
        Some(max) =>
          if quantity_below(max, quantity) {
            raise Schema("quantity is above declared maximum")
          }
        None => ()
      }
    }
    _ => ()
  }
}

///|
fn quantity_below(
  left : @types.QuantityValue,
  right : @types.QuantityValue,
) -> Bool {
  if left.scale == right.scale {
    left.mantissa < right.mantissa
  } else {
    let left_value = left.mantissa.to_double() *
      @math.pow(10.0, -left.scale.to_double())
    let right_value = right.mantissa.to_double() *
      @math.pow(10.0, -right.scale.to_double())
    left_value < right_value
  }
}

///|
/// Reports whether this schema root contains a live stream, following named
/// references without looping on recursive definitions.
pub fn SchemaRef::contains_stream(self : SchemaRef) -> Bool {
  self.type_contains_stream(self.root, Map([]))
}

///|
fn SchemaRef::type_contains_stream(
  self : SchemaRef,
  schema : @model.SchemaType,
  visited : Map[String, Unit],
) -> Bool {
  match schema.body {
    Stream(_) => true
    Ref(id) =>
      if visited.contains(id) {
        false
      } else {
        visited[id] = ()
        match self.graph.defs.iter().find_first(def => def.id == id) {
          Some(def) => self.type_contains_stream(def.body, visited)
          None => false
        }
      }
    Record(fields) =>
      fields.any(field => self.type_contains_stream(field.body, visited))
    Variant(cases) =>
      cases.any(case => {
        match case.payload {
          Some(payload) => self.type_contains_stream(payload, visited)
          None => false
        }
      })
    Tuple(elements) =>
      elements.any(element => self.type_contains_stream(element, visited))
    List(element)
    | FixedList(element, _)
    | Option(element)
    | Secret({ inner: element, .. }) =>
      self.type_contains_stream(element, visited)
    Map(key, value) =>
      self.type_contains_stream(key, visited) ||
      self.type_contains_stream(value, visited)
    Result(ok, err) => {
      let ok_contains = match ok {
        Some(element) => self.type_contains_stream(element, visited)
        None => false
      }
      let err_contains = match err {
        Some(element) => self.type_contains_stream(element, visited)
        None => false
      }
      ok_contains || err_contains
    }
    Union(branches) =>
      branches.any(branch => self.type_contains_stream(branch.body, visited))
    Future(inner) =>
      match inner {
        Some(element) => self.type_contains_stream(element, visited)
        None => false
      }
    _ => false
  }
}

///|
fn SchemaRef::resolve(
  self : SchemaRef,
  schema : @model.SchemaType,
) -> @model.SchemaType raise ReflectError {
  match schema.body {
    Ref(id) =>
      match self.graph.defs.iter().find_first(def => def.id == id) {
        Some(def) => self.resolve(def.body)
        None => raise Schema("dangling schema reference '" + id + "'")
      }
    _ => schema
  }
}

///|
fn json_object(value : Json) -> Map[String, Json] raise ReflectError {
  match value {
    Object(fields) => fields
    _ => raise Json("expected a JSON object")
  }
}

///|
fn json_array(value : Json) -> Array[Json] raise ReflectError {
  match value {
    Array(values) => values
    _ => raise Json("expected a JSON array")
  }
}

///|
fn json_string(value : Json) -> String raise ReflectError {
  match value {
    String(value) => value
    _ => raise Json("expected a JSON string")
  }
}

///|
fn json_number(value : Json) -> Double raise ReflectError {
  match value {
    Number(value, ..) => value
    _ => raise Json("expected a JSON number")
  }
}

///|
fn json_integer(
  value : Json,
  minimum : Double,
  maximum : Double,
) -> Double raise ReflectError {
  let value = json_number(value)
  guard !value.is_nan() && !value.is_inf() && value == value.trunc() else {
    raise Json("expected a finite integer")
  }
  guard value >= minimum && value <= maximum else {
    raise Json("integer is outside the supported range")
  }
  value
}

///|
fn required_field(
  fields : Map[String, Json],
  name : String,
) -> Json raise ReflectError {
  match fields.get(name) {
    Some(value) => value
    None => raise Json("missing field '" + name + "'")
  }
}

///|
fn binary_to_json(bytes : FixedArray[Byte]) -> String {
  @base64.encode(Bytes::from_array(bytes).view(), padding=false)
  .replace_all(old="+", new="-")
  .replace_all(old="/", new="_")
}

///|
fn binary_from_json(encoded : String) -> FixedArray[Byte] raise ReflectError {
  guard !encoded.contains("=") else {
    raise Json("binary bytes must be unpadded base64url")
  }
  let standard = encoded
    .replace_all(old="-", new="+")
    .replace_all(old="_", new="/")
  let bytes = @base64.decode(standard) catch {
    _ => raise Json("invalid base64url binary bytes")
  }
  let result = bytes.to_fixedarray()
  guard binary_to_json(result) == encoded else {
    raise Json("invalid canonical base64url binary bytes")
  }
  result
}

///|
fn find_string_index(values : Array[String], expected : String) -> Int? {
  for index in 0..<values.length() {
    if values[index] == expected {
      return Some(index)
    }
  }
  None
}

///|
fn find_variant_index(
  values : Array[@model.VariantCaseType],
  expected : String,
) -> Int? {
  for index in 0..<values.length() {
    if values[index].name == expected {
      return Some(index)
    }
  }
  None
}

///|
fn discriminator_matches(
  rule : @types.DiscriminatorRule,
  json : Json,
) -> Bool raise ReflectError {
  match rule {
    Prefix(prefix) =>
      match json {
        String(value) => value.has_prefix(prefix)
        _ => false
      }
    Suffix(suffix) =>
      match json {
        String(value) => value.has_suffix(suffix)
        _ => false
      }
    Contains(part) =>
      match json {
        String(value) => value.contains(part)
        _ => false
      }
    Regex(pattern) =>
      match json {
        String(value) => {
          let regex = @string.Regex::Regex(pattern) catch {
            error =>
              raise Json("invalid union discriminator regex: \{repr(error)}")
          }
          regex.execute(value) is Some(_)
        }
        _ => false
      }
    FieldEquals(expected) =>
      match json {
        Object(fields) =>
          match fields.get(expected.field_name) {
            Some(String(value)) =>
              match expected.literal {
                Some(literal) => value == literal
                None => true
              }
            Some(_) => expected.literal is None
            None => false
          }
        _ => false
      }
    FieldAbsent(name) =>
      match json {
        Object(fields) => !fields.contains(name)
        _ => false
      }
  }
}

///|
pub fn SchemaRef::pack_json(
  self : SchemaRef,
  value : Json,
) -> @model.SchemaValue raise ReflectError {
  let packed = self.pack(self.resolve(self.root), value)
  self.validate_value(packed)
  packed
}

///|
pub fn SchemaRef::unpack_json(
  self : SchemaRef,
  value : @model.SchemaValue,
) -> Json raise ReflectError {
  self.validate_value(value)
  self.unpack(self.resolve(self.root), value)
}

///|
fn SchemaRef::unpack(
  self : SchemaRef,
  schema : @model.SchemaType,
  value : @model.SchemaValue,
) -> Json raise ReflectError {
  match (schema.body, value) {
    (@model.SchemaTypeBody::Bool, @model.SchemaValue::Bool(value)) =>
      Json::boolean(value)
    (@model.SchemaTypeBody::S8(_), @model.SchemaValue::S8(value))
    | (@model.SchemaTypeBody::S16(_), @model.SchemaValue::S16(value))
    | (@model.SchemaTypeBody::S32(_), @model.SchemaValue::S32(value)) =>
      Json::number(value.to_double())
    (@model.SchemaTypeBody::S64(_), @model.SchemaValue::S64(value)) => {
      guard value >= -9007199254740991L && value <= 9007199254740991L else {
        raise Json("64-bit integer is outside the safe JSON range")
      }
      Json::number(value.to_double())
    }
    (@model.SchemaTypeBody::U8(_), @model.SchemaValue::U8(value)) =>
      Json::number(value.to_int().to_double())
    (@model.SchemaTypeBody::U16(_), @model.SchemaValue::U16(value)) =>
      Json::number(value.reinterpret_as_int().to_double())
    (@model.SchemaTypeBody::U32(_), @model.SchemaValue::U32(value)) =>
      Json::number(value.to_double())
    (@model.SchemaTypeBody::U64(_), @model.SchemaValue::U64(value)) => {
      guard value <= 9007199254740991UL else {
        raise Json("64-bit integer is outside the safe JSON range")
      }
      Json::number(value.to_double())
    }
    (@model.SchemaTypeBody::F32(_), @model.SchemaValue::F32(value)) => {
      guard !value.is_nan() && !value.is_inf() else {
        raise Json("expected a finite JSON number")
      }
      Json::number(value.to_double())
    }
    (@model.SchemaTypeBody::F64(_), @model.SchemaValue::F64(value)) => {
      guard !value.is_nan() && !value.is_inf() else {
        raise Json("expected a finite JSON number")
      }
      Json::number(value)
    }
    (@model.SchemaTypeBody::Char, @model.SchemaValue::Char(value)) =>
      Json::string(value.to_string())
    (@model.SchemaTypeBody::String, @model.SchemaValue::String(value)) =>
      Json::string(value)
    (@model.SchemaTypeBody::Record(expected), @model.SchemaValue::Record(values)
    ) => {
      guard values.length() == expected.length() else {
        raise Schema("record value length does not match schema")
      }
      let fields : Map[String, Json] = Map([])
      for index in 0..<expected.length() {
        fields[expected[index].name] = self.unpack(
          self.resolve(expected[index].body),
          values[index],
        )
      }
      Json::object(fields)
    }
    (@model.SchemaTypeBody::Tuple(expected), @model.SchemaValue::Tuple(values)) => {
      guard values.length() == expected.length() else {
        raise Schema("tuple value length does not match schema")
      }
      let result : Array[Json] = []
      for index in 0..<expected.length() {
        result.push(self.unpack(self.resolve(expected[index]), values[index]))
      }
      Json::array(result)
    }
    (@model.SchemaTypeBody::List(element), @model.SchemaValue::List(values))
    | (
      @model.SchemaTypeBody::FixedList(element, _),
      @model.SchemaValue::FixedList(values),
    ) => {
      let result : Array[Json] = []
      for entry in values {
        result.push(self.unpack(self.resolve(element), entry))
      }
      Json::array(result)
    }
    (@model.SchemaTypeBody::Option(_), @model.SchemaValue::Option(None)) =>
      Json::null()
    (
      @model.SchemaTypeBody::Option(element),
      @model.SchemaValue::Option(Some(value)),
    ) => self.unpack(self.resolve(element), value)
    (@model.SchemaTypeBody::Enum(cases), @model.SchemaValue::Enum(index)) => {
      let index = index.reinterpret_as_int()
      guard index >= 0 && index < cases.length() else {
        raise Schema("enum case index is outside the schema")
      }
      Json::string(cases[index])
    }
    (@model.SchemaTypeBody::Flags(names), @model.SchemaValue::Flags(enabled)) => {
      guard names.length() == enabled.length() else {
        raise Schema("flags value length does not match schema")
      }
      let result : Array[Json] = []
      for index in 0..<names.length() {
        if enabled[index] {
          result.push(Json::string(names[index]))
        }
      }
      Json::array(result)
    }
    (
      @model.SchemaTypeBody::Variant(cases),
      @model.SchemaValue::Variant(index, payload),
    ) => {
      let index = index.reinterpret_as_int()
      guard index >= 0 && index < cases.length() else {
        raise Schema("variant case index is outside the schema")
      }
      let entry = cases[index]
      match (entry.payload, payload) {
        (None, None) => Json::string(entry.name)
        (Some(schema), Some(value)) => {
          let fields : Map[String, Json] = Map([])
          fields[entry.name] = self.unpack(self.resolve(schema), value)
          Json::object(fields)
        }
        _ => raise Schema("variant payload does not match schema")
      }
    }
    (@model.SchemaTypeBody::Map(key, entry), @model.SchemaValue::Map(values)) => {
      let result : Array[Json] = []
      for value in values {
        result.push(
          Json::array([
            self.unpack(self.resolve(key), value.key),
            self.unpack(self.resolve(entry), value.value),
          ]),
        )
      }
      Json::array(result)
    }
    (@model.SchemaTypeBody::Result(ok, _), @model.SchemaValue::ResultOk(payload)
    ) =>
      Json::object({
        "ok": (
          match (ok, payload) {
            (None, None) => Json::null()
            (Some(schema), Some(value)) =>
              self.unpack(self.resolve(schema), value)
            _ => raise Schema("result ok payload does not match schema")
          } : Json),
      })
    (
      @model.SchemaTypeBody::Result(_, err),
      @model.SchemaValue::ResultErr(payload),
    ) =>
      Json::object({
        "err": (
          match (err, payload) {
            (None, None) => Json::null()
            (Some(schema), Some(value)) =>
              self.unpack(self.resolve(schema), value)
            _ => raise Schema("result err payload does not match schema")
          } : Json),
      })
    (@model.SchemaTypeBody::Text(_), @model.SchemaValue::Text(text, language)) => {
      let fields : Map[String, Json] = { "text": Json::string(text) }
      match language {
        Some(language) => fields["language"] = Json::string(language)
        None => ()
      }
      Json::object(fields)
    }
    (@model.SchemaTypeBody::Path(_), @model.SchemaValue::Path(value))
    | (@model.SchemaTypeBody::Url(_), @model.SchemaValue::Url(value)) =>
      Json::string(value)
    (
      @model.SchemaTypeBody::Union(branches),
      @model.SchemaValue::Union(tag, value),
    ) =>
      match branches.iter().find_first(branch => branch.tag == tag) {
        Some(branch) => self.unpack(self.resolve(branch.body), value)
        None => raise Schema("union tag '" + tag + "' is outside the schema")
      }
    (@model.SchemaTypeBody::Binary(_), @model.SchemaValue::Binary(bytes, mime)) => {
      let fields : Map[String, Json] = {
        "bytes": Json::string(binary_to_json(bytes)),
      }
      match mime {
        Some(mime) => fields["mimeType"] = Json::string(mime)
        None => ()
      }
      Json::object(fields)
    }
    (@model.SchemaTypeBody::Quantity(_), @model.SchemaValue::Quantity(value)) => {
      guard value.mantissa >= -9007199254740991L &&
        value.mantissa <= 9007199254740991L else {
        raise Json("quantity mantissa is outside the safe JSON range")
      }
      Json::object({
        "mantissa": Json::number(value.mantissa.to_double()),
        "scale": Json::number(value.scale.to_double()),
        "unit": Json::string(value.unit),
      })
    }
    (@model.SchemaTypeBody::Datetime, @model.SchemaValue::Datetime(value)) =>
      Json::string(datetime_to_json(value))
    (@model.SchemaTypeBody::Duration, @model.SchemaValue::Duration(value)) =>
      Json::string(duration_to_json(value))
    (@model.SchemaTypeBody::Secret(_), @model.SchemaValue::Secret(_))
    | (@model.SchemaTypeBody::QuotaToken(_), @model.SchemaValue::QuotaToken(_))
    | (
      @model.SchemaTypeBody::PermissionCard(_),
      @model.SchemaValue::PermissionCard(_),
    ) => raise Json("capability values have no JSON representation")
    (@model.SchemaTypeBody::Future(_), _)
    | (@model.SchemaTypeBody::Stream(_), _) =>
      raise Json("future and stream values have no JSON representation")
    (@model.SchemaTypeBody::Ref(_), _) =>
      raise Schema("unresolved schema reference")
    _ => raise Schema("schema value does not match the reflected schema")
  }
}

///|
fn SchemaRef::pack(
  self : SchemaRef,
  schema : @model.SchemaType,
  json : Json,
) -> @model.SchemaValue raise ReflectError {
  match schema.body {
    Bool =>
      match json {
        True => Bool(true)
        False => Bool(false)
        _ => raise Json("expected a JSON boolean")
      }
    S8(_) => S8(json_integer(json, -128.0, 127.0).to_int())
    S16(_) => S16(json_integer(json, -32768.0, 32767.0).to_int())
    S32(_) => S32(json_integer(json, -2147483648.0, 2147483647.0).to_int())
    S64(_) =>
      S64(
        json_integer(json, -9007199254740991.0, 9007199254740991.0).to_int64(),
      )
    U8(_) => U8(json_integer(json, 0.0, 255.0).to_uint().to_byte())
    U16(_) => U16(json_integer(json, 0.0, 65535.0).to_uint())
    U32(_) => U32(json_integer(json, 0.0, 4294967295.0).to_uint())
    U64(_) => U64(json_integer(json, 0.0, 9007199254740991.0).to_uint64())
    F32(_) => {
      let value = json_number(json)
      guard !value.is_nan() && !value.is_inf() else {
        raise Json("expected a finite number")
      }
      let narrowed = Float::from_double(value)
      guard !narrowed.is_nan() && !narrowed.is_inf() else {
        raise Json("number is outside the f32 range")
      }
      F32(narrowed)
    }
    F64(_) => {
      let value = json_number(json)
      guard !value.is_nan() && !value.is_inf() else {
        raise Json("expected a finite number")
      }
      F64(value)
    }
    Char => {
      let text = json_string(json)
      guard text.char_length() == 1 else {
        raise Json("expected one character")
      }
      Char(text.get_char(0).unwrap())
    }
    String => String(json_string(json))
    Record(expected) => {
      let fields = json_object(json)
      for item in fields {
        let (name, _) = item
        guard expected.any(field => field.name == name) else {
          raise Json("unknown field '" + name + "'")
        }
      }
      let values = expected.map(field => {
        self.pack(self.resolve(field.body), required_field(fields, field.name))
      })
      Record(values)
    }
    Tuple(expected) => {
      let values = json_array(json)
      guard values.length() == expected.length() else {
        raise Json("tuple length does not match schema")
      }
      let packed : Array[@model.SchemaValue] = []
      for index in 0..<expected.length() {
        packed.push(self.pack(self.resolve(expected[index]), values[index]))
      }
      Tuple(packed)
    }
    List(element) =>
      List(
        json_array(json).map(value => self.pack(self.resolve(element), value)),
      )
    FixedList(element, length) => {
      let values = json_array(json)
      guard values.length() == length.reinterpret_as_int() else {
        raise Json("fixed-list length does not match schema")
      }
      FixedList(values.map(value => self.pack(self.resolve(element), value)))
    }
    Option(element) =>
      match json {
        Null => Option(None)
        value => Option(Some(self.pack(self.resolve(element), value)))
      }
    Enum(cases) => {
      let name = json_string(json)
      match find_string_index(cases, name) {
        Some(index) => Enum(index.reinterpret_as_uint())
        None => raise Json("unknown enum case '" + name + "'")
      }
    }
    Flags(names) => {
      let requested_json = json_array(json)
      let requested : Array[String] = []
      for entry in requested_json {
        let name = json_string(entry)
        guard names.any(expected => expected == name) else {
          raise Json("unknown flag '" + name + "'")
        }
        guard !requested.any(existing => existing == name) else {
          raise Json("duplicate flag '" + name + "'")
        }
        requested.push(name)
      }
      Flags(names.map(name => requested.any(requested => requested == name)))
    }
    Variant(cases) =>
      match json {
        String(name) =>
          match find_variant_index(cases, name) {
            Some(index) =>
              if cases[index].payload is None {
                Variant(index.reinterpret_as_uint(), None)
              } else {
                raise Json("variant case '" + name + "' requires a payload")
              }
            None =>
              raise Json("unknown payload-free variant case '" + name + "'")
          }
        Object(fields) => {
          guard fields.length() == 1 else {
            raise Json("expected a single-key variant object")
          }
          let (name, payload_json) = fields.iter().next().unwrap()
          match find_variant_index(cases, name) {
            Some(index) =>
              match cases[index].payload {
                Some(payload) =>
                  Variant(
                    index.reinterpret_as_uint(),
                    Some(self.pack(self.resolve(payload), payload_json)),
                  )
                None => raise Json("variant case '" + name + "' has no payload")
              }
            None => raise Json("unknown payload variant case '" + name + "'")
          }
        }
        _ => raise Json("expected a variant string or object")
      }
    Map(key, entry) =>
      Map(
        json_array(json).map(item => {
          let pair = json_array(item)
          guard pair.length() == 2 else {
            raise Json("expected a two-element map entry")
          }
          {
            key: self.pack(self.resolve(key), pair[0]),
            value: self.pack(self.resolve(entry), pair[1]),
          }
        }),
      )
    Result(ok, err) => {
      let fields = json_object(json)
      guard fields.length() == 1 else {
        raise Json("expected exactly one 'ok' or 'err' result field")
      }
      if fields.get("ok") is Some(payload) {
        ResultOk(
          match ok {
            Some(schema) => Some(self.pack(self.resolve(schema), payload))
            None =>
              if payload is Null {
                None
              } else {
                raise Json("expected null unit ok payload")
              }
          },
        )
      } else if fields.get("err") is Some(payload) {
        ResultErr(
          match err {
            Some(schema) => Some(self.pack(self.resolve(schema), payload))
            None =>
              if payload is Null {
                None
              } else {
                raise Json("expected null unit err payload")
              }
          },
        )
      } else {
        raise Json("expected an 'ok' or 'err' result object")
      }
    }
    Text(_) => {
      let fields = json_object(json)
      Text(
        json_string(required_field(fields, "text")),
        fields.get("language").map(json_string),
      )
    }
    Path(_) => Path(json_string(json))
    Url(_) => Url(json_string(json))
    Union(branches) => {
      let matching = branches.filter(branch => {
        discriminator_matches(branch.discriminator, json)
      })
      guard matching.length() == 1 else {
        raise Json(
          "expected exactly one matching union branch, found \{matching.length()}",
        )
      }
      let branch = matching[0]
      Union(branch.tag, self.pack(self.resolve(branch.body), json))
    }
    Binary(_) => {
      let fields = json_object(json)
      guard fields.length() <= 2 &&
        (fields.length() == 1 || fields.contains("mimeType")) else {
        raise Json("unexpected binary JSON field")
      }
      Binary(
        binary_from_json(json_string(required_field(fields, "bytes"))),
        fields.get("mimeType").map(json_string),
      )
    }
    Quantity(_) => {
      let fields = json_object(json)
      guard fields.length() == 3 else {
        raise Json("quantity JSON requires mantissa, scale, and unit")
      }
      Quantity({
        mantissa: json_integer(
          required_field(fields, "mantissa"),
          -9007199254740991.0,
          9007199254740991.0,
        ).to_int64(),
        scale: json_integer(
          required_field(fields, "scale"),
          -2147483648.0,
          2147483647.0,
        ).to_int(),
        unit: json_string(required_field(fields, "unit")),
      })
    }
    Datetime => Datetime(datetime_from_json(json_string(json)))
    Duration => Duration(duration_from_json(json))
    Secret(_) | QuotaToken(_) | PermissionCard(_) =>
      raise Json("capability values cannot be constructed from JSON")
    Future(_) | Stream(_) =>
      raise Json("future and stream values have no JSON representation")
    Ref(_) => raise Schema("unresolved schema reference")
  }
}
