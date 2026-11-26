// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
/// A schema-directed codec for the frozen public invocation JSON format.
pub(all) struct PublicValueCodec {
  graph : Json
  defs : Map[String, Json]
  stream_elements : Map[String, PublicValueCodec]
}

///|
#warnings("-deprecated")
pub fn public_value_codec(schema_graph_json : String) -> PublicValueCodec raise {
  let graph = @json.parse(schema_graph_json, max_nesting_depth=128)
  let root = pvc_object(graph)
  ignore(pvc_field(root, "root"))
  let defs : Map[String, Json] = Map([])
  match root.get("defs") {
    None => ()
    Some(Json::Array(values)) =>
      for def in values {
        let d = pvc_object(def)
        let id = pvc_string(pvc_field(d, "id"))
        if defs.contains(id) {
          raise BridgeError("Duplicate schema definition '\{id}'")
        }
        defs[id] = pvc_field(d, "body")
      }
    _ => raise BridgeError("Schema graph defs must be an array")
  }
  { graph, defs, stream_elements: {}, }
}

///|
pub fn PublicValueCodec::encode(
  self : PublicValueCodec,
  value : SchemaValue,
) -> Json raise {
  self.stream_elements.clear()
  let charge = Ref(0)
  ignore(
    self.encode_at(
      pvc_field(pvc_object(self.graph), "root"),
      value,
      0,
      charge,
      false,
      materialize=false,
    ),
  )
  schema_value_to_json(value)
}

///|
pub fn PublicValueCodec::encode_application(
  self : PublicValueCodec,
  value : SchemaValue,
) -> Json raise {
  self.stream_elements.clear()
  schema_value_check_text(value)
  self.encode_at(
    pvc_field(pvc_object(self.graph), "root"),
    value,
    0,
    Ref(0),
    true,
  )
}

///|
pub fn PublicValueCodec::decode(
  self : PublicValueCodec,
  value : Json,
) -> SchemaValue raise {
  self.stream_elements.clear()
  let charge = Ref(0)
  let decoded = schema_value_from_json(value)
  ignore(
    self.encode_at(
      pvc_field(pvc_object(self.graph), "root"),
      decoded,
      0,
      charge,
      false,
      materialize=false,
    ),
  )
  decoded
}

///|
fn pvc_fail(message : String) -> Unit raise {
  raise BridgeError("Invalid public schema value: \{message}")
}

///|
fn pvc_object(value : Json) -> Map[String, Json] raise {
  match value {
    Json::Object(object) => object
    _ => {
      pvc_fail("expected an object")
      {}
    }
  }
}

///|
fn pvc_array(value : Json) -> Array[Json] raise {
  match value {
    Json::Array(values) => values
    _ => {
      pvc_fail("expected an array")
      []
    }
  }
}

///|
fn pvc_string(value : Json) -> String raise {
  match value {
    Json::String(value) => value
    _ => {
      pvc_fail("expected a string")
      ""
    }
  }
}

///|
fn pvc_field(object : Map[String, Json], name : String) -> Json raise {
  match object.get(name) {
    Some(value) => value
    None => {
      pvc_fail("missing field '\{name}'")
      Json::null()
    }
  }
}

///|
fn pvc_obj(fields : Array[(String, Json)]) -> Json {
  let object : Map[String, Json] = Map([])
  for field in fields {
    object[field.0] = field.1
  }
  Json::object(object)
}

///|
fn pvc_charge(charge : Ref[Int], amount : Int) -> Unit raise {
  if amount < 0 || charge.val > 16_777_216 - amount {
    pvc_fail("logical value exceeds 16 MiB")
  }
  charge.val = charge.val + amount
}

///|
fn pvc_collection(charge : Ref[Int], length : Int) -> Unit raise {
  if length > 100_000 {
    pvc_fail("collection exceeds 100000 entries")
  }
  pvc_charge(charge, 4)
}

///|
fn pvc_utf8_len(value : String) -> Int {
  @utf8.encode(value).length()
}

///|
fn pvc_regex_matches(pattern : String, value : String) -> Bool {
  try (@string.Regex(pattern).execute(value) is Some(_)) catch {
    _ => false
  }
}

