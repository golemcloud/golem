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
pub fn[Data : ByteSource] sha1(input : Data) -> FixedArray[Byte] {
  // Padding
  let old_length = input.length()
  let bits = old_length.to_uint64() * 8
  let mut new_length = (old_length / 64 + 1) * 64
  if new_length - old_length < 9 {
    new_length += 64
  }
  let bytes = FixedArray::make(new_length, b'\x00')
  input.blit_to(bytes, len=old_length, src_offset=0, dst_offset=0)
  bytes[old_length] = (0x80).to_byte()
  for i = new_length - 8; i < new_length; i = i + 1 {
    bytes[i] = (bits >> (56 - (i - (new_length - 8)) * 8)).to_byte()
  }
  // Hash
  let h = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0]
  let bytes_per_chunk = 512 / 8
  let chunk_count = new_length / bytes_per_chunk // `new_length` is a multiple of 512
  for chunk = 0; chunk < chunk_count; chunk = chunk + 1 {
    let words = FixedArray::make(80, 0)
    for i = 0; i < 16; i = i + 1 {
      let slice_start = chunk * bytes_per_chunk + i * 4
      words[i] = (bytes[slice_start].to_int() << 24) |
        (bytes[slice_start + 1].to_int() << 16) |
        (bytes[slice_start + 2].to_int() << 8) |
        bytes[slice_start + 3].to_int()
    }
    for i = 16; i < 80; i = i + 1 {
      words[i] = rotate_left(
        words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16],
        1,
      )
    }
    let v = h.copy()
    for i = 0; i < 80; i = i + 1 {
      let (f, k) = if i < 20 {
        ((v[1] & v[2]) | (v[1].lnot() & v[3]), 0x5A827999)
      } else if i < 40 {
        (v[1] ^ (v[2] ^ v[3]), 0x6ED9EBA1)
      } else if i < 60 {
        ((v[1] & v[2]) | (v[1] & v[3]) | (v[2] & v[3]), 0x8F1BBCDC)
      } else {
        (v[1] ^ (v[2] ^ v[3]), 0xCA62C1D6)
      }
      let temp = rotate_left(v[0], 5) + f + v[4] + k + words[i]
      v[4] = v[3]
      v[3] = v[2]
      v[2] = rotate_left(v[1], 30)
      v[1] = v[0]
      v[0] = temp
    }
    for i = 0; i < 5; i = i + 1 {
      h[i] += v[i]
    }
  }
  // Digest
  let result = FixedArray::make(20, b'\x00')
  for i = 0; i < 5; i = i + 1 {
    result[i * 4 + 0] = (h[i].reinterpret_as_uint() >> 24).to_byte()
    result[i * 4 + 1] = (h[i].reinterpret_as_uint() >> 16).to_byte()
    result[i * 4 + 2] = (h[i].reinterpret_as_uint() >> 8).to_byte()
    result[i * 4 + 3] = h[i].to_byte()
  }
  result
}
