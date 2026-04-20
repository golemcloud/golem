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
#alias(Sha256Context, deprecated="Use `SHA256` instead")
struct SHA256 {
  reg : FixedArray[UInt] // register A B C D E F G H. i.e. digest
  mut len : UInt64
  buf : FixedArray[Byte]
  mut buf_index : Int
}

///|
/// Instantiate a Sha256 context
/// `reg` is the initial hash value. Defaults to Sha256's.
pub fn SHA256::new(
  reg? : FixedArray[UInt] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
    0x5be0cd19,
  ],
) -> SHA256 {
  { reg, len: 0, buf: FixedArray::make(64, Byte::default()), buf_index: 0 }
}

///|
let sha256_t : FixedArray[UInt] = [ // pre calculated
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
  0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
  0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
  0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
  0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
  0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
  0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
  0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
  0xc67178f2,
]

///|
fn SHA256::transform(data : FixedArray[Byte], reg : FixedArray[UInt]) -> Unit {
  let w = FixedArray::make(64, 0U)
  guard reg.length() == 8
  let mut a = reg.unsafe_get(0)
  let mut b = reg.unsafe_get(1)
  let mut c = reg.unsafe_get(2)
  let mut d = reg.unsafe_get(3)
  let mut e = reg.unsafe_get(4)
  let mut f = reg.unsafe_get(5)
  let mut g = reg.unsafe_get(6)
  let mut h = reg.unsafe_get(7)
  for index = 0; index < 16; index = index + 1 {
    w[index] = bytes_u8_to_u32be(data, i=4 * index)
  }
  for index = 16; index < 64; index = index + 1 {
    let sigma_0 = rotate_right_u(w[index - 15], 7) ^
      rotate_right_u(w[index - 15], 18) ^
      (w[index - 15] >> 3)
    let sigma_1 = rotate_right_u(w[index - 2], 17) ^
      rotate_right_u(w[index - 2], 19) ^
      (w[index - 2] >> 10)
    w[index] = w[index - 16] + sigma_0 + w[index - 7] + sigma_1
  }
  for index = 0; index < 64; index = index + 1 {
    let big_sigma_1 = rotate_right_u(e, 6) ^
      rotate_right_u(e, 11) ^
      rotate_right_u(e, 25)
    let t_1 = h + big_sigma_1 + SM3::gg_1(e, f, g) + sha256_t[index] + w[index]
    let big_sigma_0 = rotate_right_u(a, 2) ^
      rotate_right_u(a, 13) ^
      rotate_right_u(a, 22)
    let t_2 = big_sigma_0 + SM3::ff_1(a, b, c)
    h = g
    g = f
    f = e
    e = d + t_1
    d = c
    c = b
    b = a
    a = t_1 + t_2
  }
  reg.unsafe_set(0, reg.unsafe_get(0) + a)
  reg.unsafe_set(1, reg.unsafe_get(1) + b)
  reg.unsafe_set(2, reg.unsafe_get(2) + c)
  reg.unsafe_set(3, reg.unsafe_get(3) + d)
  reg.unsafe_set(4, reg.unsafe_get(4) + e)
  reg.unsafe_set(5, reg.unsafe_get(5) + f)
  reg.unsafe_set(6, reg.unsafe_get(6) + g)
  reg.unsafe_set(7, reg.unsafe_get(7) + h)
}

///|
pub fn SHA256::update_from_iter(self : SHA256, data : Iter[Byte]) -> Unit {
  data.each(fn(b) {
    self.buf[self.buf_index] = b
    self.buf_index += 1
    if self.buf_index == 64 {
      self.buf_index = 0
      self.len += 512UL
      SHA256::transform(self.buf, self.reg)
    }
  })
}

///|
pub impl CryptoHasher for SHA256 with update(self : SHA256, data : BytesView) -> Unit {
  self.update(data)
}

///|
/// update the state of given context from new `data` 
pub fn[Data : ByteSource] SHA256::update(self : SHA256, data : Data) -> Unit {
  let mut offset = 0
  while offset < data.length() {
    let min_len = if 64 - self.buf_index >= data.length() - offset {
      data.length() - offset
    } else {
      64 - self.buf_index
    }
    data.blit_to(
      self.buf,
      len=min_len,
      src_offset=offset,
      dst_offset=self.buf_index,
    )
    self.buf_index += min_len
    if self.buf_index == 64 {
      self.len += 512UL
      self.buf_index = 0
      SHA256::transform(self.buf, self.reg)
    }
    offset += min_len
  }
}

///|
pub fn SHA256::finalize(self : SHA256) -> FixedArray[Byte] {
  let ret = FixedArray::make(32, Byte::default())
  self._finalize_into(ret)
  ret
}

