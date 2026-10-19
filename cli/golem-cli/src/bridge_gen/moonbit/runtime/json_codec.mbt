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
// unit payload) are encoded as explicit `null` fields.

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
fn json_uint(u : UInt) -> Json {
  Json::number(u.to_double(), repr=u.to_string())
}

///|
fn json_double(d : Double) -> Json {
  if d.is_nan() {
    obj([("$float", Json::string("nan"))])
  } else if d.is_pos_inf() {
    obj([("$float", Json::string("positive-infinity"))])
  } else if d.is_neg_inf() {
    obj([("$float", Json::string("negative-infinity"))])
  } else {
    let repr = if d == 0.0 && d.reinterpret_as_uint64() == 0x8000000000000000UL {
      "-0"
    } else {
      d.to_string()
    }
    Json::number(d, repr~)
  }
}

///|
fn json_float(f : Float) -> Json {
  json_double(f.to_double())
}

///|
fn json_bool(b : Bool) -> Json {
  Json::boolean(b)
}

///|
fn node(kind : String, value : Json) -> Json {
  let m : Map[String, Json] = Map([])
  m["kind"] = Json::string(kind)
  m["value"] = value
  Json::object(m)
}

///|
fn obj(fields : Array[(String, Json)]) -> Json {
  let m : Map[String, Json] = Map([])
  for f in fields {
    m[f.0] = f.1
  }
  Json::object(m)
}

// --- Encoding ------------------------------------------------------------

///|
pub fn schema_value_render(value : SchemaValue) -> String {
  let output = StringBuilder()
  schema_value_write_json(value, output)
  output.to_string()
}

