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
/// The rich `path` kind: a path in the agent's file system.
pub(all) struct Path {
  value : String
} derive(Debug, Eq, Hash)

///|
pub fn Path::new(value : String) -> Path {
  { value, }
}

///|
/// The rich `url` kind.
pub(all) struct Url {
  value : String
} derive(Debug, Eq, Hash)

///|
pub fn Url::new(value : String) -> Url {
  { value, }
}

///|
/// The rich `duration` kind: a signed number of nanoseconds.
pub(all) struct Duration {
  nanoseconds : Int64
} derive(Debug, Eq, Hash)

///|
pub fn Duration::new(nanoseconds : Int64) -> Duration {
  { nanoseconds, }
}

///|
/// The restrictions of a `text` kind, carried by the marker type of [Text].
pub(open) trait TextSpec {
  fn type_id() -> String
  fn languages() -> Array[String]
  fn min_length() -> UInt?
  fn max_length() -> UInt?
  fn regex() -> String?
}

///|
/// The rich `text` kind: a string with an optional BCP-47 language code. The
/// schema restrictions are checked when the value is encoded or decoded.
#warnings("-unused_type_variable")
pub struct Text[S] {
  value : String
  language : String?
} derive(Debug, Eq)

///|
pub fn[S] Text::new(value : String, language? : String) -> Text[S] {
  { value, language }
}

///|
pub fn[S] Text::value(self : Text[S]) -> String {
  self.value
}

///|
pub fn[S] Text::language(self : Text[S]) -> String? {
  self.language
}

///|
/// The unit of a `quantity` kind, carried by the marker type of [Quantity].
pub(open) trait QuantityUnit {
  fn type_id() -> String
  fn base_unit() -> String
  fn allowed_suffixes() -> Array[String]
}

///|
/// The rich `quantity` kind: the fixed-point value `mantissa * 10^-scale` in
/// `unit`. The unit is one of the allowed suffixes of `U`, or its base unit
/// when `U` allows no suffix.
#warnings("-unused_type_variable")
pub struct Quantity[U] {
  mantissa : Int64
  scale : Int
  unit : String
} derive(Debug, Eq)

///|
pub fn[U : QuantityUnit] Quantity::new(
  mantissa : Int64,
  scale : Int,
  unit : String,
) -> Quantity[U] raise BridgeError {
  let suffixes = U::allowed_suffixes()
  let allowed = if suffixes.is_empty() {
    unit == U::base_unit()
  } else {
    suffixes.contains(unit)
  }
  if !allowed {
    raise BridgeError(
      "Quantity unit `\{unit}` is not allowed for \{U::base_unit()}",
    )
  }
  { mantissa, scale, unit }
}

///|
pub fn as_text(v : SchemaValue) -> (String, String?) raise {
  match v {
    TextValue(text, language) => (text, language)
    o => mismatch("text", o)
  }
}

///|
pub fn as_quantity(v : SchemaValue) -> (Int64, Int, String) raise {
  match v {
    QuantityValue(mantissa, scale, unit) => (mantissa, scale, unit)
    o => mismatch("quantity", o)
  }
}