///|
/// The quantity unit selects the quantity type, so a client checks it.
fn pvc_quantity_unit(schema : Map[String, Json], unit : String) -> Unit raise {
  let spec = pvc_object(pvc_field(schema, "spec"))
  let suffixes = match spec.get("allowedSuffixes") {
    Some(v) => pvc_array(v).map(pvc_string)
    None => []
  }
  let unit_ok = if suffixes.is_empty() {
    unit == pvc_string(pvc_field(spec, "baseUnit"))
  } else {
    suffixes.any(fn(x) { x == unit })
  }
  if !unit_ok {
    pvc_fail("quantity unit is not allowed")
  }
}

///|
fn pvc_url_scheme(value : String) -> Bool {
  let chars = value.to_array()
  !chars.is_empty() &&
  ((chars[0] >= 'A' && chars[0] <= 'Z') || (chars[0] >= 'a' && chars[0] <= 'z')) &&
  chars[1:].all(fn(c) {
    (c >= 'A' && c <= 'Z') ||
    (c >= 'a' && c <= 'z') ||
    (c >= '0' && c <= '9') ||
    c == '+' ||
    c == '.' ||
    c == '-'
  })
}

///|
fn pvc_url_form(url : String) -> Unit raise {
  let url_parts = url.split(":").to_array()
  if url_parts.length() < 2 {
    pvc_fail("invalid URL")
    return
  }
  if !pvc_url_scheme(url_parts[0].to_owned()) {
    pvc_fail("invalid URL scheme")
  }
  if url.any(fn(c) { c <= '\u{20}' || c == '\u{7f}' }) {
    pvc_fail("invalid URL")
  }
}

