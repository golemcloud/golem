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
/// The Unicode Replacement Character, which is used to replace invalid or unrecognized sequences during lossy decoding.
/// https://unicode.org/charts/nameslist/n_FFF0.html
pub const U_REP = '\u{FFFD}'

///|
let utf_8_len : ReadOnlyArray[Int] = [
  1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
  1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
  1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
  1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
  1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0,
  0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
  0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
  0, 0, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
  2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4,
  4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]

///|
/// Create and return a `Decoder` for the specified character encoding.
///
/// The `Decoder` consumes byte sequences and decodes them into the original string format.
///
/// # Parameters
///
/// - `encoding`: The character encoding format to be used for decoding the input byte sequences.
///
/// # Returns
///
/// A `Decoder` instance that can be used to decode byte sequences into strings.
///
/// # Examples
///
/// ```moonbit
/// let inputs = [b"abc", b"\xf0", b"\x9f\x90\xb0"] // UTF8(🐰) == <F09F 90B0>
/// let decoder = decoder(UTF8)
/// inspect(decoder.consume(inputs[0]), content="abc")
/// inspect(decoder.consume(inputs[1]), content="")
/// inspect(decoder.consume(inputs[2]), content="🐰")
/// assert_true(decoder.finish().is_empty())
pub fn decoder(encoding : Encoding) -> Decoder {
  let i = FixedArray::default()
  let i_pos = 0
  let t = FixedArray::make(4, Byte::default())
  let t_len = 0
  let t_need = 0
  let k = match encoding {
    UTF8 => Decoder::decode_utf_8
    UTF16 => Decoder::decode_utf_16le
    UTF16LE => Decoder::decode_utf_16le
    UTF16BE => Decoder::decode_utf_16be
  }
  { i, i_pos, t, t_len, t_need, k }
}

///|
/// Decode the given byte sequence using the specified `Decoder` and return the resulting string.
///
/// This function can work in streaming mode where bytes are consumed incrementally.
/// When `stream` is `false`, it indicates the end of the input and triggers the final decoding step.
///
/// # Parameters
///
/// - `self`: The `Decoder` instance used to decode the byte sequence.
/// - `input`: The byte sequence to be decoded.
/// - `stream~`: A boolean indicating whether more bytes will be supplied for decoding. It defaults to `false`.
///
/// # Returns
///
/// A `String` representing the decoded content from the input byte sequence.
///
/// # Errors
///
/// `MalformedError`: when the byte sequence is not properly formatted according to the specified encoding.
/// `TruncatedError`: when the byte sequence ends prematurely, implying that more data is expected for complete decoding.
///
/// # Examples
///
/// ```moonbit check
/// test {
///   let inputs = [b"abc", b"\xf0", b"\x9f\x90\xb0"] // UTF8(🐰) == <F09F 90B0>
///   let decoder = @encoding.decoder(UTF8)
///   inspect(decoder.decode(inputs[0], stream=true), content="abc")
///   inspect(decoder.decode(inputs[1], stream=true), content="")
///   inspect(decoder.decode(inputs[2], stream=false), content="🐰")
/// }
/// ```
pub fn Decoder::decode(
  self : Decoder,
  input : BytesView,
  stream? : Bool = false,
) -> String raise Error {
  if input.length() > 0 {
    self.i_cont(input)
  }
  if self.i_rem() == 0 {
    return String::default()
  }

  // TODO: Estimate size_hint based on input and encoding more accurately
  let builder = StringBuilder::new(size_hint=input.length())

  // drive decoder to decode
  loop self.decode_() {
    Uchar(u) => {
      builder.write_char(u)
      continue self.decode_()
    }
    Malformed(bs) =>
      if stream && self.t_need > 0 {
        builder.to_string()
      } else {
        raise MalformedError(bs)
      }
    End => builder.to_string()
    Refill(t) =>
      if stream {
        builder.to_string()
      } else {
        raise TruncatedError(t)
      }
  }
}

