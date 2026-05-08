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
pub suberror MalformedError {
  MalformedError(Bytes)
}

///|
pub suberror TruncatedError {
  TruncatedError(Bytes)
}

///|
pub(all) enum Encoding {
  UTF8
  UTF16 // alias for UTF16LE
  UTF16LE
  UTF16BE
}

// Decoder

///|
priv struct Cont((Decoder) -> Decode)

///|
struct Decoder {
  // Input bytes
  // Stores the input bytes that need to be decoded.
  // The primary data source from which characters are read and decoded.
  mut i : FixedArray[Byte]
  // Input position
  // Keeps track of the current position within the input bytes `i`.
  // Indicates the next byte (starting point) to read from `i` during the decoding process
  mut i_pos : Int
  // Temporary bytes
  // Used to temporarily store bytes that are read in parts
  // (which might happen for multi-byte encoded characters).
  t : FixedArray[Byte]
  // Temporary Length
  // Tracks how many bytes currently reside in the temporary bytes `t`.
  mut t_len : Int
  // Temporary Need
  // The number of bytes still needed to complete the character code currently being processed.
  mut t_need : Int
  // Continuation
  // Called with a `Decoder` state.
  mut k : Cont
}

///|
priv enum Decode {
  End
  Refill(Bytes)
  Malformed(Bytes)
  Uchar(Char)
} derive(Show)

///|
test {
  // make unused warning happy
  inspect(End.to_string(), content="End")
}

// helper constructor

///|
fn slice(bytes : FixedArray[Byte], offset : Int, length : Int) -> Bytes {
  let new_bytes = FixedArray::make(length, b'0')
  bytes.blit_to(new_bytes, len=length, src_offset=offset)
  Bytes::from_array(new_bytes)
}

///|
fn malformed(bytes : FixedArray[Byte], offset : Int, length : Int) -> Decode {
  Malformed(slice(bytes, offset, length))
}

///|
fn malformed_pair(
  be : Bool,
  hi : Int,
  bytes : FixedArray[Byte],
  offset : Int,
  length : Int,
) -> Decode {
  let bs1 = FixedArray::make(length, Byte::default())
  bytes.blit_to(bs1, len=length, src_offset=offset)
  let bs0 = FixedArray::make(2, Byte::default())
  let (j0, j1) = if be { (0, 1) } else { (1, 0) }
  bs0[j0] = (hi >> 8).to_byte()
  bs0[j1] = hi.land(0xFF).to_byte()
  let bs = @buffer.new(size_hint=bs0.length() + bs1.length())
  // specify a known length to skip call method `bs0.length()`
  bs.write_bytes(Bytes::from_array(bs0[0:2]))
  // ditto
  bs.write_bytes(Bytes::from_array(bs1[0:length]))
  Malformed(slice(bs.to_bytes().to_fixedarray(), 0, bs.length()))
}