///|
fn schema_value_write_json(value : SchemaValue, output : StringBuilder) -> Unit {
  fn string(value : String) {
    output.write_string(Json::string(value).stringify())
  }
  fn node(kind : String, body : () -> Unit) {
    output <+ "{\"kind\":\"\{kind}\",\"value\":"
    body()
    output.write_char('}')
  }
  fn children(kind : String, field : String, values : Array[SchemaValue]) {
    node(kind, () => {
      output <+ "{\"\{field}\":["
      for index, value in values {
        if index != 0 {
          output.write_char(',')
        }
        schema_value_write_json(value, output)
      }
      output.write_string("]}")
    })
  }
  fn optional(value : SchemaValue?) {
    match value {
      Some(value) => schema_value_write_json(value, output)
      None => output.write_string("null")
    }
  }
  match value {
    BoolValue(v) => node("bool", () => output <+ "\{v}")
    S8Value(v) => node("s8", () => output <+ "\{v}")
    S16Value(v) => node("s16", () => output <+ "\{v}")
    S32Value(v) => node("s32", () => output <+ "\{v}")
    S64Value(v) => node("s64", () => string(v.to_string()))
    U8Value(v) => node("u8", () => output <+ "\{v}")
    U16Value(v) => node("u16", () => output <+ "\{v}")
    U32Value(v) => node("u32", () => output <+ "\{v}")
    U64Value(v) => node("u64", () => string(v.to_string()))
    F32Value(v) =>
      node("f32", () => output.write_string(json_float(v).stringify()))
    F64Value(v) =>
      node("f64", () => output.write_string(json_double(v).stringify()))
    CharValue(v) =>
      node("char", () => string(String::from_array([v.unsafe_to_char()])))
    StringValue(v) => node("string", () => string(v))
    UuidValue(v) => node("uuid", () => string(v))
    RecordValue(v) => children("record", "fields", v)
    TupleValue(v) => children("tuple", "elements", v)
    ListValue(v) => children("list", "elements", v)
    FixedListValue(v) => children("fixed-list", "elements", v)
    VariantValue(index, payload) =>
      node("variant", () => {
        output <+ "{\"case\":\{index}"
        match payload {
          Some(value) => {
            output.write_string(",\"payload\":")
            schema_value_write_json(value, output)
          }
          None => ()
        }
        output.write_char('}')
      })
    EnumValue(index) => node("enum", () => output <+ "{\"case\":\{index}}")
    FlagsValue(bits) =>
      node("flags", () => {
        output.write_string("{\"bits\":[")
        for index, bit in bits {
          if index != 0 {
            output.write_char(',')
          }
          output <+ "\{bit}"
        }
        output.write_string("]}")
      })
    MapValue(entries) =>
      node("map", () => {
        output.write_string("{\"entries\":[")
        for index, entry in entries {
          if index != 0 {
            output.write_char(',')
          }
          output.write_char('[')
          schema_value_write_json(entry.key, output)
          output.write_char(',')
          schema_value_write_json(entry.value, output)
          output.write_char(']')
        }
        output.write_string("]}")
      })
    OptionValue(inner) =>
      node("option", () => {
        output.write_string("{\"inner\":")
        optional(inner)
        output.write_char('}')
      })
    ResultValue(result) =>
      node("result", () => {
        let (tag, payload) = match result {
          ResultOk(value) => ("ok", value)
          ResultErr(value) => ("err", value)
        }
        output <+ "{\"tag\":\"\{tag}\",\"value\":"
        optional(payload)
        output.write_char('}')
      })
    TextValue(text, language) =>
      node("text", () => {
        output.write_char('{')
        match language {
          Some(language) => {
            output.write_string("\"language\":")
            string(language)
            output.write_char(',')
          }
          None => ()
        }
        output.write_string("\"text\":")
        string(text)
        output.write_char('}')
      })
    BinaryValue(bytes, mime) =>
      node("binary", () => {
        output.write_string("{\"bytes\":[")
        for index, byte in bytes {
          if index != 0 {
            output.write_char(',')
          }
          output <+ "\{byte.to_int()}"
        }
        output.write_char(']')
        match mime {
          Some(mime) => {
            output.write_string(",\"mimeType\":")
            string(mime)
          }
          None => ()
        }
        output.write_char('}')
      })
    PathValue(v) =>
      node("path", () => {
        output.write_string("{\"path\":")
        string(v)
        output.write_char('}')
      })
    UrlValue(v) =>
      node("url", () => {
        output.write_string("{\"url\":")
        string(v)
        output.write_char('}')
      })
    DatetimeValue(v) =>
      node("datetime", () => {
        output.write_string("{\"value\":")
        string(v)
        output.write_char('}')
      })
    DurationValue(v) =>
      node("duration", () => {
        output.write_string("{\"nanoseconds\":")
        string(v.to_string())
        output.write_char('}')
      })
    QuantityValue(mantissa, scale, unit) =>
      node("quantity", () => {
        output.write_string("{\"mantissa\":")
        string(mantissa.to_string())
        output <+ ",\"scale\":\{scale},\"unit\":"
        string(unit)
        output.write_char('}')
      })
    StreamReferenceValue(provisional, token) =>
      node("stream", () => {
        output.write_char('{')
        match provisional {
          Some(v) => {
            output.write_string("\"provisionalRef\":")
            string(v)
          }
          None => ()
        }
        match token {
          Some(v) => {
            if provisional is Some(_) {
              output.write_char(',')
            }
            output.write_string("\"streamToken\":")
            string(v)
          }
          None => ()
        }
        output.write_char('}')
      })
    UnionValue(tag, body) =>
      node("union", () => {
        output.write_string("{\"body\":")
        schema_value_write_json(body, output)
        output.write_string(",\"tag\":")
        string(tag)
        output.write_char('}')
      })
  }
}