///|
fn pvc_leap_year(year : Int) -> Bool {
  year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

///|
fn pvc_datetime(value : String) -> Bool raise {
  if !pvc_regex_matches(
      "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\\.[0-9]{1,9})?Z$",
      value,
    ) {
    return false
  }
  let year = @string.parse_int(value[0:4])
  let month = @string.parse_int(value[5:7])
  let day = @string.parse_int(value[8:10])
  let hour = @string.parse_int(value[11:13])
  let minute = @string.parse_int(value[14:16])
  let second = @string.parse_int(value[17:19])
  if month < 1 || month > 12 || hour > 23 || minute > 59 || second > 59 {
    return false
  }
  let max_day = match month {
    2 => if pvc_leap_year(year) { 29 } else { 28 }
    4 | 6 | 9 | 11 => 30
    _ => 31
  }
  day >= 1 && day <= max_day
}

///|
fn pvc_canonical_datetime(value : String) -> String {
  let without_z = value[0:value.length() - 1].to_owned()
  match without_z.split(".").to_array() {
    [whole] => whole.to_owned() + ".000000000Z"
    [whole, fraction] =>
      whole.to_owned() +
      "." +
      fraction.to_owned() +
      "0".repeat(9 - fraction.length()) +
      "Z"
    _ => value
  }
}

///|
fn PublicValueCodec::resolved(
  self : PublicValueCodec,
  schema : Json,
) -> Json raise {
  let mut current = schema
  let mut visited : Map[String, Bool]? = None
  for ;; {
    let node = pvc_object(current)
    if pvc_string(pvc_field(node, "kind")) != "ref" {
      return current
    }
    let value = pvc_object(pvc_field(node, "value"))
    let id = pvc_string(pvc_field(value, "id"))
    let seen = match visited {
      Some(seen) => seen
      None => {
        let seen : Map[String, Bool] = Map([])
        visited = Some(seen)
        seen
      }
    }
    if seen.contains(id) {
      pvc_fail("cyclic schema alias '\{id}'")
    }
    seen[id] = true
    current = match self.defs.get(id) {
      Some(body) => body
      None => {
        pvc_fail("unresolved schema reference '\{id}'")
        Json::null()
      }
    }
  }
}

///|
fn PublicValueCodec::element_codec(
  self : PublicValueCodec,
  schema : Json,
) -> PublicValueCodec {
  let serialized_defs : Array[Json] = []
  self.defs.each((id, body) => {
    serialized_defs.push(pvc_obj([("body", body), ("id", Json::string(id))]))
  })
  let graph = pvc_obj([("defs", Json::array(serialized_defs)), ("root", schema)])
  { graph, defs: self.defs, stream_elements: {}, }
}

///|
fn pvc_schema(schema : Json) -> (String, Map[String, Json]) raise {
  let node = pvc_object(schema)
  (pvc_string(pvc_field(node, "kind")), pvc_object(pvc_field(node, "value")))
}

///|
fn pvc_canonical_unsigned(value : String) -> Bool {
  if value.is_empty() {
    return false
  }
  if value != "0" && value[0] == '0' {
    return false
  }
  value.all(fn(c) { c >= '0' && c <= '9' })
}

///|
fn pvc_u32(value : Json) -> UInt raise {
  let literal = number_literal(value)
  if !pvc_canonical_unsigned(literal) {
    pvc_fail("integer is not canonical")
  }
  @string.parse_uint(literal)
}

///|
fn pvc_encode_float(value : Double) -> Json {
  json_double(value)
}

///|
fn pvc_mime(value : String) -> Bool {
  let parts = value.split("/").to_array()
  if parts.length() != 2 || parts[0].is_empty() || parts[1].is_empty() {
    return false
  }
  parts.all(fn(part) {
    part.all(fn(c) {
      (c >= 'A' && c <= 'Z') ||
      (c >= 'a' && c <= 'z') ||
      (c >= '0' && c <= '9') ||
      c == '!' ||
      c == '#' ||
      c == '$' ||
      c == '&' ||
      c == '^' ||
      c == '_' ||
      c == '.' ||
      c == '+' ||
      c == '-'
    })
  })
}

///|
fn pvc_base64_encode(bytes : Bytes) -> String {
  let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
  let out = StringBuilder()
  for i = 0; i < bytes.length(); i = i + 3 {
    let a = bytes[i].to_int()
    let have_b = i + 1 < bytes.length()
    let have_c = i + 2 < bytes.length()
    let b = if have_b { bytes[i + 1].to_int() } else { 0 }
    let c = if have_c { bytes[i + 2].to_int() } else { 0 }
    out.write_char(alphabet.get_char(a >> 2).unwrap())
    out.write_char(alphabet.get_char(((a & 3) << 4) | (b >> 4)).unwrap())
    out.write_char(
      if have_b {
        alphabet.get_char(((b & 15) << 2) | (c >> 6)).unwrap()
      } else {
        '='
      },
    )
    out.write_char(
      if have_c {
        alphabet.get_char(c & 63).unwrap()
      } else {
        '='
      },
    )
  }
  out.to_string()
}

///|
fn pvc_base64url_encode(bytes : Bytes) -> String {
  pvc_base64_encode(bytes)
  .replace(old="+", new="-")
  .replace(old="/", new="_")
  .trim_end(chars="=")
  .to_owned()
}

///|
fn pvc_string_view(value : SchemaValue) -> String? {
  match value {
    StringValue(v) | PathValue(v) | UrlValue(v) => Some(v)
    TextValue(v, _) => Some(v)
    _ => None
  }
}

///|
fn pvc_discriminator(
  codec : PublicValueCodec,
  body_schema : Json,
  body : SchemaValue,
  branches : Array[Json],
  tag : String,
) -> Unit raise {
  let mut branch = Json::null()
  for candidate in branches {
    if pvc_string(pvc_field(pvc_object(candidate), "tag")) == tag {
      branch = candidate
    }
  }
  let b = pvc_object(branch)
  let disc = pvc_object(pvc_field(b, "discriminator"))
  let rule = pvc_string(pvc_field(disc, "rule"))
  let value = pvc_object(pvc_field(disc, "value"))
  let matches = match rule {
    "prefix" =>
      pvc_string_view(body)
      .map(s => s.has_prefix(pvc_string(pvc_field(value, "prefix"))))
      .unwrap_or(false)
    "suffix" =>
      pvc_string_view(body)
      .map(s => s.has_suffix(pvc_string(pvc_field(value, "suffix"))))
      .unwrap_or(false)
    "contains" =>
      pvc_string_view(body)
      .map(s => s.contains(pvc_string(pvc_field(value, "substring"))))
      .unwrap_or(false)
    "regex" =>
      pvc_string_view(body)
      .map(s => pvc_regex_matches(pvc_string(pvc_field(value, "regex")), s))
      .unwrap_or(false)
    "field-equals" =>
      match body {
        RecordValue(fields) => {
          let resolved = pvc_object(codec.resolved(body_schema))
          let names = pvc_array(
            pvc_field(pvc_object(pvc_field(resolved, "value")), "fields"),
          )
          let field_name = pvc_string(pvc_field(value, "fieldName"))
          let mut found : SchemaValue? = None
          for i, field in names {
            if pvc_string(pvc_field(pvc_object(field), "name")) == field_name {
              found = Some(fields[i])
            }
          }
          match (found, value.get("literal")) {
            (Some(_), None) => true
            (Some(v), Some(raw)) => pvc_string_view(v) == Some(pvc_string(raw))
            _ => false
          }
        }
        _ => false
      }
    "field-absent" =>
      match body {
        RecordValue(_) => {
          let resolved = pvc_object(codec.resolved(body_schema))
          let names = pvc_array(
            pvc_field(pvc_object(pvc_field(resolved, "value")), "fields"),
          )
          let field_name = pvc_string(pvc_field(value, "fieldName"))
          !names.any(field => {
            pvc_string(pvc_field(pvc_object(field), "name")) == field_name
          })
        }
        _ => false
      }
    _ => false
  }
  if !matches {
    pvc_fail("union discriminator does not match selected branch")
  }
}

///|
fn pvc_uuid_v4(value : String) -> Bool {
  if value.length() != 36 ||
    value[8] != '-' ||
    value[13] != '-' ||
    value[18] != '-' ||
    value[23] != '-' ||
    value[14] != '4' {
    return false
  }
  for i = 0; i < value.length(); i = i + 1 {
    if i == 8 || i == 13 || i == 18 || i == 23 {
      continue
    }
    let c = value[i]
    if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f')) {
      return false
    }
  }
  value[19] == '8' || value[19] == '9' || value[19] == 'a' || value[19] == 'b'
}

