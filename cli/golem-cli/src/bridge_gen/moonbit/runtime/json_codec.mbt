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

// Codec between [SchemaValue] and the server's schema-native JSON wire form.
//
// The wire form mirrors the server's Rust `SchemaValue` serde derive with
// `tag = "kind"` / `content = "value"` and kebab-cased discriminants. Composite
// payloads are positional and driven by the schema (records carry no field
// names, variants carry a `case` index, etc.).
//
// Empty optional payloads (`option` with no inner value, `result` ok/err with a
// unit payload) are encoded by omitting the corresponding field. The server
// treats a missing field and an explicit `null` identically, so decoding
// accepts both.

// --- JSON number helpers -------------------------------------------------
//
// MoonBit's core `Json::Number` is `Double`-backed. To preserve s64/u64
// precision on the wire we attach the exact decimal `repr` so `stringify`
// emits the raw integer token instead of a lossy double.

///|
fn json_int(i : Int) -> Json {
  Json::number(i.to_double(), repr=i.to_string())
}

///|
fn json_int64(i : Int64) -> Json {
  Json::number(i.to_double(), repr=i.to_string())
}

///|
fn json_uint(u : UInt) -> Json {
  Json::number(u.to_double(), repr=u.to_string())
}

///|
fn json_uint64(u : UInt64) -> Json {
  Json::number(u.to_double(), repr=u.to_string())
}

///|
fn json_double(d : Double) -> Json {
  Json::number(d)
}

///|
fn json_float(f : Float) -> Json {
  Json::number(f.to_double(), repr=f.to_string())
}

///|
fn json_bool(b : Bool) -> Json {
  Json::boolean(b)
}

///|
fn node(kind : String, value : Json) -> Json {
  let m : Map[String, Json] = {}
  m["kind"] = Json::string(kind)
  m["value"] = value
  Json::object(m)
}

///|
fn obj(fields : Array[(String, Json)]) -> Json {
  let m : Map[String, Json] = {}
  for f in fields {
    m[f.0] = f.1
  }
  Json::object(m)
}

// --- Encoding ------------------------------------------------------------

///|
/// Encodes a [SchemaValue] into the server's schema-native JSON wire form.
pub fn schema_value_to_json(value : SchemaValue) -> Json {
  match value {
    BoolValue(v) => node("bool", json_bool(v))
    S8Value(v) => node("s8", json_int(v))
    S16Value(v) => node("s16", json_int(v))
    S32Value(v) => node("s32", json_int(v))
    S64Value(v) => node("s64", json_int64(v))
    U8Value(v) => node("u8", json_int(v))
    U16Value(v) => node("u16", json_int(v))
    U32Value(v) => node("u32", json_uint(v))
    U64Value(v) => node("u64", json_uint64(v))
    F32Value(v) => node("f32", json_float(v))
    F64Value(v) => node("f64", json_double(v))
    CharValue(v) => node("char", Json::string(String::from_array([v.unsafe_to_char()])))
    StringValue(v) => node("string", Json::string(v))
    RecordValue(fields) =>
      node(
        "record",
        obj([("fields", Json::array(fields.map(schema_value_to_json)))]),
      )
    VariantValue(case_index, payload) => {
      let fields : Array[(String, Json)] = [("case", json_int(case_index))]
      match payload {
        Some(p) => fields.push(("payload", schema_value_to_json(p)))
        None => ()
      }
      node("variant", obj(fields))
    }
    EnumValue(case_index) => node("enum", obj([("case", json_int(case_index))]))
    FlagsValue(flags) =>
      node("flags", obj([("bits", Json::array(flags.map(json_bool)))]))
    TupleValue(elements) =>
      node(
        "tuple",
        obj([("elements", Json::array(elements.map(schema_value_to_json)))]),
      )
    ListValue(elements) =>
      node(
        "list",
        obj([("elements", Json::array(elements.map(schema_value_to_json)))]),
      )
    FixedListValue(elements) =>
      node(
        "fixed-list",
        obj([("elements", Json::array(elements.map(schema_value_to_json)))]),
      )
    MapValue(entries) => {
      let encoded : Array[Json] = entries.map(fn(entry) {
        Json::array([
          schema_value_to_json(entry.key),
          schema_value_to_json(entry.value),
        ])
      })
      node("map", obj([("entries", Json::array(encoded))]))
    }
    OptionValue(inner) => {
      let fields : Array[(String, Json)] = []
      match inner {
        Some(v) => fields.push(("inner", schema_value_to_json(v)))
        None => ()
      }
      node("option", obj(fields))
    }
    ResultValue(result) => {
      let fields : Array[(String, Json)] = []
      match result {
        ResultOk(v) => {
          fields.push(("tag", Json::string("ok")))
          match v {
            Some(x) => fields.push(("value", schema_value_to_json(x)))
            None => ()
          }
        }
        ResultErr(v) => {
          fields.push(("tag", Json::string("err")))
          match v {
            Some(x) => fields.push(("value", schema_value_to_json(x)))
            None => ()
          }
        }
      }
      node("result", obj(fields))
    }
    TextValue(text, language) => {
      let fields : Array[(String, Json)] = [("text", Json::string(text))]
      match language {
        Some(l) => fields.push(("language", Json::string(l)))
        None => ()
      }
      node("text", obj(fields))
    }
    BinaryValue(bytes, mime_type) => {
      let arr : Array[Json] = []
      for b in bytes {
        arr.push(json_int(b.to_int()))
      }
      let fields : Array[(String, Json)] = [("bytes", Json::array(arr))]
      match mime_type {
        Some(m) => fields.push(("mime_type", Json::string(m)))
        None => ()
      }
      node("binary", obj(fields))
    }
    PathValue(v) => node("path", obj([("path", Json::string(v))]))
    UrlValue(v) => node("url", obj([("url", Json::string(v))]))
    DatetimeValue(v) => node("datetime", obj([("value", Json::string(v))]))
    DurationValue(v) =>
      node("duration", obj([("nanoseconds", json_int64(v))]))
    UnionValue(tag, body) =>
      node(
        "union",
        obj([("tag", Json::string(tag)), ("body", schema_value_to_json(body))]),
      )
  }
}

