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
/// Encode a given string to the specified character encoding and returns the resulting bytes.
///
/// # Parameters
///
/// - `encoding` : The target encoding format.
/// - `src`: The input string to be encoded.
///
/// # Returns
///
/// A `bytes` representing the encoded string in the selected format.
///
/// # Examples
///
/// ```moonbit check
/// test {
///   let src = "Hello, World!"
///   let encoded_bytes = encode(UTF8, src)
///   assert_eq(
///     encoded_bytes,
///     Bytes::from_array([
///       72, 101, 108, 108, 111, 44, 32, 87, 111, 114, 108, 100, 33,
///     ]),
///   )
/// }
/// ```
pub fn encode(encoding : Encoding, src : String) -> Bytes {
  // NOTE: special case: MoonBit String are already valid UTF16(LE) bytes
  match encoding {
    UTF16 | UTF16LE => {
      let arr = FixedArray::make(src.length() * 2, b'\x00')
      arr.blit_from_string(0, src, 0, src.length())
      return arr.unsafe_reinterpret_as_bytes()
    }
    _ => ()
  }
  let new_buf = @buffer.new(size_hint=src.length() * 4)
  let write = match encoding {
    UTF8 => write_utf8_char
    UTF16BE => write_utf16be_char
    _ => abort("unreachable")
  }
  for char in src {
    // SAFETY: Assume String are always valid UTF16LE
    write(new_buf, char)
  }
  new_buf.to_bytes()
}

///|
/// Encodes a string into the specified character encoding and writes the result
/// directly into a buffer.
///
/// Parameters:
///
/// * `string` : The input string to be encoded.
/// * `buffer` : The buffer where the encoded bytes will be written to.
/// * `encoding` : The target encoding format. Defaults to UTF8 if not specified.
///
/// Example:
///
/// ```moonbit check
/// test {
///   let buf = @buffer.new()
///   let text = "Hello, world"
///   @encoding.encode_to(text, buf, encoding=UTF16)
///   inspect(buf.to_string(), content="Hello, world")
/// }
/// ```
pub fn encode_to(
  src : String,
  buffer : @buffer.Buffer,
  encoding~ : Encoding,
) -> Unit {
  match encoding {
    UTF8 =>
      for char in src {
        write_utf8_char(buffer, char)
      }
    UTF16BE =>
      for char in src {
        write_utf16be_char(buffer, char)
      }
    UTF16 | UTF16LE => buffer.write_string(src)
  }
}

///|
/// Converts a Char to UTF-8 bytes.
///
/// Note that this function incurs allocation overhead
/// due to the intermediate creation of new buffers.
pub fn to_utf8_bytes(value : Char) -> Bytes {
  let buf = @buffer.new(size_hint=4)
  write_utf8_char(buf, value)
  buf.to_bytes()
}

///|
/// Converts a Char to UTF-16LE bytes.
///
/// Alias for `to_utf16le_bytes`.
///
/// Note that this function incurs allocation overhead
/// due to the intermediate creation of new buffers.
pub fn to_utf16_bytes(value : Char) -> Bytes {
  to_utf16le_bytes(value)
}

///|
/// Converts a Char to UTF-16LE bytes.
///
/// Note that this function incurs allocation overhead
/// due to the intermediate creation of new buffers.
pub fn to_utf16le_bytes(value : Char) -> Bytes {
  let buf = @buffer.new(size_hint=4)
  write_utf16le_char(buf, value)
  buf.to_bytes()
}

///|
/// Converts a Char to UTF-16BE bytes.
///
/// Note that this function incurs allocation overhead
/// due to the intermediate creation of new buffers.
pub fn to_utf16be_bytes(value : Char) -> Bytes {
  let buf = @buffer.new(size_hint=4)
  write_utf16be_char(buf, value)
  buf.to_bytes()
}

///|
/// Write a char into buffer as UTF8.
pub fn write_utf8_char(buf : @buffer.Buffer, value : Char) -> Unit {
  let code = value.to_uint()
  match code {
    _..<0x80 => {
      let b0 = ((code & 0x7F) | 0x00).to_byte()
      buf.write_byte(b0)
    }
    _..<0x0800 => {
      let b0 = (((code >> 6) & 0x1F) | 0xC0).to_byte()
      let b1 = ((code & 0x3F) | 0x80).to_byte()
      buf.write_byte(b0)
      buf.write_byte(b1)
    }
    _..<0x010000 => {
      let b0 = (((code >> 12) & 0x0F) | 0xE0).to_byte()
      let b1 = (((code >> 6) & 0x3F) | 0x80).to_byte()
      let b2 = ((code & 0x3F) | 0x80).to_byte()
      buf.write_byte(b0)
      buf.write_byte(b1)
      buf.write_byte(b2)
    }
    _..<0x110000 => {
      let b0 = (((code >> 18) & 0x07) | 0xF0).to_byte()
      let b1 = (((code >> 12) & 0x3F) | 0x80).to_byte()
      let b2 = (((code >> 6) & 0x3F) | 0x80).to_byte()
      let b3 = ((code & 0x3F) | 0x80).to_byte()
      buf.write_byte(b0)
      buf.write_byte(b1)
      buf.write_byte(b2)
      buf.write_byte(b3)
    }
    _ => abort("Char out of range")
  }
}

///|
/// Write a char into buffer as UTF16LE.
/// Alias for `write_utf16le_char`
pub fn write_utf16_char(buf : @buffer.Buffer, value : Char) -> Unit {
  write_utf16le_char(buf, value)
}

///|
/// Write a char into buffer as UTF16LE.
pub fn write_utf16le_char(buf : @buffer.Buffer, value : Char) -> Unit {
  let code = value.to_uint()
  if code < 0x10000 {
    let b0 = (code & 0xFF).to_byte()
    let b1 = (code >> 8).to_byte()
    buf.write_byte(b0)
    buf.write_byte(b1)
  } else if code < 0x110000 {
    let hi = code - 0x10000
    let lo = (hi >> 10) | 0xD800
    let hi = (hi & 0x3FF) | 0xDC00
    let b0 = (lo & 0xFF).to_byte()
    let b1 = (lo >> 8).to_byte()
    let b2 = (hi & 0xFF).to_byte()
    let b3 = (hi >> 8).to_byte()
    buf.write_byte(b0)
    buf.write_byte(b1)
    buf.write_byte(b2)
    buf.write_byte(b3)
  } else {
    abort("Char out of range")
  }
}

///|
/// Write a char into buffer as UTF16BE.
pub fn write_utf16be_char(buf : @buffer.Buffer, value : Char) -> Unit {
  let code = value.to_uint()
  if code < 0x10000 {
    let b0 = (code >> 8).to_byte()
    let b1 = (code & 0xFF).to_byte()
    buf.write_byte(b0)
    buf.write_byte(b1)
  } else if code < 0x110000 {
    let hi = code - 0x10000
    let lo = (hi >> 10) | 0xD800
    let hi = (hi & 0x3FF) | 0xDC00
    let b0 = (lo >> 8).to_byte()
    let b1 = (lo & 0xFF).to_byte()
    let b2 = (hi >> 8).to_byte()
    let b3 = (hi & 0xFF).to_byte()
    buf.write_byte(b0)
    buf.write_byte(b1)
    buf.write_byte(b2)
    buf.write_byte(b3)
  } else {
    abort("Char out of range")
  }
}
