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

// Typed accessors used by the generated per-type codecs. Each extracts the
// mapped MoonBit value from a [SchemaValue] node, raising [BridgeError] on a
// wire-shape mismatch. They centralise the structural navigation so the
// generated codecs stay thin.

///|
fn[T] mismatch(expected : String, got : SchemaValue) -> T raise {
  raise BridgeError("Expected a \{expected} value, got \{@debug.to_string(got)}")
}

///|
pub fn as_bool(v : SchemaValue) -> Bool raise {
  match v {
    BoolValue(x) => x
    o => mismatch("bool", o)
  }
}

///|
pub fn as_s8(v : SchemaValue) -> Int raise {
  match v {
    S8Value(x) => x
    o => mismatch("s8", o)
  }
}

///|
pub fn as_s16(v : SchemaValue) -> Int raise {
  match v {
    S16Value(x) => x
    o => mismatch("s16", o)
  }
}

///|
pub fn as_s32(v : SchemaValue) -> Int raise {
  match v {
    S32Value(x) => x
    o => mismatch("s32", o)
  }
}

///|
pub fn as_s64(v : SchemaValue) -> Int64 raise {
  match v {
    S64Value(x) => x
    o => mismatch("s64", o)
  }
}

///|
pub fn as_u8(v : SchemaValue) -> Byte raise {
  match v {
    U8Value(x) => x.to_byte()
    o => mismatch("u8", o)
  }
}

///|
pub fn as_u16(v : SchemaValue) -> Int raise {
  match v {
    U16Value(x) => x
    o => mismatch("u16", o)
  }
}

///|
pub fn as_u32(v : SchemaValue) -> UInt raise {
  match v {
    U32Value(x) => x
    o => mismatch("u32", o)
  }
}

///|
pub fn as_u64(v : SchemaValue) -> UInt64 raise {
  match v {
    U64Value(x) => x
    o => mismatch("u64", o)
  }
}

///|
pub fn as_f32(v : SchemaValue) -> Float raise {
  match v {
    F32Value(x) => x
    o => mismatch("f32", o)
  }
}

///|
pub fn as_f64(v : SchemaValue) -> Double raise {
  match v {
    F64Value(x) => x
    o => mismatch("f64", o)
  }
}

///|
pub fn as_char(v : SchemaValue) -> Char raise {
  match v {
    CharValue(x) => x.unsafe_to_char()
    o => mismatch("char", o)
  }
}

///|
pub fn as_string(v : SchemaValue) -> String raise {
  match v {
    StringValue(x) => x
    o => mismatch("string", o)
  }
}

///|
pub fn as_path(v : SchemaValue) -> String raise {
  match v {
    PathValue(x) => x
    o => mismatch("path", o)
  }
}

///|
pub fn as_url(v : SchemaValue) -> String raise {
  match v {
    UrlValue(x) => x
    o => mismatch("url", o)
  }
}

///|
pub fn as_datetime(v : SchemaValue) -> String raise {
  match v {
    DatetimeValue(x) => x
    o => mismatch("datetime", o)
  }
}

///|
pub fn as_duration(v : SchemaValue) -> Int64 raise {
  match v {
    DurationValue(x) => x
    o => mismatch("duration", o)
  }
}

///|
pub fn as_record(v : SchemaValue) -> Array[SchemaValue] raise {
  match v {
    RecordValue(fields) => fields
    o => mismatch("record", o)
  }
}

///|
pub fn as_variant(v : SchemaValue) -> (Int, SchemaValue?) raise {
  match v {
    VariantValue(case_index, payload) => (case_index, payload)
    o => mismatch("variant", o)
  }
}

///|
pub fn as_enum(v : SchemaValue) -> Int raise {
  match v {
    EnumValue(case_index) => case_index
    o => mismatch("enum", o)
  }
}

///|
pub fn as_flags(v : SchemaValue) -> Array[Bool] raise {
  match v {
    FlagsValue(bits) => bits
    o => mismatch("flags", o)
  }
}

///|
pub fn as_tuple(v : SchemaValue) -> Array[SchemaValue] raise {
  match v {
    TupleValue(elements) => elements
    o => mismatch("tuple", o)
  }
}

///|
pub fn as_list(v : SchemaValue) -> Array[SchemaValue] raise {
  match v {
    ListValue(elements) => elements
    o => mismatch("list", o)
  }
}

///|
pub fn as_fixed_list(v : SchemaValue) -> Array[SchemaValue] raise {
  match v {
    FixedListValue(elements) => elements
    o => mismatch("fixed-list", o)
  }
}

///|
pub fn as_map(v : SchemaValue) -> Array[SchemaMapEntry] raise {
  match v {
    MapValue(entries) => entries
    o => mismatch("map", o)
  }
}

///|
pub fn as_option(v : SchemaValue) -> SchemaValue? raise {
  match v {
    OptionValue(inner) => inner
    o => mismatch("option", o)
  }
}

///|
pub fn as_result(v : SchemaValue) -> SchemaResult raise {
  match v {
    ResultValue(r) => r
    o => mismatch("result", o)
  }
}

///|
pub fn as_union(v : SchemaValue) -> (String, SchemaValue) raise {
  match v {
    UnionValue(tag, body) => (tag, body)
    o => mismatch("union", o)
  }
}

///|
/// Reads the payload of a non-unit variant case, raising if it is absent.
pub fn variant_payload(
  case_index : Int,
  payload : SchemaValue?,
) -> SchemaValue raise {
  match payload {
    Some(p) => p
    None =>
      raise BridgeError("Variant case \{case_index} is missing its payload")
  }
}

///|
/// Reads a required payload, raising a context-tagged [BridgeError] if absent.
/// Used by the generated codecs for non-unit `result` arms.
pub fn required_payload(
  payload : SchemaValue?,
  context : String,
) -> SchemaValue raise {
  match payload {
    Some(p) => p
    None => raise BridgeError("Missing payload for \{context}")
  }
}