// --- JSON access helpers -------------------------------------------------

///|
fn expect_object(j : Json) -> Map[String, Json] raise {
  match j {
    Json::Object(m) => m
    _ => raise BridgeError("Expected a JSON object, got \{@debug.to_string(j)}")
  }
}

///|
fn get_field(m : Map[String, Json], name : String) -> Json raise {
  match m.get(name) {
    Some(v) => v
    None => raise BridgeError("Missing JSON field '\{name}'")
  }
}

///|
/// Returns the field if present and not `null`, otherwise `None`.
fn field_opt(m : Map[String, Json], name : String) -> Json? {
  match m.get(name) {
    Some(Json::Null) => None
    Some(v) => Some(v)
    None => None
  }
}

///|
fn expect_string(j : Json) -> String raise {
  match j {
    Json::String(s) => s
    _ => raise BridgeError("Expected a JSON string, got \{@debug.to_string(j)}")
  }
}

///|
fn expect_bool(j : Json) -> Bool raise {
  match j {
    Json::True => true
    Json::False => false
    _ => raise BridgeError("Expected a JSON boolean, got \{@debug.to_string(j)}")
  }
}

///|
fn expect_array(j : Json) -> Array[Json] raise {
  match j {
    Json::Array(a) => a
    _ => raise BridgeError("Expected a JSON array, got \{@debug.to_string(j)}")
  }
}

///|
/// Extracts the exact decimal literal of a JSON number, preserving full
/// integer precision even for values outside the safe `Double` range.
pub fn number_literal(j : Json) -> String raise {
  match j {
    Json::Number(_, repr=Some(r)) => r
    Json::Number(d, repr=None) => double_to_literal(d)
    _ => raise BridgeError("Expected a JSON number, got \{@debug.to_string(j)}")
  }
}

///|
fn double_to_literal(d : Double) -> String {
  if d == d.trunc() && d.abs() < 9.0e15 {
    d.to_int64().to_string()
  } else {
    d.to_string()
  }
}

// --- Decoding ------------------------------------------------------------