///|
/// Decodes the given byte sequence using the specified decoder and writes the
/// result directly to a StringBuilder.
/// Similar to `decode!`, but writes the result to an existing StringBuilder
/// instead of creating a new String.
///
/// Parameters:
///
/// * `decoder` : The decoder instance used to decode the byte sequence.
/// * `input` : The byte sequence to be decoded.
/// * `output` : The StringBuilder where the decoded content will be written to.
///
/// Throws a `MalformedError` when the byte sequence is not properly formatted
/// according to the specified encoding.
///
/// Example:
///
/// ```moonbit check
/// test {
///   let decoder = decoder(UTF8)
///   let buf = StringBuilder::new()
///   decoder.decode_to(b"Hello", buf)
///   inspect(buf.to_string(), content="Hello")
/// }
/// ```
pub fn Decoder::decode_to(
  self : Decoder,
  input : BytesView,
  output : StringBuilder,
  stream? : Bool = false,
) -> Unit raise {
  if input.length() > 0 {
    self.i_cont(input)
  }
  if self.i_rem() == 0 {
    return
  }
  // drive decoder to decode
  loop self.decode_() {
    Uchar(u) => {
      output.write_char(u)
      continue self.decode_()
    }
    Malformed(bs) =>
      if stream && self.t_need > 0 {
        return
      } else {
        raise MalformedError(bs)
      }
    End => return
    Refill(t) => if stream { return } else { raise TruncatedError(t) }
  }
}

///|
pub fn decode_to(
  input : BytesView,
  output : StringBuilder,
  encoding~ : Encoding,
) -> Unit raise {
  decoder(encoding).decode_to(input, output)
}

///|
/// Consume the given byte sequence using the specified `Decoder` and return the resulting string incrementally.
///
/// This function calls `decode!` with the `stream` parameter set to `true`, indicating that more bytes will follow for decoding.
///
/// # Parameters
///
/// - `self`: The `Decoder` instance used to consume the byte sequence.
/// - `input`: The byte sequence to be consumed and decoded incrementally.
///
/// # Returns
///
/// A `String` representing the partially decoded content from the input byte sequence, as more bytes are expected.
///
/// # Errors
///
/// `MalformedError`: when the byte sequence is not properly formatted according to the specified encoding.
/// `TruncatedError`: when the byte sequence ends prematurely, implying that more data is expected for complete decoding.
pub fn Decoder::consume(
  self : Decoder,
  input : BytesView,
) -> String raise Error {
  self.decode(input, stream=true)
}

///|
/// Finalize the decoding process and return the remaining decoded string.
///
/// This function calls `decode!` with the `stream` parameter set to `false`, indicating that no more bytes will be supplied
/// and triggering the final decoding step to produce the remaining output.
///
/// # Parameters
///
/// - `self`: The `Decoder` instance used to finalize the decoding process.
///
/// # Returns
///
/// A `String` representing the final part of the decoded content, after all byte sequences have been processed.
///
/// # Errors
///
/// `MalformedError`: This error is raised if the remaining byte sequence is not properly formatted according to the specified encoding.
/// `TruncatedError`: This error is raised if the remaining byte sequence ends prematurely, implying that more data was expected for complete decoding.
pub fn Decoder::finish(self : Decoder) -> String raise Error {
  self.decode(b"", stream=false)
}

///|
/// Decode the given byte sequence using the specified `Decoder` and return the resulting string, replacing any invalid sequences with the Unicode Replacement Character (`U+FFFD`).
///
/// This function can work in streaming mode where bytes are consumed incrementally.
/// When `stream` is `false`, it indicates the end of the input and triggers the final decoding step.
///
/// # Parameters
///
/// - `self`: The `Decoder` instance used to decode the byte sequence.
/// - `input`: The byte sequence to be decoded.
/// - `stream~`: A boolean indicating whether more bytes will be supplied for decoding. It defaults to `false`.
///
/// # Returns
///
/// A `String` representing the decoded content from the input byte sequence, with any invalid sequences replaced by the Unicode Replacement Character (`U+FFFD`).
pub fn Decoder::decode_lossy(
  self : Decoder,
  input : BytesView,
  stream? : Bool = false,
) -> String {
  if input.length() > 0 {
    self.i_cont(input)
  }
  if self.i_rem() == 0 {
    return String::default()
  }

  // drive decoder to decode
  let chars = []
  loop self.decode_() {
    Uchar(u) => {
      chars.push(u)
      continue self.decode_()
    }
    Malformed(_) =>
      if stream && self.t_need > 0 {
        String::from_array(chars)
      } else {
        chars.push(U_REP)
        continue self.decode_()
      }
    End => String::from_array(chars)
    Refill(_) =>
      if stream {
        String::from_array(chars)
      } else {
        continue self.decode_()
      }
  }
}

///|
pub fn Decoder::decode_lossy_to(
  self : Decoder,
  input : BytesView,
  output : StringBuilder,
  stream? : Bool = false,
) -> Unit {
  if input.length() > 0 {
    self.i_cont(input)
  }
  if self.i_rem() == 0 {
    return
  }

  // drive decoder to decode
  loop self.decode_() {
    Uchar(u) => {
      output.write_char(u)
      continue self.decode_()
    }
    Malformed(_) =>
      if stream && self.t_need > 0 {
        return
      } else {
        output.write_char(U_REP)
        continue self.decode_()
      }
    End => return
    Refill(_) => if stream { return } else { continue self.decode_() }
  }
}

