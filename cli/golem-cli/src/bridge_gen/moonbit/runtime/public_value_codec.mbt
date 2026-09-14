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
  let charge = Ref(0)
  self.encode_at(pvc_field(pvc_object(self.graph), "root"), value, 0, charge)
}

///|
pub fn PublicValueCodec::decode(
  self : PublicValueCodec,
  value : Json,
) -> SchemaValue raise {
  let charge = Ref(0)
  self.decode_at(pvc_field(pvc_object(self.graph), "root"), value, 0, charge)
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
fn pvc_bool(value : Json) -> Bool raise {
  match value {
    Json::True => true
    Json::False => false
    _ => {
      pvc_fail("expected a boolean")
      false
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
fn pvc_exact(object : Map[String, Json], names : Array[String]) -> Unit raise {
  if object.length() != names.length() {
    pvc_fail("object members do not match the expected representation")
  }
  for name, _ in object {
    if !names.any(fn(expected) { expected == name }) {
      pvc_fail("unexpected object member '\{name}'")
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
fn pvc_language(value : String) -> Bool {
  pvc_regex_matches("^[A-Za-z]{1,8}(?:-[A-Za-z0-9]{1,8})*$", value)
}

///|
fn pvc_text_restrictions(
  schema : Map[String, Json],
  text : String,
  language : String?,
) -> Unit raise {
  guard schema.get("restrictions") is Some(raw) else { return }
  let r = pvc_object(raw)
  match (r.get("languages"), language) {
    (Some(v), Some(lang)) =>
      if !pvc_array(v).map(pvc_string).any(fn(x) { x == lang }) {
        pvc_fail("text language is not allowed")
      }
    _ => ()
  }
  match r.get("minLength") {
    Some(v) =>
      if text.char_length().reinterpret_as_uint() < pvc_u32(v) {
        pvc_fail("text is shorter than minLength")
      }
    None => ()
  }
  match r.get("maxLength") {
    Some(v) =>
      if text.char_length().reinterpret_as_uint() > pvc_u32(v) {
        pvc_fail("text is longer than maxLength")
      }
    None => ()
  }
  match r.get("regex") {
    Some(v) =>
      if !pvc_regex_matches(pvc_string(v), text) {
        pvc_fail("text does not match regex")
      }
    None => ()
  }
}

///|
fn pvc_numeric_bound(bound : Json) -> (String, Json) raise {
  let o = pvc_object(bound)
  (pvc_string(pvc_field(o, "kind")), pvc_field(o, "value"))
}

///|
fn pvc_integer_text(value : Json) -> String raise {
  match value {
    Json::String(v) => v
    Json::Number(_) => number_literal(value)
    _ => {
      pvc_fail("expected integer text")
      ""
    }
  }
}

///|
fn pvc_numeric_restrictions(
  schema : Map[String, Json],
  family : String,
  signed : Int64?,
  unsigned : UInt64?,
  float : Double?,
) -> Unit raise {
  guard schema.get("restrictions") is Some(raw) else { return }
  let r = pvc_object(raw)
  for name in ["min", "max"] {
    guard r.get(name) is Some(raw_bound) else { continue }
    let (kind, raw_value) = pvc_numeric_bound(raw_bound)
    let ok = match (family, kind, signed, unsigned, float) {
      ("signed", "signed", Some(v), _, _) => {
        let b = @string.parse_int64(pvc_integer_text(raw_value))
        if name == "min" {
          v >= b
        } else {
          v <= b
        }
      }
      ("unsigned", "unsigned", _, Some(v), _) => {
        let b = @string.parse_uint64(pvc_integer_text(raw_value))
        if name == "min" {
          v >= b
        } else {
          v <= b
        }
      }
      ("float", "float-bits", _, _, Some(v)) => {
        let b = @string.parse_uint64(pvc_integer_text(raw_value)).reinterpret_as_double()
        if name == "min" {
          v >= b
        } else {
          v <= b
        }
      }
      _ => true
    }
    if !ok {
      pvc_fail("numeric value is outside restrictions")
    }
  }
}

///|
fn pvc_quantity_from_json(value : Json) -> (Int64, Int, String) raise {
  let o = pvc_object(value)
  pvc_exact(o, ["mantissa", "scale", "unit"])
  let mantissa = pvc_string(pvc_field(o, "mantissa"))
  if !pvc_canonical_signed(mantissa) {
    pvc_fail("quantity mantissa is not canonical")
  }
  (
    @string.parse_int64(mantissa),
    pvc_int(pvc_field(o, "scale"), -2147483648L, 2147483647L).to_int(),
    pvc_string(pvc_field(o, "unit")),
  )
}

///|
fn pvc_quantity_bound(value : Json) -> (Int64, Int) raise {
  let o = pvc_object(value)
  (
    @string.parse_int64(pvc_integer_text(pvc_field(o, "mantissa"))),
    pvc_int(pvc_field(o, "scale"), -2147483648L, 2147483647L).to_int(),
  )
}

///|
fn pvc_quantity_le(
  a_m : Int64,
  a_s : Int,
  b_m : Int64,
  b_s : Int,
) -> Bool raise {
  let common = if a_s > b_s { a_s } else { b_s }
  let a_shift = common.to_int64() - a_s.to_int64()
  let b_shift = common.to_int64() - b_s.to_int64()
  if a_shift > 38L || b_shift > 38L {
    pvc_fail("quantity comparison overflow")
  }
  let ten = @bigint.BigInt::from_int(10)
  let a = @bigint.BigInt::from_int64(a_m) *
    ten.pow(@bigint.BigInt::from_int(a_shift.to_int()))
  let b = @bigint.BigInt::from_int64(b_m) *
    ten.pow(@bigint.BigInt::from_int(b_shift.to_int()))
  let min_i128 = @bigint.BigInt::from_string(
    "-170141183460469231731687303715884105728",
  )
  let max_i128 = @bigint.BigInt::from_string(
    "170141183460469231731687303715884105727",
  )
  if a < min_i128 || a > max_i128 || b < min_i128 || b > max_i128 {
    pvc_fail("quantity comparison overflow")
  }
  a <= b
}

///|
fn pvc_quantity_restrictions(
  schema : Map[String, Json],
  mantissa : Int64,
  scale : Int,
  unit : String,
) -> Unit raise {
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
  match spec.get("min") {
    Some(v) => {
      let (m, s) = pvc_quantity_bound(v)
      if !pvc_quantity_le(m, s, mantissa, scale) {
        pvc_fail("quantity is below minimum")
      }
    }
    None => ()
  }
  match spec.get("max") {
    Some(v) => {
      let (m, s) = pvc_quantity_bound(v)
      if !pvc_quantity_le(mantissa, scale, m, s) {
        pvc_fail("quantity is above maximum")
      }
    }
    None => ()
  }
}

///|
fn pvc_path_restrictions(
  schema : Map[String, Json],
  path : String,
) -> Unit raise {
  let spec = match schema.get("spec") {
    Some(v) => pvc_object(v)
    None => schema
  }
  guard spec.get("allowedExtensions") is Some(v) else { return }
  let name = path.split("/").to_array().last().unwrap_or("")
  let parts = name.split(".").to_array()
  if parts.length() > 1 {
    let ext = parts.last().unwrap().to_owned()
    if !ext.is_empty() && !pvc_array(v).map(pvc_string).any(fn(x) { x == ext }) {
      pvc_fail("path extension is not allowed")
    }
  }
}

///|
fn pvc_ascii_lower(value : String) -> String {
  let out = StringBuilder()
  for c in value {
    out.write_char(
      if c >= 'A' && c <= 'Z' {
        (c.to_int() + 32).unsafe_to_char()
      } else {
        c
      },
    )
  }
  out.to_string()
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
fn pvc_url_restrictions(schema : Map[String, Json], url : String) -> Unit raise {
  let url_parts = url.split(":").to_array()
  if url_parts.length() < 2 {
    pvc_fail("invalid URL")
    return
  }
  let scheme = url_parts[0].to_owned()
  if !pvc_url_scheme(scheme) {
    pvc_fail("invalid URL scheme")
  }
  if url.any(fn(c) { c <= '\u{20}' || c == '\u{7f}' }) {
    pvc_fail("invalid URL")
  }
  let remainder = url[scheme.length() + 1:]
  let restrictions = match schema.get("restrictions") {
    Some(v) => pvc_object(v)
    None => return
  }
  match restrictions.get("allowedSchemes") {
    Some(v) =>
      if !pvc_array(v)
        .map(pvc_string)
        .any(fn(x) { pvc_ascii_lower(x) == pvc_ascii_lower(scheme) }) {
        pvc_fail("URL scheme is not allowed")
      }
    None => ()
  }
  match restrictions.get("allowedHosts") {
    Some(v) => {
      let authority_path = match remainder.strip_prefix("//") {
        Some(value) => value.to_owned()
        None => {
          pvc_fail("URL host is missing")
          ""
        }
      }
      let authority = authority_path.split("/").to_array()[0]
        .split("?")
        .to_array()[0]
        .split("#")
        .to_array()[0]
      let host_port = authority.split("@").to_array().last().unwrap_or("")
      let host = if host_port.strip_prefix("[") is Some(ipv6) {
        match ipv6.split("]").to_array() {
          [host, ..] => host.to_owned()
          _ => {
            pvc_fail("invalid URL host")
            ""
          }
        }
      } else {
        host_port.split(":").to_array()[0].to_owned()
      }
      if host.is_empty() ||
        !pvc_array(v)
        .map(pvc_string)
        .any(fn(x) { pvc_ascii_lower(x) == pvc_ascii_lower(host) }) {
        pvc_fail("URL host is not allowed")
      }
    }
    None => ()
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
fn PublicValueCodec::resolved(
  self : PublicValueCodec,
  schema : Json,
) -> Json raise {
  let mut current = schema
  let visited : Map[String, Bool] = Map([])
  for ;; {
    let node = pvc_object(current)
    if pvc_string(pvc_field(node, "kind")) != "ref" {
      return current
    }
    let value = pvc_object(pvc_field(node, "value"))
    let id = pvc_string(pvc_field(value, "id"))
    if visited.contains(id) {
      pvc_fail("cyclic schema alias '\{id}'")
    }
    visited[id] = true
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
fn pvc_canonical_signed(value : String) -> Bool {
  match value.strip_prefix("-") {
    Some(rest) => rest != "0" && pvc_canonical_unsigned(rest.to_owned())
    None => pvc_canonical_unsigned(value)
  }
}

///|
fn pvc_int(value : Json, lo : Int64, hi : Int64) -> Int64 raise {
  let literal = number_literal(value)
  if !pvc_canonical_signed(literal) {
    pvc_fail("integer is not canonical")
  }
  let parsed = @string.parse_int64(literal)
  if parsed < lo || parsed > hi {
    pvc_fail("integer is out of range")
  }
  parsed
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
fn pvc_float(value : Json) -> Double raise {
  match value {
    Json::Number(number, ..) => number
    Json::Object(object) => {
      pvc_exact(object, ["$float"])
      match pvc_string(pvc_field(object, "$float")) {
        "nan" => 0.0 / 0.0
        "positive-infinity" => 1.0 / 0.0
        "negative-infinity" => -1.0 / 0.0
        _ => {
          pvc_fail("unknown exceptional float tag")
          0.0
        }
      }
    }
    _ => {
      pvc_fail("expected a float")
      0.0
    }
  }
}

///|
fn pvc_encode_float(value : Double) -> Json {
  if value.is_nan() {
    pvc_obj([("$float", Json::string("nan"))])
  } else if value.is_pos_inf() {
    pvc_obj([("$float", Json::string("positive-infinity"))])
  } else if value.is_neg_inf() {
    pvc_obj([("$float", Json::string("negative-infinity"))])
  } else {
    Json::number(value, repr=value.to_string())
  }
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
fn pvc_b64_value(c : Char) -> Int {
  if c >= 'A' && c <= 'Z' {
    c.to_int() - 'A'.to_int()
  } else if c >= 'a' && c <= 'z' {
    c.to_int() - 'a'.to_int() + 26
  } else if c >= '0' && c <= '9' {
    c.to_int() - '0'.to_int() + 52
  } else if c == '+' {
    62
  } else if c == '/' {
    63
  } else {
    -1
  }
}

///|
fn pvc_base64_decode(value : String) -> Bytes raise {
  let chars = value.to_array()
  if chars.length() % 4 != 0 {
    pvc_fail("base64 padding is missing")
  }
  let bytes : Array[Byte] = []
  for i = 0; i < chars.length(); i = i + 4 {
    let a = pvc_b64_value(chars[i])
    let b = pvc_b64_value(chars[i + 1])
    let last = i + 4 == chars.length()
    let c = if chars[i + 2] == '=' { -2 } else { pvc_b64_value(chars[i + 2]) }
    let d = if chars[i + 3] == '=' { -2 } else { pvc_b64_value(chars[i + 3]) }
    if a < 0 ||
      b < 0 ||
      c == -1 ||
      d == -1 ||
      (!last && (c < 0 || d < 0)) ||
      (c == -2 && d != -2) ||
      (c == -2 && (b & 15) != 0) ||
      (d == -2 && c >= 0 && (c & 3) != 0) {
      pvc_fail("binary bytes are not canonical padded base64")
    }
    bytes.push(((a << 2) | (b >> 4)).to_byte())
    if c >= 0 {
      bytes.push((((b & 15) << 4) | (c >> 2)).to_byte())
      if d >= 0 {
        bytes.push((((c & 3) << 6) | d).to_byte())
      }
    }
  }
  Bytes::from_array(bytes)
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
fn pvc_binary_restrictions(
  schema : Map[String, Json],
  bytes : Bytes,
  mime : String?,
) -> Unit raise {
  let restrictions = match schema.get("restrictions") {
    Some(v) => pvc_object(v)
    None => return
  }
  match restrictions.get("minBytes") {
    Some(v) =>
      if bytes.length().reinterpret_as_uint() < pvc_u32(v) {
        pvc_fail("binary is shorter than minBytes")
      }
    None => ()
  }
  match restrictions.get("maxBytes") {
    Some(v) =>
      if bytes.length().reinterpret_as_uint() > pvc_u32(v) {
        pvc_fail("binary is longer than maxBytes")
      }
    None => ()
  }
  match restrictions.get("mimeTypes") {
    Some(v) => {
      let allowed = pvc_array(v).map(pvc_string)
      match mime {
        Some(m) =>
          if !allowed.any(fn(x) { x == m }) {
            pvc_fail("binary MIME type is not allowed")
          }
        None => pvc_fail("binary MIME type is required by the schema")
      }
    }
    None => ()
  }
}

///|
fn PublicValueCodec::decode_at(
  self : PublicValueCodec,
  raw_schema : Json,
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  if depth >= 64 {
    pvc_fail("nesting exceeds 64 levels")
  }
  pvc_charge(charge, 1)
  let (kind, schema) = pvc_schema(self.resolved(raw_schema))
  match kind {
    "bool" => {
      pvc_charge(charge, 1)
      BoolValue(pvc_bool(json))
    }
    "s8" => {
      pvc_charge(charge, 1)
      let v = pvc_int(json, -128L, 127L)
      pvc_numeric_restrictions(schema, "signed", Some(v), None, None)
      S8Value(v.to_int())
    }
    "s16" => {
      pvc_charge(charge, 2)
      let v = pvc_int(json, -32768L, 32767L)
      pvc_numeric_restrictions(schema, "signed", Some(v), None, None)
      S16Value(v.to_int())
    }
    "s32" => {
      pvc_charge(charge, 4)
      let v = pvc_int(json, -2147483648L, 2147483647L)
      pvc_numeric_restrictions(schema, "signed", Some(v), None, None)
      S32Value(v.to_int())
    }
    "s64" => {
      let s = pvc_string(json)
      if !pvc_canonical_signed(s) {
        pvc_fail("s64 is not canonical")
      }
      pvc_charge(charge, 8)
      let v = @string.parse_int64(s)
      pvc_numeric_restrictions(schema, "signed", Some(v), None, None)
      S64Value(v)
    }
    "u8" => {
      pvc_charge(charge, 1)
      let v = pvc_int(json, 0L, 255L)
      pvc_numeric_restrictions(
        schema,
        "unsigned",
        None,
        Some(v.reinterpret_as_uint64()),
        None,
      )
      U8Value(v.to_int())
    }
    "u16" => {
      pvc_charge(charge, 2)
      let v = pvc_int(json, 0L, 65535L)
      pvc_numeric_restrictions(
        schema,
        "unsigned",
        None,
        Some(v.reinterpret_as_uint64()),
        None,
      )
      U16Value(v.to_int())
    }
    "u32" => {
      pvc_charge(charge, 4)
      let v = pvc_u32(json)
      pvc_numeric_restrictions(
        schema,
        "unsigned",
        None,
        Some(v.to_uint64()),
        None,
      )
      U32Value(v)
    }
    "u64" => {
      let s = pvc_string(json)
      if !pvc_canonical_unsigned(s) {
        pvc_fail("u64 is not canonical")
      }
      pvc_charge(charge, 8)
      let v = @string.parse_uint64(s)
      pvc_numeric_restrictions(schema, "unsigned", None, Some(v), None)
      U64Value(v)
    }
    "f32" => {
      let d = pvc_float(json)
      let f = Float::from_double(d)
      if d.is_inf() == false && f.is_inf() {
        pvc_fail("f32 is out of range")
      }
      pvc_charge(charge, 4)
      pvc_numeric_restrictions(schema, "float", None, None, Some(f.to_double()))
      F32Value(f)
    }
    "f64" => {
      pvc_charge(charge, 8)
      let v = pvc_float(json)
      pvc_numeric_restrictions(schema, "float", None, None, Some(v))
      F64Value(v)
    }
    "char" => {
      let s = pvc_string(json)
      if s.char_length() != 1 {
        pvc_fail("char must contain one Unicode scalar")
      }
      let c = s.get_char(0).unwrap()
      pvc_charge(charge, pvc_utf8_len(s))
      CharValue(c.to_int())
    }
    "string" => {
      let s = pvc_string(json)
      pvc_charge(charge, @utf8.encode(s).length())
      StringValue(s)
    }
    "record" => self.decode_record(schema, json, depth, charge)
    "variant" => self.decode_variant(schema, json, depth, charge)
    "enum" => {
      let name = pvc_string(json)
      let cases = pvc_array(pvc_field(schema, "cases")).map(pvc_string)
      let mut found = -1
      for i, x in cases {
        if x == name {
          found = i
        }
      }
      if found < 0 {
        pvc_fail("unknown enum case")
      }
      pvc_charge(charge, @utf8.encode(name).length())
      EnumValue(found)
    }
    "flags" => self.decode_flags(schema, json, charge)
    "tuple" => self.decode_tuple(schema, json, depth, charge)
    "list" => self.decode_list(schema, json, depth, charge, false)
    "fixed-list" => self.decode_list(schema, json, depth, charge, true)
    "map" => self.decode_map(schema, json, depth, charge)
    "option" => self.decode_option(schema, json, depth, charge)
    "result" => self.decode_result(schema, json, depth, charge)
    "text" => {
      let o = pvc_object(json)
      if o.length() < 1 || o.length() > 2 {
        pvc_fail("invalid text object")
      }
      let text = pvc_string(pvc_field(o, "text"))
      let language = match o.get("language") {
        Some(v) => {
          let s = pvc_string(v)
          if !pvc_language(s) {
            pvc_fail("language is not valid BCP-47 syntax")
          }
          Some(s)
        }
        None => None
      }
      match language {
        Some(_) => pvc_exact(o, ["text", "language"])
        None => pvc_exact(o, ["text"])
      }
      pvc_text_restrictions(schema, text, language)
      let extra = match language {
        Some(x) => @utf8.encode(x).length()
        None => 0
      }
      pvc_charge(charge, @utf8.encode(text).length() + extra)
      TextValue(text, language)
    }
    "binary" => {
      let o = pvc_object(json)
      if o.length() < 1 || o.length() > 2 {
        pvc_fail("invalid binary object")
      }
      for k, _ in o {
        if k != "bytes" && k != "mimeType" {
          pvc_fail("invalid binary member")
        }
      }
      let bytes = pvc_base64_decode(pvc_string(pvc_field(o, "bytes")))
      let mime = match o.get("mimeType") {
        Some(v) => {
          let s = pvc_string(v)
          if !pvc_mime(s) {
            pvc_fail("invalid MIME type")
          }
          Some(s)
        }
        None => None
      }
      pvc_binary_restrictions(schema, bytes, mime)
      let extra = match mime {
        Some(x) => pvc_utf8_len(x)
        None => 0
      }
      pvc_charge(charge, bytes.length() + extra)
      BinaryValue(bytes, mime)
    }
    "path" => {
      let s = pvc_string(json)
      if s.is_empty() {
        pvc_fail("path must not be empty")
      }
      pvc_path_restrictions(schema, s)
      pvc_charge(charge, @utf8.encode(s).length())
      PathValue(s)
    }
    "url" => {
      let s = pvc_string(json)
      if s.is_empty() {
        pvc_fail("URL must not be empty")
      }
      pvc_url_restrictions(schema, s)
      pvc_charge(charge, @utf8.encode(s).length())
      UrlValue(s)
    }
    "datetime" => {
      let s = pvc_string(json)
      if !pvc_datetime(s) {
        pvc_fail("datetime is not canonical RFC3339 UTC")
      }
      pvc_charge(charge, pvc_utf8_len(s))
      DatetimeValue(s)
    }
    "duration" => {
      let o = pvc_object(json)
      pvc_exact(o, ["nanoseconds"])
      let s = pvc_string(pvc_field(o, "nanoseconds"))
      if !pvc_canonical_signed(s) {
        pvc_fail("duration is not canonical")
      }
      pvc_charge(charge, 8)
      DurationValue(@string.parse_int64(s))
    }
    "quantity" => {
      let (mantissa, scale, unit) = pvc_quantity_from_json(json)
      pvc_quantity_restrictions(schema, mantissa, scale, unit)
      pvc_charge(charge, 12 + pvc_utf8_len(unit))
      QuantityValue(mantissa, scale, unit)
    }
    "union" => self.decode_union(schema, json, depth, charge)
    "stream" => self.decode_stream(schema, json, charge)
    "secret" | "quota-token" | "permission-card" | "future" => {
      pvc_fail("unsupported value kind '\{kind}'")
      BoolValue(false)
    }
    _ => {
      pvc_fail("unknown schema kind '\{kind}'")
      BoolValue(false)
    }
  }
}

///|
fn PublicValueCodec::decode_record(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  let fields = pvc_array(pvc_field(schema, "fields"))
  let object = pvc_object(json)
  pvc_collection(charge, object.length())
  if object.length() != fields.length() {
    pvc_fail("record members must exactly match schema")
  }
  let values : Array[SchemaValue] = []
  for field in fields {
    let f = pvc_object(field)
    let name = pvc_string(pvc_field(f, "name"))
    let value = match object.get(name) {
      Some(v) => v
      None => {
        pvc_fail("missing record field '\{name}'")
        Json::null()
      }
    }
    pvc_charge(charge, @utf8.encode(name).length())
    values.push(self.decode_at(pvc_field(f, "body"), value, depth + 1, charge))
  }
  for name, _ in object {
    if !fields.any(f => pvc_string(pvc_field(pvc_object(f), "name")) == name) {
      pvc_fail("unknown record field '\{name}'")
    }
  }
  RecordValue(values)
}

///|
fn PublicValueCodec::decode_variant(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  let o = pvc_object(json)
  if o.length() < 1 || o.length() > 2 {
    pvc_fail("invalid variant object")
  }
  let name = pvc_string(pvc_field(o, "$case"))
  let cases = pvc_array(pvc_field(schema, "cases"))
  let mut index = -1
  let mut selected = Json::null()
  for i, c in cases {
    let co = pvc_object(c)
    if pvc_string(pvc_field(co, "name")) == name {
      index = i
      selected = c
    }
  }
  if index < 0 {
    pvc_fail("unknown variant case")
  }
  let case = pvc_object(selected)
  let payload_schema = case.get("payload")
  let payload_json = o.get("value")
  if (payload_schema is Some(_)) != (payload_json is Some(_)) {
    pvc_fail("variant payload presence mismatch")
  }
  if payload_schema is Some(_) {
    pvc_exact(o, ["$case", "value"])
  } else {
    pvc_exact(o, ["$case"])
  }
  pvc_charge(charge, pvc_utf8_len(name))
  VariantValue(
    index,
    match (payload_schema, payload_json) {
      (Some(s), Some(v)) => Some(self.decode_at(s, v, depth + 1, charge))
      _ => None
    },
  )
}

///|
fn PublicValueCodec::decode_flags(
  _self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  charge : Ref[Int],
) -> SchemaValue raise {
  let names = pvc_array(pvc_field(schema, "flags")).map(pvc_string)
  let selected = pvc_array(json)
  pvc_collection(charge, selected.length())
  let bits = names.map(fn(_) { false })
  let mut previous = -1
  for item in selected {
    let name = pvc_string(item)
    let mut index = -1
    for i, flag in names {
      if flag == name {
        index = i
      }
    }
    if index < 0 || index <= previous {
      pvc_fail("flags must be unique and in declaration order")
    }
    bits[index] = true
    previous = index
    pvc_charge(charge, pvc_utf8_len(name))
  }
  FlagsValue(bits)
}

///|
fn PublicValueCodec::decode_tuple(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  let types = pvc_array(pvc_field(schema, "elements"))
  let values = pvc_array(json)
  pvc_collection(charge, values.length())
  if values.length() != types.length() {
    pvc_fail("tuple arity mismatch")
  }
  TupleValue(
    values.mapi((i, v) => self.decode_at(types[i], v, depth + 1, charge)),
  )
}

///|
fn PublicValueCodec::decode_list(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
  fixed : Bool,
) -> SchemaValue raise {
  let values = pvc_array(json)
  pvc_collection(charge, values.length())
  if fixed {
    let length = pvc_u32(pvc_field(schema, "length")).reinterpret_as_int()
    if values.length() != length {
      pvc_fail("fixed-list length mismatch")
    }
  }
  let decoded = values.map(v => {
    self.decode_at(pvc_field(schema, "element"), v, depth + 1, charge)
  })
  if fixed {
    FixedListValue(decoded)
  } else {
    ListValue(decoded)
  }
}

///|
fn PublicValueCodec::decode_map(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  let entries = pvc_array(json)
  pvc_collection(charge, entries.length())
  MapValue(
    entries.map(entry => {
      let pair = pvc_array(entry)
      if pair.length() != 2 {
        pvc_fail("map entry must have two elements")
      }
      {
        key: self.decode_at(
          pvc_field(schema, "key"),
          pair[0],
          depth + 1,
          charge,
        ),
        value: self.decode_at(
          pvc_field(schema, "value"),
          pair[1],
          depth + 1,
          charge,
        ),
      }
    }),
  )
}

///|
fn PublicValueCodec::decode_option(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  let o = pvc_object(json)
  let tag = pvc_string(pvc_field(o, "$option"))
  match tag {
    "none" => {
      pvc_exact(o, ["$option"])
      pvc_charge(charge, 4)
      OptionValue(None)
    }
    "some" => {
      pvc_exact(o, ["$option", "value"])
      pvc_charge(charge, 4)
      OptionValue(
        Some(
          self.decode_at(
            pvc_field(schema, "inner"),
            pvc_field(o, "value"),
            depth + 1,
            charge,
          ),
        ),
      )
    }
    _ => {
      pvc_fail("invalid option tag")
      OptionValue(None)
    }
  }
}

///|
fn PublicValueCodec::decode_result(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  let o = pvc_object(json)
  let tag = pvc_string(pvc_field(o, "$result"))
  let spec = pvc_object(pvc_field(schema, "spec"))
  let arm = match tag {
    "ok" => spec.get("ok")
    "err" => spec.get("err")
    _ => {
      pvc_fail("invalid result tag")
      None
    }
  }
  if (arm is Some(_)) != (o.get("value") is Some(_)) {
    pvc_fail("result payload presence mismatch")
  }
  if o.length() != (if arm is Some(_) { 2 } else { 1 }) {
    pvc_fail("invalid result members")
  }
  let value = match (arm, o.get("value")) {
    (Some(s), Some(v)) => Some(self.decode_at(s, v, depth + 1, charge))
    _ => None
  }
  pvc_charge(charge, pvc_utf8_len(tag))
  ResultValue(if tag == "ok" { ResultOk(value) } else { ResultErr(value) })
}

///|
fn PublicValueCodec::decode_union(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  depth : Int,
  charge : Ref[Int],
) -> SchemaValue raise {
  let o = pvc_object(json)
  pvc_exact(o, ["$union", "value"])
  let tag = pvc_string(pvc_field(o, "$union"))
  let spec = pvc_object(pvc_field(schema, "spec"))
  let branches = pvc_array(pvc_field(spec, "branches"))
  let mut body = Json::null()
  let mut found = false
  for branch in branches {
    let b = pvc_object(branch)
    if pvc_string(pvc_field(b, "tag")) == tag {
      body = pvc_field(b, "body")
      found = true
    }
  }
  if !found {
    pvc_fail("unknown union branch")
  }
  let decoded = self.decode_at(body, pvc_field(o, "value"), depth + 1, charge)
  pvc_discriminator(self, body, decoded, branches, tag)
  pvc_charge(charge, pvc_utf8_len(tag))
  UnionValue(tag, decoded)
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
fn PublicValueCodec::decode_stream(
  self : PublicValueCodec,
  schema : Map[String, Json],
  json : Json,
  charge : Ref[Int],
) -> SchemaValue raise {
  let outer = pvc_object(json)
  pvc_exact(outer, ["$stream"])
  let inner = pvc_object(pvc_field(outer, "$stream"))
  if inner.length() != 1 {
    pvc_fail("stream must have exactly one identity")
  }
  match (inner.get("provisionalRef"), inner.get("streamToken")) {
    (Some(v), None) => {
      let s = pvc_string(v)
      if !pvc_uuid_v4(s) {
        pvc_fail("invalid provisional stream reference")
      }
      self.stream_elements[s] = self.element_codec(pvc_field(schema, "inner"))
      pvc_charge(charge, 16)
      StreamReferenceValue(Some(s), None)
    }
    (None, Some(v)) => {
      let s = pvc_string(v)
      if s.is_empty() || pvc_utf8_len(s) > 8192 {
        pvc_fail("invalid stream token length")
      }
      self.stream_elements[s] = self.element_codec(pvc_field(schema, "inner"))
      pvc_charge(charge, pvc_utf8_len(s))
      StreamReferenceValue(None, Some(s))
    }
    _ => {
      pvc_fail("invalid stream identity")
      StreamReferenceValue(None, None)
    }
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
) -> Json raise {
  if depth >= 64 {
    pvc_fail("nesting exceeds 64 levels")
  }
  pvc_charge(charge, 1)
  let (kind, schema) = pvc_schema(self.resolved(raw_schema))
  match (kind, value) {
    ("bool", BoolValue(v)) => {
      pvc_charge(charge, 1)
      Json::boolean(v)
    }
    ("s8", S8Value(v)) => {
      if v < -128 || v > 127 {
        pvc_fail("s8 out of range")
      }
      pvc_numeric_restrictions(schema, "signed", Some(v.to_int64()), None, None)
      pvc_charge(charge, 1)
      json_int(v)
    }
    ("s16", S16Value(v)) => {
      if v < -32768 || v > 32767 {
        pvc_fail("s16 out of range")
      }
      pvc_numeric_restrictions(schema, "signed", Some(v.to_int64()), None, None)
      pvc_charge(charge, 2)
      json_int(v)
    }
    ("s32", S32Value(v)) => {
      pvc_numeric_restrictions(schema, "signed", Some(v.to_int64()), None, None)
      pvc_charge(charge, 4)
      json_int(v)
    }
    ("s64", S64Value(v)) => {
      pvc_numeric_restrictions(schema, "signed", Some(v), None, None)
      pvc_charge(charge, 8)
      Json::string(v.to_string())
    }
    ("u8", U8Value(v)) => {
      if v < 0 || v > 255 {
        pvc_fail("u8 out of range")
      }
      pvc_numeric_restrictions(
        schema,
        "unsigned",
        None,
        Some(v.to_uint64()),
        None,
      )
      pvc_charge(charge, 1)
      json_int(v)
    }
    ("u16", U16Value(v)) => {
      if v < 0 || v > 65535 {
        pvc_fail("u16 out of range")
      }
      pvc_numeric_restrictions(
        schema,
        "unsigned",
        None,
        Some(v.to_uint64()),
        None,
      )
      pvc_charge(charge, 2)
      json_int(v)
    }
    ("u32", U32Value(v)) => {
      pvc_numeric_restrictions(
        schema,
        "unsigned",
        None,
        Some(v.to_uint64()),
        None,
      )
      pvc_charge(charge, 4)
      json_uint(v)
    }
    ("u64", U64Value(v)) => {
      pvc_numeric_restrictions(schema, "unsigned", None, Some(v), None)
      pvc_charge(charge, 8)
      Json::string(v.to_string())
    }
    ("f32", F32Value(v)) => {
      pvc_numeric_restrictions(schema, "float", None, None, Some(v.to_double()))
      pvc_charge(charge, 4)
      pvc_encode_float(v.to_double())
    }
    ("f64", F64Value(v)) => {
      pvc_numeric_restrictions(schema, "float", None, None, Some(v))
      pvc_charge(charge, 8)
      pvc_encode_float(v)
    }
    ("char", CharValue(v)) => {
      let s = String::from_array([v.unsafe_to_char()])
      pvc_charge(charge, @utf8.encode(s).length())
      Json::string(s)
    }
    ("string", StringValue(v)) => {
      pvc_charge(charge, @utf8.encode(v).length())
      Json::string(v)
    }
    ("record", RecordValue(v)) => self.encode_record(schema, v, depth, charge)
    ("variant", VariantValue(i, v)) =>
      self.encode_variant(schema, i, v, depth, charge)
    ("enum", EnumValue(i)) => {
      let cases = pvc_array(pvc_field(schema, "cases"))
      if i < 0 || i >= cases.length() {
        pvc_fail("enum case out of range")
      }
      let name = pvc_string(cases[i])
      pvc_charge(charge, pvc_utf8_len(name))
      Json::string(name)
    }
    ("flags", FlagsValue(bits)) => {
      let names = pvc_array(pvc_field(schema, "flags"))
      if bits.length() != names.length() {
        pvc_fail("flags arity mismatch")
      }
      let out : Array[Json] = []
      for i, set in bits {
        if set {
          let name = pvc_string(names[i])
          pvc_charge(charge, pvc_utf8_len(name))
          out.push(Json::string(name))
        }
      }
      pvc_collection(charge, out.length())
      Json::array(out)
    }
    ("tuple", TupleValue(v)) =>
      self.encode_sequence(
        pvc_array(pvc_field(schema, "elements")),
        v,
        depth,
        charge,
      )
    ("list", ListValue(v)) =>
      self.encode_homogeneous(pvc_field(schema, "element"), v, depth, charge)
    ("fixed-list", FixedListValue(v)) => {
      if v.length() != pvc_u32(pvc_field(schema, "length")).reinterpret_as_int() {
        pvc_fail("fixed-list length mismatch")
      }
      self.encode_homogeneous(pvc_field(schema, "element"), v, depth, charge)
    }
    ("map", MapValue(v)) => {
      pvc_collection(charge, v.length())
      Json::array(
        v.map(e => {
          Json::array([
            self.encode_at(pvc_field(schema, "key"), e.key, depth + 1, charge),
            self.encode_at(
              pvc_field(schema, "value"),
              e.value,
              depth + 1,
              charge,
            ),
          ])
        }),
      )
    }
    ("option", OptionValue(v)) =>
      match v {
        None => {
          pvc_charge(charge, 4)
          pvc_obj([("$option", Json::string("none"))])
        }
        Some(x) => {
          pvc_charge(charge, 4)
          pvc_obj([
            ("$option", Json::string("some")),
            (
              "value",
              self.encode_at(pvc_field(schema, "inner"), x, depth + 1, charge),
            ),
          ])
        }
      }
    ("result", ResultValue(v)) => self.encode_result(schema, v, depth, charge)
    ("text", TextValue(text, language)) => {
      match language {
        Some(x) =>
          if !pvc_language(x) {
            pvc_fail("language is not valid BCP-47 syntax")
          }
        None => ()
      }
      pvc_text_restrictions(schema, text, language)
      let extra = match language {
        Some(x) => @utf8.encode(x).length()
        None => 0
      }
      pvc_charge(charge, @utf8.encode(text).length() + extra)
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
      pvc_binary_restrictions(schema, bytes, mime)
      let extra = match mime {
        Some(x) => pvc_utf8_len(x)
        None => 0
      }
      pvc_charge(charge, bytes.length() + extra)
      let f = [("bytes", Json::string(pvc_base64_encode(bytes)))]
      match mime {
        Some(x) => f.push(("mimeType", Json::string(x)))
        None => ()
      }
      pvc_obj(f)
    }
    ("path", PathValue(v)) => {
      if v.is_empty() {
        pvc_fail("path must not be empty")
      }
      pvc_path_restrictions(schema, v)
      pvc_charge(charge, pvc_utf8_len(v))
      Json::string(v)
    }
    ("url", UrlValue(v)) => {
      if v.is_empty() {
        pvc_fail("URL must not be empty")
      }
      pvc_url_restrictions(schema, v)
      pvc_charge(charge, pvc_utf8_len(v))
      Json::string(v)
    }
    ("datetime", DatetimeValue(v)) => {
      if !pvc_datetime(v) {
        pvc_fail("datetime is not canonical RFC3339 UTC")
      }
      pvc_charge(charge, pvc_utf8_len(v))
      Json::string(v)
    }
    ("duration", DurationValue(v)) => {
      pvc_charge(charge, 8)
      pvc_obj([("nanoseconds", Json::string(v.to_string()))])
    }
    ("quantity", QuantityValue(mantissa, scale, unit)) => {
      pvc_quantity_restrictions(schema, mantissa, scale, unit)
      pvc_charge(charge, 12 + pvc_utf8_len(unit))
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
      pvc_obj([
        ("$union", Json::string(tag)),
        ("value", self.encode_at(ty, body, depth + 1, charge)),
      ])
    }
    ("stream", StreamReferenceValue(provisional, token)) =>
      match (provisional, token) {
        (Some(v), None) => {
          if !pvc_uuid_v4(v) {
            pvc_fail("invalid provisional stream reference")
          }
          self.stream_elements[v] = self.element_codec(
            pvc_field(schema, "inner"),
          )
          pvc_charge(charge, 16)
          pvc_obj([("$stream", pvc_obj([("provisionalRef", Json::string(v))]))])
        }
        (None, Some(v)) => {
          if v.is_empty() || pvc_utf8_len(v) > 8192 {
            pvc_fail("invalid stream token")
          }
          self.stream_elements[v] = self.element_codec(
            pvc_field(schema, "inner"),
          )
          pvc_charge(charge, pvc_utf8_len(v))
          pvc_obj([("$stream", pvc_obj([("streamToken", Json::string(v))]))])
        }
        _ => {
          pvc_fail("stream must have exactly one identity")
          Json::null()
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
    out[name] = self.encode_at(
      pvc_field(f, "body"),
      values[i],
      depth + 1,
      charge,
    )
  }
  Json::object(out)
}

///|
fn PublicValueCodec::encode_variant(
  self : PublicValueCodec,
  schema : Map[String, Json],
  index : Int,
  payload : SchemaValue?,
  depth : Int,
  charge : Ref[Int],
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
  let fields = [("$case", Json::string(name))]
  match (ty, payload) {
    (Some(t), Some(v)) =>
      fields.push(("value", self.encode_at(t, v, depth + 1, charge)))
    _ => ()
  }
  pvc_obj(fields)
}

///|
fn PublicValueCodec::encode_sequence(
  self : PublicValueCodec,
  types : Array[Json],
  values : Array[SchemaValue],
  depth : Int,
  charge : Ref[Int],
) -> Json raise {
  if types.length() != values.length() {
    pvc_fail("tuple arity mismatch")
  }
  pvc_collection(charge, values.length())
  Json::array(
    values.mapi((i, v) => self.encode_at(types[i], v, depth + 1, charge)),
  )
}

///|
fn PublicValueCodec::encode_homogeneous(
  self : PublicValueCodec,
  ty : Json,
  values : Array[SchemaValue],
  depth : Int,
  charge : Ref[Int],
) -> Json raise {
  pvc_collection(charge, values.length())
  Json::array(values.map(v => self.encode_at(ty, v, depth + 1, charge)))
}

///|
fn PublicValueCodec::encode_result(
  self : PublicValueCodec,
  schema : Map[String, Json],
  result : SchemaResult,
  depth : Int,
  charge : Ref[Int],
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
  let fields = [("$result", Json::string(tag))]
  pvc_charge(charge, pvc_utf8_len(tag))
  match (ty, value) {
    (Some(t), Some(v)) =>
      fields.push(("value", self.encode_at(t, v, depth + 1, charge)))
    _ => ()
  }
  pvc_obj(fields)
}
