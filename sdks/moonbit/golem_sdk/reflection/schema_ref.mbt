///|
/// A graph-backed view of one reflected schema root.
pub(all) struct SchemaRef {
  graph : @model.SchemaGraph
  root : @model.SchemaType
} derive(Debug, Eq)

///|
pub fn SchemaRef::new(graph : @model.SchemaGraph) -> SchemaRef {
  { root: graph.root, graph, }
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
  self.pack(self.resolve(self.root), value)
}

///|
pub fn SchemaRef::unpack_json(
  self : SchemaRef,
  value : @model.SchemaValue,
) -> Json raise ReflectError {
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
    (@model.SchemaTypeBody::S64(_), @model.SchemaValue::S64(value)) =>
      Json::number(value.to_double())
    (@model.SchemaTypeBody::U8(_), @model.SchemaValue::U8(value)) =>
      Json::number(value.to_int().to_double())
    (@model.SchemaTypeBody::U16(_), @model.SchemaValue::U16(value))
    | (@model.SchemaTypeBody::U32(_), @model.SchemaValue::U32(value)) =>
      Json::number(value.reinterpret_as_int().to_double())
    (@model.SchemaTypeBody::U64(_), @model.SchemaValue::U64(value)) =>
      Json::number(value.to_double())
    (@model.SchemaTypeBody::F32(_), @model.SchemaValue::F32(value)) =>
      Json::number(value.to_double())
    (@model.SchemaTypeBody::F64(_), @model.SchemaValue::F64(value)) =>
      Json::number(value)
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
    (@model.SchemaTypeBody::Binary(_), @model.SchemaValue::Binary(_, _))
    | (@model.SchemaTypeBody::Datetime, @model.SchemaValue::Datetime(_))
    | (@model.SchemaTypeBody::Duration, @model.SchemaValue::Duration(_))
    | (@model.SchemaTypeBody::Quantity(_), @model.SchemaValue::Quantity(_)) =>
      raise Json("this rich schema type is not yet supported by the JSON codec")
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
      F32(Float::from_double(value))
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
    Binary(_) | Datetime | Duration | Quantity(_) =>
      raise Json("this rich schema type is not yet supported by the JSON codec")
    Secret(_) | QuotaToken(_) | PermissionCard(_) =>
      raise Json("capability values cannot be constructed from JSON")
    Future(_) | Stream(_) =>
      raise Json("future and stream values have no JSON representation")
    Ref(_) => raise Schema("unresolved schema reference")
  }
}