///|
fn PublicValueCodec::encode_at(
  self : PublicValueCodec,
  raw_schema : Json,
  value : SchemaValue,
  depth : Int,
  charge : Ref[Int],
  application : Bool,
  materialize? : Bool = true,
) -> Json raise {
  if depth >= 64 {
    pvc_fail("nesting exceeds 64 levels")
  }
  pvc_charge(charge, 1)
  let (kind, schema) = pvc_schema(self.resolved(raw_schema))
  match (kind, value) {
    ("bool", BoolValue(v)) => {
      pvc_charge(charge, 1)
      if materialize {
        Json::boolean(v)
      } else {
        Json::null()
      }
    }
    ("s8", S8Value(v)) => {
      if v < -128 || v > 127 {
        pvc_fail("s8 out of range")
      }
      pvc_charge(charge, 1)
      if materialize {
        json_int(v)
      } else {
        Json::null()
      }
    }
    ("s16", S16Value(v)) => {
      if v < -32768 || v > 32767 {
        pvc_fail("s16 out of range")
      }
      pvc_charge(charge, 2)
      if materialize {
        json_int(v)
      } else {
        Json::null()
      }
    }
    ("s32", S32Value(v)) => {
      pvc_charge(charge, 4)
      if materialize {
        json_int(v)
      } else {
        Json::null()
      }
    }
    ("s64", S64Value(v)) => {
      pvc_charge(charge, 8)
      if materialize {
        Json::string(v.to_string())
      } else {
        Json::null()
      }
    }
    ("u8", U8Value(v)) => {
      if v < 0 || v > 255 {
        pvc_fail("u8 out of range")
      }
      pvc_charge(charge, 1)
      if materialize {
        json_int(v)
      } else {
        Json::null()
      }
    }
    ("u16", U16Value(v)) => {
      if v < 0 || v > 65535 {
        pvc_fail("u16 out of range")
      }
      pvc_charge(charge, 2)
      if materialize {
        json_int(v)
      } else {
        Json::null()
      }
    }
    ("u32", U32Value(v)) => {
      pvc_charge(charge, 4)
      if materialize {
        json_uint(v)
      } else {
        Json::null()
      }
    }
    ("u64", U64Value(v)) => {
      pvc_charge(charge, 8)
      if materialize {
        Json::string(v.to_string())
      } else {
        Json::null()
      }
    }
    ("f32", F32Value(v)) => {
      if application && (v.is_nan() || v.is_inf()) {
        pvc_fail("exceptional floats have no application JSON representation")
      }
      pvc_charge(charge, 4)
      if materialize {
        pvc_encode_float(v.to_double())
      } else {
        Json::null()
      }
    }
    ("f64", F64Value(v)) => {
      if application && (v.is_nan() || v.is_inf()) {
        pvc_fail("exceptional floats have no application JSON representation")
      }
      pvc_charge(charge, 8)
      if materialize {
        pvc_encode_float(v)
      } else {
        Json::null()
      }
    }
    ("char", CharValue(v)) => {
      let s = String::from_array([v.unsafe_to_char()])
      pvc_charge(charge, @utf8.encode(s).length())
      if materialize {
        Json::string(s)
      } else {
        Json::null()
      }
    }
    ("string", StringValue(v)) => {
      pvc_charge(charge, @utf8.encode(v).length())
      if materialize {
        Json::string(v)
      } else {
        Json::null()
      }
    }
    ("uuid", UuidValue(v)) => {
      pvc_charge(charge, 36)
      if materialize {
        Json::string(v.to_string())
      } else {
        Json::null()
      }
    }
    ("record", RecordValue(v)) =>
      self.encode_record(schema, v, depth, charge, application, materialize)
    ("variant", VariantValue(i, v)) =>
      self.encode_variant(schema, i, v, depth, charge, application, materialize)
    ("enum", EnumValue(i)) => {
      let cases = pvc_array(pvc_field(schema, "cases"))
      if i < 0 || i >= cases.length() {
        pvc_fail("enum case out of range")
      }
      let name = pvc_string(cases[i])
      pvc_charge(charge, pvc_utf8_len(name))
      if materialize {
        Json::string(name)
      } else {
        Json::null()
      }
    }
    ("flags", FlagsValue(bits)) => {
      let names = pvc_array(pvc_field(schema, "flags"))
      if bits.length() != names.length() {
        pvc_fail("flags arity mismatch")
      }
      let out : Array[Json] = []
      let mut selected = 0
      for i, set in bits {
        if set {
          let name = pvc_string(names[i])
          pvc_charge(charge, pvc_utf8_len(name))
          selected += 1
          if materialize {
            out.push(Json::string(name))
          }
        }
      }
      pvc_collection(charge, selected)
      if materialize {
        Json::array(out)
      } else {
        Json::null()
      }
    }
    ("tuple", TupleValue(v)) =>
      self.encode_sequence(
        pvc_array(pvc_field(schema, "elements")),
        v,
        depth,
        charge,
        application,
        materialize,
      )
    ("list", ListValue(v)) =>
      self.encode_homogeneous(
        pvc_field(schema, "element"),
        v,
        depth,
        charge,
        application,
        materialize,
      )
    ("fixed-list", FixedListValue(v)) => {
      if v.length() != pvc_u32(pvc_field(schema, "length")).reinterpret_as_int() {
        pvc_fail("fixed-list length mismatch")
      }
      self.encode_homogeneous(
        pvc_field(schema, "element"),
        v,
        depth,
        charge,
        application,
        materialize,
      )
    }
    ("map", MapValue(v)) => {
      pvc_collection(charge, v.length())
      let out : Array[Json] = []
      for e in v {
        let key = self.encode_at(
          pvc_field(schema, "key"),
          e.key,
          depth + 1,
          charge,
          application,
          materialize~,
        )
        let value = self.encode_at(
          pvc_field(schema, "value"),
          e.value,
          depth + 1,
          charge,
          application,
          materialize~,
        )
        if materialize {
          out.push(Json::array([key, value]))
        }
      }
      if materialize {
        Json::array(out)
      } else {
        Json::null()
      }
    }
    ("option", OptionValue(v)) =>
      match v {
        None => {
          pvc_charge(charge, 4)
          Json::null()
        }
        Some(x) => {
          pvc_charge(charge, 4)
          self.encode_at(
            pvc_field(schema, "inner"),
            x,
            depth + 1,
            charge,
            application,
            materialize~,
          )
        }
      }
    ("result", ResultValue(v)) =>
      self.encode_result(schema, v, depth, charge, application, materialize)
    ("text", TextValue(text, language)) => {
      let extra = match language {
        Some(x) => @utf8.encode(x).length()
        None => 0
      }
      pvc_charge(charge, @utf8.encode(text).length() + extra)
      if !materialize {
        return Json::null()
      }
      let f = [("text", Json::string(text))]
      match language {
        Some(x) => f.push(("language", Json::string(x)))
        None => ()
      }
      pvc_obj(f)
    }
    ("binary", BinaryValue(bytes, mime)) => {
      match mime {
        Some(x) => if !pvc_mime(x) { pvc_fail("invalid MIME type") }
        None => ()
      }
      let extra = match mime {
        Some(x) => pvc_utf8_len(x)
        None => 0
      }
      pvc_charge(charge, bytes.length() + extra)
      if !materialize {
        return Json::null()
      }
      let f = [("bytes", Json::string(pvc_base64url_encode(bytes)))]
      match mime {
        Some(x) => f.push(("mimeType", Json::string(x)))
        None => ()
      }
      pvc_obj(f)
    }
    ("path", PathValue(v)) => {
      pvc_charge(charge, pvc_utf8_len(v))
      if materialize {
        Json::string(v)
      } else {
        Json::null()
      }
    }
    ("url", UrlValue(v)) => {
      if v.is_empty() {
        pvc_fail("URL must not be empty")
      }
      pvc_url_form(v)
      pvc_charge(charge, pvc_utf8_len(v))
      if materialize {
        Json::string(v)
      } else {
        Json::null()
      }
    }
    ("datetime", DatetimeValue(v)) => {
      if !pvc_datetime(v) {
        pvc_fail("datetime is not canonical RFC3339 UTC")
      }
      pvc_charge(charge, pvc_utf8_len(v))
      if materialize {
        Json::string(if application { pvc_canonical_datetime(v) } else { v })
      } else {
        Json::null()
      }
    }
    ("duration", DurationValue(v)) => {
      pvc_charge(charge, 8)
      if materialize {
        pvc_obj([("nanoseconds", Json::string(v.to_string()))])
      } else {
        Json::null()
      }
    }
    ("quantity", QuantityValue(mantissa, scale, unit)) => {
      pvc_quantity_unit(schema, unit)
      pvc_charge(charge, 12 + pvc_utf8_len(unit))
      if !materialize {
        return Json::null()
      }
      pvc_obj([
        ("mantissa", Json::string(mantissa.to_string())),
        ("scale", json_int(scale)),
        ("unit", Json::string(unit)),
      ])
    }
    ("union", UnionValue(tag, body)) => {
      let branches = pvc_array(
        pvc_field(pvc_object(pvc_field(schema, "spec")), "branches"),
      )
      let mut ty = Json::null()
      let mut found = false
      for b in branches {
        let bo = pvc_object(b)
        if pvc_string(pvc_field(bo, "tag")) == tag {
          ty = pvc_field(bo, "body")
          found = true
        }
      }
      if !found {
        pvc_fail("unknown union branch")
      }
      pvc_discriminator(self, ty, body, branches, tag)
      pvc_charge(charge, pvc_utf8_len(tag))
      self.encode_at(ty, body, depth + 1, charge, application, materialize~)
    }
    ("stream", StreamReferenceValue(provisional, token)) => {
      if application {
        pvc_fail("stream values have no application JSON representation")
      }
      match (provisional, token) {
        (Some(v), None) => {
          if !pvc_uuid_v4(v) {
            pvc_fail("invalid provisional stream reference")
          }
          if self.stream_elements.contains(v) {
            pvc_fail("stream reference appears more than once")
          }
          self.stream_elements[v] = self.element_codec(
            pvc_field(schema, "inner"),
          )
          pvc_charge(charge, 16)
          if !materialize {
            return Json::null()
          }
          pvc_obj([
            ("kind", Json::string("stream")),
            ("value", pvc_obj([("provisionalRef", Json::string(v))])),
          ])
        }
        (None, Some(v)) => {
          if v.is_empty() || pvc_utf8_len(v) > 8192 {
            pvc_fail("invalid stream token")
          }
          if self.stream_elements.contains(v) {
            pvc_fail("stream reference appears more than once")
          }
          self.stream_elements[v] = self.element_codec(
            pvc_field(schema, "inner"),
          )
          pvc_charge(charge, pvc_utf8_len(v))
          if !materialize {
            return Json::null()
          }
          pvc_obj([
            ("kind", Json::string("stream")),
            ("value", pvc_obj([("streamToken", Json::string(v))])),
          ])
        }
        _ => {
          pvc_fail("stream must have exactly one identity")
          Json::null()
        }
      }
    }
    ("secret" | "quota-token" | "permission-card" | "future", _) => {
      pvc_fail("unsupported value kind '\{kind}'")
      Json::null()
    }
    _ => {
      pvc_fail("value does not match schema kind '\{kind}'")
      Json::null()
    }
  }
}