///|
/// Encodes a [SchemaValue] into the server's schema-native JSON wire form.
pub fn schema_value_to_json(value : SchemaValue) -> Json {
  match value {
    BoolValue(v) => node("bool", json_bool(v))
    S8Value(v) => node("s8", json_int(v))
    S16Value(v) => node("s16", json_int(v))
    S32Value(v) => node("s32", json_int(v))
    S64Value(v) => node("s64", Json::string(v.to_string()))
    U8Value(v) => node("u8", json_int(v))
    U16Value(v) => node("u16", json_int(v))
    U32Value(v) => node("u32", json_uint(v))
    U64Value(v) => node("u64", Json::string(v.to_string()))
    F32Value(v) => node("f32", json_float(v))
    F64Value(v) => node("f64", json_double(v))
    CharValue(v) =>
      node("char", Json::string(String::from_array([v.unsafe_to_char()])))
    StringValue(v) => node("string", Json::string(v))
    UuidValue(v) => node("uuid", Json::string(v))
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
    OptionValue(inner) =>
      node(
        "option",
        obj([
          (
            "inner",
            match inner {
              Some(v) => schema_value_to_json(v)
              None => Json::null()
            },
          ),
        ]),
      )
    ResultValue(result) => {
      let fields : Array[(String, Json)] = []
      match result {
        ResultOk(v) => {
          fields.push(("tag", Json::string("ok")))
          fields.push(
            (
              "value",
              match v {
                Some(x) => schema_value_to_json(x)
                None => Json::null()
              },
            ),
          )
        }
        ResultErr(v) => {
          fields.push(("tag", Json::string("err")))
          fields.push(
            (
              "value",
              match v {
                Some(x) => schema_value_to_json(x)
                None => Json::null()
              },
            ),
          )
        }
      }
      node("result", obj(fields))
    }
    TextValue(text, language) => {
      let fields : Array[(String, Json)] = []
      match language {
        Some(l) => fields.push(("language", Json::string(l)))
        None => ()
      }
      fields.push(("text", Json::string(text)))
      node("text", obj(fields))
    }
    BinaryValue(bytes, mime_type) => {
      let arr : Array[Json] = []
      for b in bytes {
        arr.push(json_int(b.to_int()))
      }
      let fields : Array[(String, Json)] = [("bytes", Json::array(arr))]
      match mime_type {
        Some(m) => fields.push(("mimeType", Json::string(m)))
        None => ()
      }
      node("binary", obj(fields))
    }
    PathValue(v) => node("path", obj([("path", Json::string(v))]))
    UrlValue(v) => node("url", obj([("url", Json::string(v))]))
    DatetimeValue(v) => node("datetime", obj([("value", Json::string(v))]))
    DurationValue(v) =>
      node("duration", obj([("nanoseconds", Json::string(v.to_string()))]))
    QuantityValue(mantissa, scale, unit) =>
      node(
        "quantity",
        obj([
          ("mantissa", Json::string(mantissa.to_string())),
          ("scale", json_int(scale)),
          ("unit", Json::string(unit)),
        ]),
      )
    StreamReferenceValue(provisional_ref, stream_token) => {
      let fields : Array[(String, Json)] = []
      match provisional_ref {
        Some(v) => fields.push(("provisionalRef", Json::string(v)))
        None => ()
      }
      match stream_token {
        Some(v) => fields.push(("streamToken", Json::string(v)))
        None => ()
      }
      node("stream", obj(fields))
    }
    UnionValue(tag, body) =>
      node(
        "union",
        obj([("body", schema_value_to_json(body)), ("tag", Json::string(tag))]),
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
    _ =>
      raise BridgeError("Expected a JSON boolean, got \{@debug.to_string(j)}")
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
  exact_fields(outer, ["kind", "value"])
  let kind = expect_string(get_field(outer, "kind"))
  let value = get_field(outer, "value")
  match kind {
    "bool" => BoolValue(expect_bool(value))
    "s8" => S8Value(parse_int_range(value, -128, 127, "s8"))
    "s16" => S16Value(parse_int_range(value, -32768, 32767, "s16"))
    "s32" => S32Value(@string.parse_int(number_literal(value)))
    "s64" => S64Value(parse_canonical_s64(value, "s64"))
    "u8" => U8Value(parse_int_range(value, 0, 255, "u8"))
    "u16" => U16Value(parse_int_range(value, 0, 65535, "u16"))
    "u32" => U32Value(@string.parse_uint(number_literal(value)))
    "u64" => U64Value(parse_canonical_u64_value(value, "u64"))
    "f32" => {
      let decoded = parse_schema_float(value)
      let narrowed = Float::from_double(decoded)
      if value is Json::Number(_, ..) &&
        (decoded.is_nan() || decoded.is_inf() || narrowed.is_inf()) {
        raise BridgeError("finite f32 is out of range")
      }
      F32Value(narrowed)
    }
    "f64" => {
      let decoded = parse_schema_float(value)
      if value is Json::Number(_, ..) && (decoded.is_nan() || decoded.is_inf()) {
        raise BridgeError("finite f64 is out of range")
      }
      F64Value(decoded)
    }
    "char" => {
      let s = expect_string(value)
      if s.char_length() != 1 {
        raise BridgeError(
          "Expected a single-character string for char, got '\{s}'",
        )
      }
      CharValue(s.get_char(0).unwrap().to_int())
    }
    "string" => StringValue(expect_string(value))
    "uuid" => UuidValue(expect_string(value))
    "record" => {
      let m = expect_object(value)
      exact_fields(m, ["fields"])
      let fields = expect_array(get_field(m, "fields"))
      RecordValue(fields.map(schema_value_from_json))
    }
    "variant" => {
      let m = expect_object(value)
      match m.get("payload") {
        Some(_) => exact_fields(m, ["case", "payload"])
        None => exact_fields(m, ["case"])
      }
      let case_index = @string.parse_int(number_literal(get_field(m, "case")))
      let payload = match m.get("payload") {
        Some(p) => Some(schema_value_from_json(p))
        None => None
      }
      VariantValue(case_index, payload)
    }
    "enum" => {
      let m = expect_object(value)
      exact_fields(m, ["case"])
      EnumValue(@string.parse_int(number_literal(get_field(m, "case"))))
    }
    "flags" => {
      let m = expect_object(value)
      exact_fields(m, ["bits"])
      let bits = expect_array(get_field(m, "bits"))
      FlagsValue(bits.map(expect_bool))
    }
    "tuple" => {
      let m = expect_object(value)
      exact_fields(m, ["elements"])
      let elements = expect_array(get_field(m, "elements"))
      TupleValue(elements.map(schema_value_from_json))
    }
    "list" => {
      let m = expect_object(value)
      exact_fields(m, ["elements"])
      let elements = expect_array(get_field(m, "elements"))
      ListValue(elements.map(schema_value_from_json))
    }
    "fixed-list" => {
      let m = expect_object(value)
      exact_fields(m, ["elements"])
      let elements = expect_array(get_field(m, "elements"))
      FixedListValue(elements.map(schema_value_from_json))
    }
    "map" => {
      let m = expect_object(value)
      exact_fields(m, ["entries"])
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
      exact_fields(m, ["inner"])
      match field_opt(m, "inner") {
        Some(inner) => OptionValue(Some(schema_value_from_json(inner)))
        None => OptionValue(None)
      }
    }
    "result" => {
      let m = expect_object(value)
      exact_fields(m, ["tag", "value"])
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
      match m.get("language") {
        Some(_) => exact_fields(m, ["text", "language"])
        None => exact_fields(m, ["text"])
      }
      let text = expect_string(get_field(m, "text"))
      let language = match m.get("language") {
        Some(l) => Some(expect_string(l))
        None => None
      }
      TextValue(text, language)
    }
    "binary" => {
      let m = expect_object(value)
      match m.get("mimeType") {
        Some(_) => exact_fields(m, ["bytes", "mimeType"])
        None => exact_fields(m, ["bytes"])
      }
      let arr = expect_array(get_field(m, "bytes"))
      let buf = Buffer()
      for b in arr {
        buf.write_byte(parse_int_range(b, 0, 255, "byte").to_byte())
      }
      let mime = match m.get("mimeType") {
        Some(x) => Some(expect_string(x))
        None => None
      }
      BinaryValue(buf.to_bytes(), mime)
    }
    "path" => {
      let m = expect_object(value)
      exact_fields(m, ["path"])
      PathValue(expect_string(get_field(m, "path")))
    }
    "url" => {
      let m = expect_object(value)
      exact_fields(m, ["url"])
      UrlValue(expect_string(get_field(m, "url")))
    }
    "datetime" => {
      let m = expect_object(value)
      exact_fields(m, ["value"])
      DatetimeValue(expect_string(get_field(m, "value")))
    }
    "duration" => {
      let m = expect_object(value)
      exact_fields(m, ["nanoseconds"])
      DurationValue(
        parse_canonical_s64(get_field(m, "nanoseconds"), "duration"),
      )
    }
    "quantity" => {
      let m = expect_object(value)
      exact_fields(m, ["mantissa", "scale", "unit"])
      QuantityValue(
        parse_canonical_s64(get_field(m, "mantissa"), "quantity mantissa"),
        @string.parse_int(number_literal(get_field(m, "scale"))),
        expect_string(get_field(m, "unit")),
      )
    }
    "union" => {
      let m = expect_object(value)
      exact_fields(m, ["tag", "body"])
      let tag = expect_string(get_field(m, "tag"))
      UnionValue(tag, schema_value_from_json(get_field(m, "body")))
    }
    "stream" => {
      let m = expect_object(value)
      if m.length() != 1 {
        raise BridgeError(
          "Stream reference must contain exactly one reference field",
        )
      }
      match (m.get("provisionalRef"), m.get("streamToken")) {
        (Some(v), None) => StreamReferenceValue(Some(expect_string(v)), None)
        (None, Some(v)) => StreamReferenceValue(None, Some(expect_string(v)))
        _ =>
          raise BridgeError(
            "Stream reference must contain exactly one reference field",
          )
      }
    }
    other => raise BridgeError("Unknown SchemaValue kind '\{other}'")
  }
}

///|
fn exact_fields(
  object : Map[String, Json],
  fields : Array[String],
) -> Unit raise {
  if object.length() != fields.length() {
    raise BridgeError(
      "JSON object members do not match the expected representation",
    )
  }
  for name, _ in object {
    if !fields.any(fn(field) { field == name }) {
      raise BridgeError(
        "JSON object members do not match the expected representation",
      )
    }
  }
}

///|
fn canonical_unsigned(value : String) -> Bool {
  !value.is_empty() &&
  (value == "0" || value[0] != '0') &&
  value.all(fn(c) { c >= '0' && c <= '9' })
}

///|
fn canonical_signed(value : String) -> Bool {
  match value.strip_prefix("-") {
    Some(rest) => rest != "0" && canonical_unsigned(rest.to_owned())
    None => canonical_unsigned(value)
  }
}

///|
fn parse_canonical_s64(value : Json, context : String) -> Int64 raise {
  let text = expect_string(value)
  if !canonical_signed(text) {
    raise BridgeError("\{context} must be a canonical decimal string")
  }
  @string.parse_int64(text)
}

///|
fn parse_canonical_u64_value(value : Json, context : String) -> UInt64 raise {
  let text = expect_string(value)
  if !canonical_unsigned(text) {
    raise BridgeError("\{context} must be a canonical decimal string")
  }
  @string.parse_uint64(text)
}

///|
fn parse_schema_float(value : Json) -> Double raise {
  match value {
    Json::Number(number, ..) => number
    Json::Object(object) => {
      exact_fields(object, ["$float"])
      match expect_string(get_field(object, "$float")) {
        "nan" => 0.0 / 0.0
        "positive-infinity" => 1.0 / 0.0
        "negative-infinity" => -1.0 / 0.0
        _ => raise BridgeError("Unknown exceptional float representation")
      }
    }
    _ => raise BridgeError("Expected a JSON number or exceptional float")
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
