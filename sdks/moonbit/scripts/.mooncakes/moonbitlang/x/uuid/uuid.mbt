// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
/// UUID that complies with RFC 4122.
///
/// UUID objects are immutable, hashable and usable as mapping keys.
/// The Display of UUIDs is like `12345678-1234-1234-1234-123456789abc`;
/// UUIDs can be constructed using such strings, or 16-bytes strings with big-
/// endian byte order.
///
/// # Usage
///
/// ```moonbit check
/// test {
///   let u1 = @uuid.from_hex("ddf99703-742f-7505-4c54-df36a9c243fe")
///   let u2 = @uuid.from_bytes(
///     Bytes::from_array([
///       221, 249, 151, 3, 116, 47, 117, 5, 76, 84, 223, 54, 169, 194, 67, 254,
///     ]),
///   )
///   assert_eq(u1, u2)
/// }
/// ```
///
/// Construct an RFC-4122-compliant v4 UUID:
///
/// ```moonbit check
/// test {
///   let _u = @uuid.from_hex("9558cfadb48547c70f9adf1f75cdef29").as_version(
///     @uuid.V4,
///   )
///
/// }
/// ```
///
struct UUID {
  // XXX: replace with an 128-bit data structure when there is one
  // use the derived hash
  hi : Int64
  lo : Int64
} derive(Eq, Compare)

///|
/// Constructs a UUID with the given big-endian bytes.
///
/// The produced UUID is not RFC-4122 compliant; see also `as_version()`.
///
pub fn from_bytes(bytes : Bytes) -> UUID raise Error {
  if bytes.length() != 16 {
    fail("requires 16 bytes")
  }
  // XXX: this is ideally Int64::from_bytes() (needs a cheap slicing method)
  let hi = for rv = 0L, i = 0; i < 8; {
    continue rv | (bytes[i].to_int64() << ((7 - i) * 8)), i + 1
  } else {
    rv
  }
  let lo = for rv = 0L, i = 8; i < 16; {
    continue rv | (bytes[i].to_int64() << ((15 - i) * 8)), i + 1
  } else {
    rv
  }
  { hi, lo }
}

///|
/// The UUID as a 16-byte string.
pub fn UUID::to_bytes(self : UUID) -> Bytes {
  let result = FixedArray::make(16, b'\x00')
  for i = 0; i < 8; i = i + 1 {
    result[i] = (self.hi.reinterpret_as_uint64() >> ((7 - i) * 8)).to_byte()
  }
  for i = 8; i < 16; i = i + 1 {
    result[i] = (self.lo.reinterpret_as_uint64() >> ((15 - i) * 8)).to_byte()
  }
  // move the result ownnership to Bytes
  result.unsafe_reinterpret_as_bytes()
}

///|
test "bytes" {
  let buf = Bytes::from_array(
    [201, 130, 251, 104, 223, 80, 76, 161, 145, 248, 186, 162, 4, 9, 229, 185].map(
      Int::to_byte,
    ),
  )
  inspect(
    from_bytes(buf).to_bytes(),
    content=(
      #|b"\xc9\x82\xfbh\xdfPL\xa1\x91\xf8\xba\xa2\x04\x09\xe5\xb9"
    ),
  )
  assert_true((try? from_bytes(Bytes::new(15))) is Err(_))
  assert_true((try? from_bytes(Bytes::new(17))) is Err(_))
}

///|
fn hex_char_to_int64(c : Int) -> Int64 raise Error {
  let d = if '0'.to_int() <= c && c <= '9'.to_int() {
    c - '0'
  } else if 'a'.to_int() <= c && c <= 'f'.to_int() {
    c - 'a' + 10
  } else if 'A'.to_int() <= c && c <= 'F'.to_int() {
    c - 'A' + 10
  } else {
    fail("invalid syntax")
  }
  d.to_int64()
}

///|
/// Constructs a UUID with the given hexadecimal string.
///
/// This function ignores dashes (`-`) in the given string. The remaining
/// letters must be exactly 32 hexadecimals, or an error will be raised.
///
pub fn from_hex(hex : String) -> UUID raise Error {
  // XXX: ideally we should use strconv here, but we don't have sufficient
  // tooling around String yet
  let (hi, i) = for hi = 0L, i = 0, n = 0; n < 16; {
    if i >= hex.length() {
      fail("not enough hexadecimal in UUID string")
    }
    let c = hex.code_unit_at(i)
    if c == '-' {
      continue hi, i + 1, n
    } else {
      let d = hex_char_to_int64(c.to_int())
      continue hi | (d << ((15 - n) * 4)), i + 1, n + 1
    }
  } else {
    (hi, i)
  }
  let (lo, i) = for lo = 0L, i = i, n = 0; n < 16; {
    if i >= hex.length() {
      fail("not enough hexadecimal in UUID string")
    }
    let c = hex.code_unit_at(i)
    if c == '-' {
      continue lo, i + 1, n
    } else {
      let d = hex_char_to_int64(c.to_int())
      continue lo | (d << ((15 - n) * 4)), i + 1, n + 1
    }
  } else {
    (lo, i)
  }
  if i == hex.length() {
    { hi, lo }
  } else {
    fail("badly formed hexadecimal UUID string")
  }
}

///|
let hex_table : Array[Int] = "0123456789abcdef".to_array().map(c => c.to_int())

///|
let dash : Int = '-'.to_int()

///|
let segments : ReadOnlyArray[Int] = [8, 4, 4, 0, 4, 12]

// XXX: use sum() instead; weird `moon fmt`

///|
let size : Int = (
    segments.fold((acc, x) => acc + x, init=0) + segments.length() - 2
  ) *
  2

///|
pub fn UUID::to_string(self : UUID) -> String {
  // This is a manually-composed UTF-16 string
  let rv = FixedArray::make(size, b'\x00')
  for v = self.lo, c = size - 1, i = size - 1, d = segments.length() - 1 {
    let s = segments[d] * 2
    if s == 0 {
      continue self.hi, c, i, d - 1
    } else if i > c - s {
      rv[i - 1] = hex_table[(v & 0xfL).to_int()].to_byte()
      continue (v.reinterpret_as_uint64() >> 4).reinterpret_as_int64(),
        c,
        i - 2,
        d
    } else if i > 0 {
      rv[i - 1] = dash.to_byte()
      continue v, i - 2, i - 2, d - 1
    } else {
      break Bytes::from_array(Array::from_fixed_array(rv)).to_unchecked_string()
    }
  }
}

///|
pub impl Show for UUID with output(self : UUID, logger : &Logger) -> Unit {
  logger.write_string(self.to_string())
}