///|
/// Decodes a [SchemaValue] from the server's schema-native JSON wire form.
pub fn schema_value_from_json(j : Json) -> SchemaValue raise {
  let outer = expect_object(j)
  let kind = expect_string(get_field(outer, "kind"))
  let value = get_field(outer, "value")
  match kind {
    "bool" => BoolValue(expect_bool(value))
    "s8" => S8Value(parse_int_range(value, -128, 127, "s8"))
    "s16" => S16Value(parse_int_range(value, -32768, 32767, "s16"))
    "s32" => S32Value(@string.parse_int(number_literal(value)))
    "s64" => S64Value(@string.parse_int64(number_literal(value)))
    "u8" => U8Value(parse_int_range(value, 0, 255, "u8"))
    "u16" => U16Value(parse_int_range(value, 0, 65535, "u16"))
    "u32" => U32Value(@string.parse_uint(number_literal(value)))
    "u64" => U64Value(@string.parse_uint64(number_literal(value)))
    "f32" => F32Value(Float::from_double(@string.parse_double(number_literal(value))))
    "f64" => F64Value(@string.parse_double(number_literal(value)))
    "char" => {
      let s = expect_string(value)
      if s.char_length() != 1 {
        raise BridgeError("Expected a single-character string for char, got '\{s}'")
      }
      CharValue(s.get_char(0).unwrap().to_int())
    }
    "string" => StringValue(expect_string(value))
    "record" => {
      let m = expect_object(value)
      let fields = expect_array(get_field(m, "fields"))
      RecordValue(fields.map(schema_value_from_json))
    }
    "variant" => {
      let m = expect_object(value)
      let case_index = @string.parse_int(number_literal(get_field(m, "case")))
      let payload = match field_opt(m, "payload") {
        Some(p) => Some(schema_value_from_json(p))
        None => None
      }
      VariantValue(case_index, payload)
    }
    "enum" => {
      let m = expect_object(value)
      EnumValue(@string.parse_int(number_literal(get_field(m, "case"))))
    }
    "flags" => {
      let m = expect_object(value)
      let bits = expect_array(get_field(m, "bits"))
      FlagsValue(bits.map(expect_bool))
    }
    "tuple" => {
      let m = expect_object(value)
      let elements = expect_array(get_field(m, "elements"))
      TupleValue(elements.map(schema_value_from_json))
    }
    "list" => {
      let m = expect_object(value)
      let elements = expect_array(get_field(m, "elements"))
      ListValue(elements.map(schema_value_from_json))
    }
    "fixed-list" => {
      let m = expect_object(value)
      let elements = expect_array(get_field(m, "elements"))
      FixedListValue(elements.map(schema_value_from_json))
    }
    "map" => {
      let m = expect_object(value)
      let entries = expect_array(get_field(m, "entries"))
      let decoded : Array[SchemaMapEntry] = []
      for entry in entries {
        let pair = expect_array(entry)
        if pair.length() != 2 {
          raise BridgeError("Expected a [key, value] pair in map entry")
        }
        decoded.push({
          key: schema_value_from_json(pair[0]),
          value: schema_value_from_json(pair[1]),
        })
      }
      MapValue(decoded)
    }
    "option" => {
      let m = expect_object(value)
      match field_opt(m, "inner") {
        Some(inner) => OptionValue(Some(schema_value_from_json(inner)))
        None => OptionValue(None)
      }
    }
    "result" => {
      let m = expect_object(value)
      let tag = expect_string(get_field(m, "tag"))
      let inner = match field_opt(m, "value") {
        Some(v) => Some(schema_value_from_json(v))
        None => None
      }
      match tag {
        "ok" => ResultValue(ResultOk(inner))
        "err" => ResultValue(ResultErr(inner))
        other => raise BridgeError("Unknown result tag '\{other}'")
      }
    }
    "text" => {
      let m = expect_object(value)
      let text = expect_string(get_field(m, "text"))
      let language = match field_opt(m, "language") {
        Some(l) => Some(expect_string(l))
        None => None
      }
      TextValue(text, language)
    }
    "binary" => {
      let m = expect_object(value)
      let arr = expect_array(get_field(m, "bytes"))
      let buf = @buffer.new()
      for b in arr {
        buf.write_byte(parse_int_range(b, 0, 255, "byte").to_byte())
      }
      let mime = match field_opt(m, "mime_type") {
        Some(x) => Some(expect_string(x))
        None => None
      }
      BinaryValue(buf.to_bytes(), mime)
    }
    "path" => {
      let m = expect_object(value)
      PathValue(expect_string(get_field(m, "path")))
    }
    "url" => {
      let m = expect_object(value)
      UrlValue(expect_string(get_field(m, "url")))
    }
    "datetime" => {
      let m = expect_object(value)
      DatetimeValue(expect_string(get_field(m, "value")))
    }
    "duration" => {
      let m = expect_object(value)
      DurationValue(@string.parse_int64(number_literal(get_field(m, "nanoseconds"))))
    }
    "union" => {
      let m = expect_object(value)
      let tag = expect_string(get_field(m, "tag"))
      UnionValue(tag, schema_value_from_json(get_field(m, "body")))
    }
    other => raise BridgeError("Unknown SchemaValue kind '\{other}'")
  }
}

///|
fn parse_int_range(j : Json, lo : Int, hi : Int, kind : String) -> Int raise {
  let n = @string.parse_int(number_literal(j))
  if n < lo || n > hi {
    raise BridgeError("\{kind} value \{n} out of range [\{lo}, \{hi}]")
  }
  n
}