///|
fn PublicValueCodec::encode_record(
  self : PublicValueCodec,
  schema : Map[String, Json],
  values : Array[SchemaValue],
  depth : Int,
  charge : Ref[Int],
  application : Bool,
  materialize : Bool,
) -> Json raise {
  let fields = pvc_array(pvc_field(schema, "fields"))
  if fields.length() != values.length() {
    pvc_fail("record arity mismatch")
  }
  pvc_collection(charge, values.length())
  let out : Map[String, Json] = Map([])
  for i, field in fields {
    let f = pvc_object(field)
    let name = pvc_string(pvc_field(f, "name"))
    pvc_charge(charge, pvc_utf8_len(name))
    let encoded = self.encode_at(
      pvc_field(f, "body"),
      values[i],
      depth + 1,
      charge,
      application,
      materialize~,
    )
    if materialize {
      out[name] = encoded
    }
  }
  if materialize {
    Json::object(out)
  } else {
    Json::null()
  }
}

///|
fn PublicValueCodec::encode_variant(
  self : PublicValueCodec,
  schema : Map[String, Json],
  index : Int,
  payload : SchemaValue?,
  depth : Int,
  charge : Ref[Int],
  application : Bool,
  materialize : Bool,
) -> Json raise {
  let cases = pvc_array(pvc_field(schema, "cases"))
  if index < 0 || index >= cases.length() {
    pvc_fail("variant case out of range")
  }
  let case = pvc_object(cases[index])
  let name = pvc_string(pvc_field(case, "name"))
  pvc_charge(charge, pvc_utf8_len(name))
  let ty = case.get("payload")
  if (ty is Some(_)) != (payload is Some(_)) {
    pvc_fail("variant payload mismatch")
  }
  match (ty, payload) {
    (Some(t), Some(v)) => {
      let encoded = self.encode_at(
        t,
        v,
        depth + 1,
        charge,
        application,
        materialize~,
      )
      if materialize {
        pvc_obj([(name, encoded)])
      } else {
        Json::null()
      }
    }
    _ => if materialize { Json::string(name) } else { Json::null() }
  }
}