///|
/// @param size the size of the output, defaults to 8 (i.e. 64 bytes). 7 for Sha224.
fn SHA256::_finalize_into(
  self : SHA256,
  buffer : FixedArray[Byte],
  size? : Int = 8,
  offset? : Int = 0,
) -> Unit {
  // Copy data
  let data = FixedArray::make(64, Byte::default())
  let mut cnt = self.buf_index
  let len = self.len + 8 * cnt.to_uint64()
  self.buf.blit_to(data, len=cnt)
  let reg = self.reg.copy()

  // Padding
  data[cnt] = b'\x80'
  cnt += 1
  if cnt > 56 {
    SHA256::transform(data, reg)
    data.fill(0)
  }
  data.unsafe_set(56, (len >> 56).to_byte())
  data.unsafe_set(57, (len >> 48).to_byte())
  data.unsafe_set(58, (len >> 40).to_byte())
  data.unsafe_set(59, (len >> 32).to_byte())
  data.unsafe_set(60, (len >> 24).to_byte())
  data.unsafe_set(61, (len >> 16).to_byte())
  data.unsafe_set(62, (len >> 8).to_byte())
  data.unsafe_set(63, (len >> 0).to_byte())
  SHA256::transform(data, reg)

  // Write result to buffer
  arr_u32_to_u8be_into(reg.iter().take(size), buffer, offset)
}

///|
pub impl CryptoHasher for SHA256 with size(_self : SHA256) -> Int {
  32
}

///|
pub impl CryptoHasher for SHA256 with block_size(_self : SHA256) -> Int {
  64
}

///|
pub impl CryptoHasher for SHA256 with reset(self : SHA256) -> Unit {
  self.reg[0] = 0x6a09e667
  self.reg[1] = 0xbb67ae85
  self.reg[2] = 0x3c6ef372
  self.reg[3] = 0xa54ff53a
  self.reg[4] = 0x510e527f
  self.reg[5] = 0x9b05688c
  self.reg[6] = 0x1f83d9ab
  self.reg[7] = 0x5be0cd19
  self.len = 0
  self.buf.fill(0)
  self.buf_index = 0
}

///|
/// Compute the Sha256 digest from given Sha256Context
pub impl CryptoHasher for SHA256 with finalize_into(
  self : SHA256,
  buffer : FixedArray[Byte],
  offset~ : Int,
) -> Unit {
  self._finalize_into(buffer, offset~)
}

///|
/// Compute the Sha256 digest in `Bytes` of some `data`. Note that Sha256 is big-endian.
pub fn[Data : ByteSource] sha256(data : Data) -> FixedArray[Byte] {
  SHA256::new()..update(data).finalize()
}

///|
pub fn sha256_from_iter(data : Iter[Byte]) -> FixedArray[Byte] {
  SHA256::new()..update_from_iter(data).finalize()
}

///|
test {
  inspect(
    bytes_to_hex_string(
      sha256(
        b"abc", // abc in utf-8
      ),
    ),
    content="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
  )
  inspect(
    bytes_to_hex_string(
      sha256(
        // abcd * 16 in utf-8
        b"abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd",
      ),
    ),
    content="625b41490b883891943c5fa54ad45d7c900b9b6e91e159334e320b1f5215a209",
  )
  let hash1 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
  let ctx = SHA256::new()
  ctx.update(b"\x61".to_fixedarray())
  ctx.update(b"\x62".to_fixedarray())
  ctx.update(b"\x63".to_fixedarray())
  assert_eq(hash1, bytes_to_hex_string(ctx.finalize()))
  let ctx = SHA256::new()
  let data = b"\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64"
  for i = 0; i < data.length(); i = i + 1 {
    ctx.update(FixedArray::make(1, data[i]))
  }
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="625b41490b883891943c5fa54ad45d7c900b9b6e91e159334e320b1f5215a209",
  )
  let ctx = SHA256::new()
  for i = 0; i < data.length(); i = i + 4 {
    ctx.update_from_iter(b"\x61\x62\x63\x64".iter())
  }
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="625b41490b883891943c5fa54ad45d7c900b9b6e91e159334e320b1f5215a209",
  )
}

///|
test "sha256 reentry" {
  let string = b"abcd"
  let ctx = SHA256::new()
  ctx.update(string)
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="88d4266fd4e6338d13b845fcf289579d209c897823b9217da3e161936f031589",
  )
  ctx.update(string)
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="3bc49b73e2fb201924d9dcce5fb6d6fd7cfbf58c49be8cc46439c05dc634b151",
  )
  ctx.update(string)
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="887f2749b07e559d140605a4b9de9af5721e2accad06fade91301f0410ad5cdf",
  )
}
