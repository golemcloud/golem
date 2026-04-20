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
/// Converts a byte array to a hex string, without any prefix like "0x".
let hex_digits : FixedArray[String] = [
  "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "a", "b", "c", "d", "e", "f",
]

///|
/// print a sequence of byte in hex representation
pub fn[D : ByteSource] bytes_to_hex_string(input : D) -> String {
  let mut ret = ""
  for i = input.length() - 1; i >= 0; i = i - 1 {
    let byte = input[i]
    let high = (byte.to_uint() >> 4) & 0xf
    let low = byte.to_int() & 0xf
    let high_char = hex_digits[high.reinterpret_as_int()]
    let low_char = hex_digits[low]
    ret = high_char + low_char + ret
  }
  ret
}

///|
const HEX_DIGITS = "0123456789abcdef"

///|
/// convert a sequence of `UInt` to hex representation
pub fn uints_to_hex_string(input : Iter[UInt]) -> String {
  let buffer = StringBuilder::new()
  let rems : FixedArray[Int] = FixedArray::make(8, 0)
  for x in input {
    let mut value = x
    for i in 0..<8 {
      let rem = value & 0x0f
      let quot = value >> 4
      value = quot
      rems[8 - i - 1] = rem.reinterpret_as_int()
    }
    for xdigit in rems {
      buffer.write_char(
        HEX_DIGITS.code_unit_at(xdigit).to_int() |> Int::unsafe_to_char,
      )
    }
  }
  buffer.to_string()
}

///|
test "uints_to_hex_string" {
  let xs : Array[UInt] = [0x12345678, 0xabcdcdab]
  inspect(uints_to_hex_string(xs.iter()), content="12345678abcdcdab")
}

///|
fn uint32(x : Byte) -> UInt {
  x.to_int().reinterpret_as_uint()
}

///|
/// convert 4 bytes a byte sequence to a UInt in little endian
/// - `x` : A byte sequence consisting of 4 or more bytes
/// - `i` : An offset. e.g. i = 4 will convert `x[4~7]` to a UInt.
fn u8_to_u32le(x : FixedArray[Byte], i? : Int = 0) -> UInt {
  uint32(x[i]) |
  (uint32(x[i + 1]) << 8) |
  (uint32(x[i + 2]) << 16) |
  (uint32(x[i + 3]) << 24)
}

/// convert 4 bytes of a byte sequence to a UInt in big endian 
/// - `x` : A byte sequence consisting of 4 or more bytes
/// - `i` : An offset. e.g. i = 4 will convert `x[4~7]` to a UInt.

// fn u8_to_u32be(x : Bytes, ~i : Int = 0) -> UInt {
//   uint32(x[i]).lsl(24) | uint32(x[i + 1]).lsl(16) | uint32(x[i + 2]).lsl(8) | uint32(
//     x[i + 3],
//   )
// }

// fn arr_u8_to_u32be(x : Array[Byte], ~i : Int = 0) -> UInt {
//   uint32(x[i]).lsl(24) | uint32(x[i + 1]).lsl(16) | uint32(x[i + 2]).lsl(8) | uint32(
//     x[i + 3],
//   )
// }

///|
/// convert 4 bytes of a byte array to a UInt in big endian 
/// - `x` : A byte sequence consisting of 4 or more bytes
/// - `i` : An offset. e.g. i = 4 will convert `x[4~7]` to a UInt.
fn bytes_u8_to_u32be(x : FixedArray[Byte], i? : Int = 0) -> UInt {
  (uint32(x[i]) << 24) |
  (uint32(x[i + 1]) << 16) |
  (uint32(x[i + 2]) << 8) |
  uint32(x[i + 3])
}

///|
fn arr_u32_to_u8be_into(
  x : Iter[UInt],
  buffer : FixedArray[Byte],
  offset : Int,
) -> Unit {
  let mut idx = offset
  x.each(fn(d) {
    buffer[idx + 0] = (d >> 24).to_byte()
    buffer[idx + 1] = (d >> 16).to_byte()
    buffer[idx + 2] = (d >> 8).to_byte()
    buffer[idx + 3] = d.to_byte()
    idx += 4
  })
}

///|
/// rotate a Int `x` left by `n` bit(s)
fn rotate_left(x : Int, n : Int) -> Int {
  (x << n) | (x.reinterpret_as_uint() >> (32 - n)).reinterpret_as_int()
}

///|
/// rotate a UInt `x` left by `n` bit(s)
fn rotate_left_u(x : UInt, n : Int) -> UInt {
  (x << n) | (x >> (32 - n))
}

///|
fn rotate_right_u(x : UInt, n : Int) -> UInt {
  (x >> n) | (x << (32 - n))
}