///|
fn PublicValueCodec::encode_sequence(
  self : PublicValueCodec,
  types : Array[Json],
  values : Array[SchemaValue],
  depth : Int,
  charge : Ref[Int],
  application : Bool,
  materialize : Bool,
) -> Json raise {
  if types.length() != values.length() {
    pvc_fail("tuple arity mismatch")
  }
  pvc_collection(charge, values.length())
  let out : Array[Json] = []
  for i, v in values {
    let encoded = self.encode_at(
      types[i],
      v,
      depth + 1,
      charge,
      application,
      materialize~,
    )
    if materialize {
      out.push(encoded)
    }
  }
  if materialize {
    Json::array(out)
  } else {
    Json::null()
  }
}

///|
fn PublicValueCodec::encode_homogeneous(
  self : PublicValueCodec,
  ty : Json,
  values : Array[SchemaValue],
  depth : Int,
  charge : Ref[Int],
  application : Bool,
  materialize : Bool,
) -> Json raise {
  pvc_collection(charge, values.length())
  let out : Array[Json] = []
  for v in values {
    let encoded = self.encode_at(
      ty,
      v,
      depth + 1,
      charge,
      application,
      materialize~,
    )
    if materialize {
      out.push(encoded)
    }
  }
  if materialize {
    Json::array(out)
  } else {
    Json::null()
  }
}

///|
fn PublicValueCodec::encode_result(
  self : PublicValueCodec,
  schema : Map[String, Json],
  result : SchemaResult,
  depth : Int,
  charge : Ref[Int],
  application : Bool,
  materialize : Bool,
) -> Json raise {
  let spec = pvc_object(pvc_field(schema, "spec"))
  let (tag, value) = match result {
    ResultOk(v) => ("ok", v)
    ResultErr(v) => ("err", v)
  }
  let ty = spec.get(tag)
  if (ty is Some(_)) != (value is Some(_)) {
    pvc_fail("result payload mismatch")
  }
  pvc_charge(charge, pvc_utf8_len(tag))
  match (ty, value) {
    (Some(t), Some(v)) => {
      let encoded = self.encode_at(
        t,
        v,
        depth + 1,
        charge,
        application,
        materialize~,
      )
      if materialize {
        pvc_obj([(tag, encoded)])
      } else {
        Json::null()
      }
    }
    _ => if materialize { pvc_obj([(tag, Json::null())]) } else { Json::null() }
  }
}