///|
/// Consume the given byte sequence using the specified `Decoder` and return the resulting string incrementally, replacing any invalid sequences with the Unicode Replacement Character (`U+FFFD`).
///
/// This function calls `decode_lossy` with the `stream` parameter set to `true`, indicating that more bytes will follow for decoding.
///
/// # Parameters
///
/// - `self`: The `Decoder` instance used to consume and decode the byte sequence.
/// - `input`: The byte sequence to be consumed and decoded incrementally.
///
/// # Returns
///
/// A `String` representing the partially decoded content from the input byte sequence, with any invalid sequences replaced by the Unicode Replacement Character (`U+FFFD`), as more bytes are expected.
pub fn Decoder::lossy_consume(self : Decoder, input : BytesView) -> String {
  self.decode_lossy(input, stream=true)
}

///|
/// Finalize the lossy decoding process and return the remaining decoded string, replacing any invalid sequences with the Unicode Replacement Character (`U+FFFD`).
///
/// This function calls `decode_lossy` with the `stream` parameter set to `false`, indicating that no more bytes will be supplied
/// and triggering the final decoding step to produce the remaining output.
///
/// # Parameters
///
/// - `self`: The `Decoder` instance used to finalize the lossy decoding process.
///
/// # Returns
///
/// A `String` representing the final part of the decoded content, with any invalid sequences replaced by the Unicode Replacement Character (`U+FFFD`), after all byte sequences have been processed.
pub fn Decoder::lossy_finish(self : Decoder) -> String {
  self.decode_lossy(b"", stream=false)
}

///|
fn Decoder::i_cont(self : Decoder, input : BytesView) -> Unit {
  // concat `input` to `i`, drop decoded `i`
  let i_rem = @cmp.maximum(self.i_rem(), 0)
  let new_len = i_rem + input.length()
  // init a new `i`
  let new_i = FixedArray::make(new_len, Byte::default())
  if i_rem > 0 {
    // copy the remainder of the old `i` into the new `i`
    self.i.blit_to(new_i, len=i_rem, src_offset=self.i_pos)
  }
  // copy all `input` into new `i`, starting at the remainder of the old `i`
  new_i.blit_from_bytesview(i_rem, input)
  self.i = new_i
  // reset position to starting position
  self.i_pos = 0
}

// Implementations

///|
fn Decoder::decode_(self : Decoder) -> Decode {
  (self.k)(self)
}

///|
fn Decoder::ret(self : Decoder, k : Cont, v : Decode) -> Decode {
  self.k = k
  v
}

///|
fn Decoder::i_rem(self : Decoder) -> Int {
  self.i.length() - self.i_pos
}

///|
fn Decoder::t_need(self : Decoder, need : Int) -> Unit {
  self.t_len = 0
  self.t_need = need
}

///|
fn Decoder::eoi(self : Decoder) -> Unit {
  self.i = FixedArray::default()
}

///|
fn Decoder::refill(self : Decoder, k : Cont) -> Decode {
  self.eoi()
  self.ret(k, Decode::Refill(Bytes::from_array(self.t)))
}

///|
fn t_fill(k : Cont, decoder : Decoder) -> Decode {
  fn blit(decoder : Decoder, l : Int) -> Unit {
    decoder.i.blit_to(
      decoder.t,
      len=l,
      dst_offset=decoder.t_len,
      src_offset=decoder.i_pos,
    )
    decoder.i_pos += l
    decoder.t_len += l
  }

  let rem = decoder.i_rem()
  if rem < 0 { // eoi
    k(decoder)
  } else {
    let need = decoder.t_need - decoder.t_len
    if rem < need {
      blit(decoder, rem)
      decoder.refill(curry(t_fill)(k))
    } else {
      blit(decoder, need)
      k(decoder)
    }
  }
}

// UTF8

///|
fn Decoder::decode_utf_8(self : Decoder) -> Decode {
  let rem = self.i_rem()
  if rem <= 0 {
    Decode::End
  } else {
    let idx = self.i[self.i_pos].to_int()
    let need = utf_8_len[idx]
    if rem < need {
      self.t_need(need)
      t_fill(Decoder::t_decode_utf_8, self)
    } else {
      let j = self.i_pos
      if need == 0 {
        self.i_pos += 1
        self.ret(Decoder::decode_utf_8, malformed(self.i, j, 1))
      } else {
        self.i_pos += need
        self.ret(Decoder::decode_utf_8, r_utf_8(self.i, j, need))
      }
    }
  }
}

