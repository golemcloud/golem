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
/// Recursive, in-memory mirror of the server's schema-native `SchemaValue`. The
/// value tree is structurally driven by the schema: record payload order
/// matches the schema's field order, variant/enum carry a case index, union
/// carries the discriminator's literal tag. The value side does not redundantly
/// carry field names, case names, or named-ref identifiers — those come from the
/// schema.
///
/// Capability nodes (quantity, secret, quota-token) and the streaming nodes are
/// not part of the agent IO surface exercised by the bridge and are
/// intentionally omitted.
pub(all) enum SchemaValue {
  // Primitives
  BoolValue(Bool)
  S8Value(Int)
  S16Value(Int)
  S32Value(Int)
  S64Value(Int64)
  U8Value(Int)
  U16Value(Int)
  U32Value(UInt)
  U64Value(UInt64)
  F32Value(Float)
  F64Value(Double)
  CharValue(Int)
  StringValue(String)
  // Structural composites
  RecordValue(Array[SchemaValue])
  VariantValue(Int, SchemaValue?)
  EnumValue(Int)
  FlagsValue(Array[Bool])
  TupleValue(Array[SchemaValue])
  ListValue(Array[SchemaValue])
  FixedListValue(Array[SchemaValue])
  MapValue(Array[SchemaMapEntry])
  OptionValue(SchemaValue?)
  ResultValue(SchemaResult)
  // Rich semantic
  TextValue(String, String?)
  BinaryValue(Bytes, String?)
  PathValue(String)
  UrlValue(String)
  DatetimeValue(String)
  DurationValue(Int64)
  // Discriminated union
  UnionValue(String, SchemaValue)
} derive(Debug, Eq)

///|
/// An entry of a [SchemaValue::MapValue].
pub(all) struct SchemaMapEntry {
  key : SchemaValue
  value : SchemaValue
} derive(Debug, Eq)

///|
/// Result payload: exactly one of ok/err, each optionally carrying a value.
pub(all) enum SchemaResult {
  ResultOk(SchemaValue?)
  ResultErr(SchemaValue?)
} derive(Debug, Eq)
