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
let rng : @random.Rand = @random.Rand::chacha8(
  seed=int64_to_bytes(@async.now()),
)

///|
fn int64_to_bytes(v : Int64) -> Bytes {
  let buf = Buffer()
  for round in 0..<4 {
    let mut x = v ^ (round.to_int64() << 32)
    for _ in 0..<8 {
      buf.write_byte((x & 0xFFL).to_byte())
      x = x >> 8
    }
  }
  buf.to_bytes()
}

///|
let hex_digits : String = "0123456789abcdef"

///|
/// A universally unique identifier (RFC 9562), stored as its 128 bits. `Show`
/// and `Uuid::to_string` print the lowercase hyphenated form.
pub struct Uuid {
  priv high_bits : UInt64
  priv low_bits : UInt64
} derive(Eq, Compare, Hash)

///|
pub fn Uuid::from_bits(high_bits : UInt64, low_bits : UInt64) -> Uuid {
  { high_bits, low_bits, }
}

///|
pub fn Uuid::high_bits(self : Uuid) -> UInt64 {
  self.high_bits
}

///|
pub fn Uuid::low_bits(self : Uuid) -> UInt64 {
  self.low_bits
}

///|
/// Generates a random (version 4) UUID.
pub fn Uuid::random() -> Uuid {
  let high = rng.uint64()
  let low = rng.uint64()
  {
    high_bits: (high & 0xFFFFFFFFFFFF0FFFUL) | 0x0000000000004000UL,
    low_bits: (low & 0x3FFFFFFFFFFFFFFFUL) | 0x8000000000000000UL,
  }
}

///|
fn uuid_hex_digit(c : UInt16) -> UInt64? {
  let c = c.to_int()
  if c >= '0' && c <= '9' {
    Some((c - '0').to_uint64())
  } else if c >= 'a' && c <= 'f' {
    Some((c - 'a' + 10).to_uint64())
  } else if c >= 'A' && c <= 'F' {
    Some((c - 'A' + 10).to_uint64())
  } else {
    None
  }
}

///|
/// Parses the hyphenated form `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`. Hex
/// digits may be upper or lower case.
pub fn Uuid::parse(value : String) -> Uuid raise BridgeError {
  if value.length() != 36 {
    raise BridgeError("Invalid UUID '\{value}': expected 36 characters")
  }
  let mut high = 0UL
  let mut low = 0UL
  let mut digits = 0
  for i in 0..<36 {
    let c = value.code_unit_at(i)
    if i == 8 || i == 13 || i == 18 || i == 23 {
      if c.to_int() != '-' {
        raise BridgeError("Invalid UUID '\{value}': expected '-' at \{i}")
      }
      continue
    }
    guard uuid_hex_digit(c) is Some(digit) else {
      raise BridgeError("Invalid UUID '\{value}': invalid hex digit at \{i}")
    }
    if digits < 16 {
      high = (high << 4) | digit
    } else {
      low = (low << 4) | digit
    }
    digits += 1
  }
  { high_bits: high, low_bits: low, }
}

///|
/// Parses the canonical lowercase hyphenated form that the server emits.
fn Uuid::parse_canonical(value : String) -> Uuid raise BridgeError {
  let uuid = Uuid::parse(value)
  if uuid.to_string() != value {
    raise BridgeError("Invalid UUID '\{value}': expected lowercase hex digits")
  }
  uuid
}

///|
fn uuid_write_hex(logger : &Logger, value : UInt64, digits : Int) -> Unit {
  for i = digits - 1; i >= 0; i = i - 1 {
    let nibble = ((value >> (i * 4)) & 0xFUL).to_int()
    logger.write_char(hex_digits.get_char(nibble).unwrap())
  }
}

///|
pub impl Show for Uuid with fn output(self, logger) {
  uuid_write_hex(logger, self.high_bits >> 32, 8)
  logger.write_char('-')
  uuid_write_hex(logger, self.high_bits >> 16, 4)
  logger.write_char('-')
  uuid_write_hex(logger, self.high_bits, 4)
  logger.write_char('-')
  uuid_write_hex(logger, self.low_bits >> 48, 4)
  logger.write_char('-')
  uuid_write_hex(logger, self.low_bits, 12)
}

///|
pub impl Debug for Uuid with fn to_repr(self) {
  Repr::ctor("Uuid", [(None, Repr::string(self.to_string()))])
}