///|
fn Decoder::t_decode_utf_8(self : Decoder) -> Decode {
  if self.t_len < self.t_need {
    self.ret(Decoder::decode_utf_8, malformed(self.t, 0, self.t_len))
  } else {
    self.ret(Decoder::decode_utf_8, r_utf_8(self.t, 0, self.t_len))
  }
}

///|
fn r_utf_8(bytes : FixedArray[Byte], offset : Int, length : Int) -> Decode {
  fn uchar(c : Int) {
    Uchar(Int::unsafe_to_char(c))
  }

  match length {
    1 => uchar(bytes[offset].to_int())
    2 => {
      let b0 = bytes[offset].to_int()
      let b1 = bytes[offset + 1].to_int()
      if b1 >> 6 != 0b10 {
        malformed(bytes, offset, length)
      } else {
        uchar(((b0 & 0x1F) << 6) | (b1 & 0x3F))
      }
    }
    3 => {
      let b0 = bytes[offset].to_int()
      let b1 = bytes[offset + 1].to_int()
      let b2 = bytes[offset + 2].to_int()
      let c = ((b0 & 0x0F) << 12) | (((b1 & 0x3F) << 6) | (b2 & 0x3F))
      if b2 >> 6 != 0b10 {
        malformed(bytes, offset, length)
      } else {
        match b0 {
          0xE0 =>
            if b1 < 0xA0 || 0xBF < b1 {
              malformed(bytes, offset, length)
            } else {
              uchar(c)
            }
          0xED =>
            if b1 < 0x80 || 0x9F < b1 {
              malformed(bytes, offset, length)
            } else {
              uchar(c)
            }
          _ =>
            if b1 >> 6 != 0b10 {
              malformed(bytes, offset, length)
            } else {
              uchar(c)
            }
        }
      }
    }
    4 => {
      let b0 = bytes[offset].to_int()
      let b1 = bytes[offset + 1].to_int()
      let b2 = bytes[offset + 2].to_int()
      let b3 = bytes[offset + 3].to_int()
      let c = ((b0 & 0x07) << 18) |
        ((b1 & 0x3F) << 12) |
        ((b2 & 0x3F) << 6) |
        (b3 & 0x3F)
      if b3 >> 6 != 0b10 || b2 >> 6 != 0b10 {
        malformed(bytes, offset, length)
      } else {
        match b0 {
          0xF0 =>
            if b1 < 0x90 || 0xBF < b1 {
              malformed(bytes, offset, length)
            } else {
              uchar(c)
            }
          0xF4 =>
            if b1 < 0x80 || 0x8F < b1 {
              malformed(bytes, offset, length)
            } else {
              uchar(c)
            }
          _ =>
            if b1 >> 6 != 0b10 {
              malformed(bytes, offset, length)
            } else {
              uchar(c)
            }
        }
      }
    }
    _ => panic()
  }
}

// UTF16LE

///|
priv enum UTF16Decode {
  Hi(Int)
  UTF16Malformed(Bytes)
  UTF16Uchar(Char)
}

///|
fn Decoder::decode_utf_16le(self : Decoder) -> Decode {
  let rem = self.i_rem()
  if rem <= 0 {
    Decode::End
  } else if rem < 2 {
    self.t_need(2)
    t_fill(Decoder::t_decode_utf_16le, self)
  } else {
    let j = self.i_pos
    self.i_pos += 2
    self.decode_utf_16le_lo(r_utf_16(self.i, j + 1, j))
  }
}

///|
fn Decoder::t_decode_utf_16le(self : Decoder) -> Decode {
  if self.t_len < self.t_need {
    self.ret(Decoder::decode_utf_16le, malformed(self.t, 0, self.t_len))
  } else {
    self.decode_utf_16le_lo(r_utf_16(self.t, 1, 0))
  }
}

///|
fn Decoder::decode_utf_16le_lo(self : Decoder, v : UTF16Decode) -> Decode {
  match v {
    UTF16Uchar(u) => self.ret(Decoder::decode_utf_16le, Uchar(u))
    UTF16Malformed(s) => self.ret(Decoder::decode_utf_16le, Malformed(s))
    Hi(hi) => {
      let rem = self.i_rem()
      if rem < 2 {
        self.t_need(2)
        t_fill(curry(t_decode_utf_16le_lo)(hi), self)
      } else {
        let j = self.i_pos
        let dcd = r_utf_16_lo(hi, self.i, j + 1, j)
        match dcd {
          Uchar(_) => self.i_pos += 2
          _ => ()
        }
        self.ret(Decoder::decode_utf_16le, dcd)
      }
    }
  }
}

///|
fn t_decode_utf_16le_lo(hi : Int, decoder : Decoder) -> Decode {
  if decoder.t_len < decoder.t_need {
    decoder.ret(
      Decoder::decode_utf_16le,
      malformed_pair(false, hi, decoder.t, 0, decoder.t_len),
    )
  } else {
    decoder.ret(Decoder::decode_utf_16le, r_utf_16_lo(hi, decoder.t, 1, 0))
  }
}

///|
fn r_utf_16_lo(
  hi : Int,
  bytes : FixedArray[Byte],
  offset0 : Int,
  offset1 : Int,
) -> Decode {
  let b0 = bytes[offset0].to_int()
  let b1 = bytes[offset1].to_int()
  let lo = (b0 << 8) | b1
  if lo < 0xDC00 || lo > 0xDFFF {
    // NOTE(jinser): only hi malformed, skip lo if lo is illegal
    //
    // For example, b"\xD8\x00\x00\x48" (BE)
    // Since \xD8\x00 is *legal* hi, here will try to parse lo next,
    // however the whole \xD8\x00\x00\x48 is *illegal* so the result will be a `Malformed[b"\xD8\x00\x00\x48"]`
    //
    // But \x00\x48 itself is a *legal* UTF16 code point with a value of `H`,
    // the ideal result should be: `[Malformed(b"\xD8\x00"), Uchar('H')]`
    //
    // > printf '\xD8\x00\x00\x48' | uconv --from-code UTF16BE --to-code UTF8 --from-callback substitute
    // �H
    Malformed([bytes[offset0], bytes[offset1]])
  } else {
    Uchar(Int::unsafe_to_char(((hi & 0x3FF) << 10) | ((lo & 0x3FF) + 0x10000)))
  }
}

///|
fn r_utf_16(
  bytes : FixedArray[Byte],
  offset0 : Int,
  offset1 : Int,
) -> UTF16Decode {
  let b0 = bytes[offset0].to_int()
  let b1 = bytes[offset1].to_int()
  let u = (b0 << 8) | b1
  if u < 0xD800 || u > 0xDFFF {
    UTF16Uchar(Int::unsafe_to_char(u))
  } else if u > 0xDBFF {
    UTF16Malformed(slice(bytes, @cmp.minimum(offset0, offset1), 2))
  } else {
    Hi(u)
  }
}

// UTF16BE

///|
fn Decoder::decode_utf_16be(self : Decoder) -> Decode {
  let rem = self.i_rem()
  if rem <= 0 {
    Decode::End
  } else if rem < 2 {
    self.t_need(2)
    t_fill(Decoder::t_decode_utf_16be, self)
  } else {
    let j = self.i_pos
    self.i_pos += 2
    self.decode_utf_16be_lo(r_utf_16(self.i, j, j + 1))
  }
}

///|
fn Decoder::t_decode_utf_16be(self : Decoder) -> Decode {
  if self.t_len < self.t_need {
    self.ret(Decoder::decode_utf_16be, malformed(self.t, 0, self.t_len))
  } else {
    self.decode_utf_16be_lo(r_utf_16(self.t, 0, 1))
  }
}

///|
fn Decoder::decode_utf_16be_lo(self : Decoder, decode : UTF16Decode) -> Decode {
  match decode {
    UTF16Uchar(x) => self.ret(Decoder::decode_utf_16be, Uchar(x))
    UTF16Malformed(x) => self.ret(Decoder::decode_utf_16be, Malformed(x))
    Hi(hi) => {
      let rem = self.i_rem()
      if rem < 2 {
        self.t_need(2)
        t_fill(curry(t_decode_utf_16be_lo)(hi), self)
      } else {
        let j = self.i_pos
        let dcd = r_utf_16_lo(hi, self.i, j, j + 1)
        match dcd {
          Uchar(_) => self.i_pos += 2
          _ => ()
        }
        self.ret(Decoder::decode_utf_16be, dcd)
      }
    }
  }
}

///|
fn[T, U, V] curry(f : (T, U) -> V) -> (T) -> (U) -> V {
  fn(x : T) { fn(y : U) -> V { f(x, y) } }
}

///|
fn t_decode_utf_16be_lo(hi : Int, self : Decoder) -> Decode {
  if self.t_len < self.t_need {
    self.ret(
      Decoder::decode_utf_16be,
      malformed_pair(true, hi, self.t, 0, self.t_len),
    )
  } else {
    self.ret(Decoder::decode_utf_16be, r_utf_16_lo(hi, self.t, 0, 1))
  }
}
