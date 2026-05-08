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

// A SM3 implementation based on
// - [GM/T 0004-2012]    https://www.oscca.gov.cn/sca/xxgk/2010-12/17/1002389/files/302a3ada057c4a73830536d03e683110.pdf
// SM3 is as secure as SHA256, providing similar performance. https://doi.org/10.3390/electronics8091033

///|
#alias(SM3Context, deprecated="Use `SM3` instead")
struct SM3 {
  reg : FixedArray[UInt] // register A B C D E F G H. i.e. digest
  mut len : UInt64
  buf : FixedArray[Byte]
  mut buf_index : Int
}

///|
/// Instantiate a SM3 context
pub fn SM3::new() -> SM3 {
  {
    reg: [
      0x7380166F, 0x4914B2B9, 0x172442D7, 0xDA8A0600, 0xA96F30BC, 0x163138AA, 0xE38DEE4D,
      0xB0FB0E4E,
    ],
    len: 0,
    buf: FixedArray::make(64, 0),
    buf_index: 0,
  }
}

///|
let t : FixedArray[UInt] = [ // pre calculated
  0x79cc4519, 0xf3988a32, 0xe7311465, 0xce6228cb, 0x9cc45197, 0x3988a32f, 0x7311465e,
  0xe6228cbc, 0xcc451979, 0x988a32f3, 0x311465e7, 0x6228cbce, 0xc451979c, 0x88a32f39,
  0x11465e73, 0x228cbce6, 0x9d8a7a87, 0x3b14f50f, 0x7629ea1e, 0xec53d43c, 0xd8a7a879,
  0xb14f50f3, 0x629ea1e7, 0xc53d43ce, 0x8a7a879d, 0x14f50f3b, 0x29ea1e76, 0x53d43cec,
  0xa7a879d8, 0x4f50f3b1, 0x9ea1e762, 0x3d43cec5, 0x7a879d8a, 0xf50f3b14, 0xea1e7629,
  0xd43cec53, 0xa879d8a7, 0x50f3b14f, 0xa1e7629e, 0x43cec53d, 0x879d8a7a, 0x0f3b14f5,
  0x1e7629ea, 0x3cec53d4, 0x79d8a7a8, 0xf3b14f50, 0xe7629ea1, 0xcec53d43, 0x9d8a7a87,
  0x3b14f50f, 0x7629ea1e, 0xec53d43c, 0xd8a7a879, 0xb14f50f3, 0x629ea1e7, 0xc53d43ce,
  0x8a7a879d, 0x14f50f3b, 0x29ea1e76, 0x53d43cec, 0xa7a879d8, 0x4f50f3b1, 0x9ea1e762,
  0x3d43cec5,
]

// auxiliary functions
// FF_j = | X xor Y xor Z                       where 0 <= j <= 15
//        | (X and Y) or (X and Z) or (Y and Z) where 16 <= j <=63

///|
fn SM3::ff_0(x : UInt, y : UInt, z : UInt) -> UInt {
  x ^ y ^ z
}

///|
fn SM3::ff_1(x : UInt, y : UInt, z : UInt) -> UInt {
  (x & y) | (x & z) | (y & z)
}

// GG_j = | X xor Y xor Z           where 0 <= j <= 15
//        | (X and Y) or (~X and Z) where 16 <= j <= 63

///|
fn SM3::gg_0(x : UInt, y : UInt, z : UInt) -> UInt {
  x ^ y ^ z
}

///|
fn SM3::gg_1(x : UInt, y : UInt, z : UInt) -> UInt {
  ((y ^ z) & x) ^ z // equivalent of (x & y) | (x.lnot() & z), but faster
}

// P_0 = X xor (X <<< 9) xor (X <<< 17)
// P_1 = X xor (X <<< 15) xor (X <<< 23)

///|
fn SM3::p_0(x : UInt) -> UInt {
  x ^ rotate_left_u(x, 9) ^ rotate_left_u(x, 17)
}

///|
fn SM3::p_1(x : UInt) -> UInt {
  x ^ rotate_left_u(x, 15) ^ rotate_left_u(x, 23)
}

///|
fn SM3::transform(data : FixedArray[Byte], reg : FixedArray[UInt]) -> Unit {
  let w_0 = FixedArray::make(68, 0U)
  let w_1 = FixedArray::make(64, 0U)
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
    w_0[index] = bytes_u8_to_u32be(data, i=4 * index)
  }
  for index = 16; index < 68; index = index + 1 {
    w_0[index] = SM3::p_1(
        w_0[index - 16] ^ w_0[index - 9] ^ rotate_left_u(w_0[index - 3], 15),
      ) ^
      rotate_left_u(w_0[index - 13], 7) ^
      w_0[index - 6]
  }
  for index = 0; index < 64; index = index + 1 {
    w_1[index] = w_0[index] ^ w_0[index + 4]
  }
  for index = 0; index < 16; index = index + 1 {
    let ss_1 = rotate_left_u(rotate_left_u(a, 12) + e + t[index], 7)
    let ss_2 = ss_1 ^ rotate_left_u(a, 12)
    let tt_1 = SM3::ff_0(a, b, c) + d + ss_2 + w_1[index]
    let tt_2 = SM3::gg_0(e, f, g) + h + ss_1 + w_0[index]
    d = c
    c = rotate_left_u(b, 9)
    b = a
    a = tt_1
    h = g
    g = rotate_left_u(f, 19)
    f = e
    e = SM3::p_0(tt_2)
  }
  for index = 16; index < 64; index = index + 1 {
    let ss_1 = rotate_left_u(rotate_left_u(a, 12) + e + t[index], 7)
    let ss_2 = ss_1 ^ rotate_left_u(a, 12)
    let tt_1 = SM3::ff_1(a, b, c) + d + ss_2 + w_1[index]
    let tt_2 = SM3::gg_1(e, f, g) + h + ss_1 + w_0[index]
    d = c
    c = rotate_left_u(b, 9)
    b = a
    a = tt_1
    h = g
    g = rotate_left_u(f, 19)
    f = e
    e = SM3::p_0(tt_2)
  }
  reg.unsafe_set(0, reg.unsafe_get(0) ^ a)
  reg.unsafe_set(1, reg.unsafe_get(1) ^ b)
  reg.unsafe_set(2, reg.unsafe_get(2) ^ c)
  reg.unsafe_set(3, reg.unsafe_get(3) ^ d)
  reg.unsafe_set(4, reg.unsafe_get(4) ^ e)
  reg.unsafe_set(5, reg.unsafe_get(5) ^ f)
  reg.unsafe_set(6, reg.unsafe_get(6) ^ g)
  reg.unsafe_set(7, reg.unsafe_get(7) ^ h)
}

///|
pub fn SM3::update_from_iter(self : SM3, data : Iter[Byte]) -> Unit {
  data.each(fn(b) {
    self.buf[self.buf_index] = b
    self.buf_index += 1
    if self.buf_index == 64 {
      self.buf_index = 0
      self.len += 512UL
      SM3::transform(self.buf, self.reg)
    }
  })
}

///|
pub impl CryptoHasher for SM3 with update(self : SM3, data : BytesView) -> Unit {
  self.update(data)
}

///|
/// update the state of given context from new `data` 
pub fn[Data : ByteSource] SM3::update(self : SM3, data : Data) -> Unit {
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
      SM3::transform(self.buf, self.reg)
    }
    offset += min_len
  }
}

///|
pub fn SM3::finalize(self : SM3) -> FixedArray[Byte] {
  let ret = FixedArray::make(32, Byte::default())
  self._finalize_into(ret)
  ret
}

///|
fn SM3::_finalize_into(
  self : SM3,
  buffer : FixedArray[Byte],
  offset? : Int = 0,
) -> Unit {
  // Copy data
  let data : FixedArray[Byte] = FixedArray::make(64, 0)
  let mut cnt = self.buf_index
  self.buf.blit_to(data, len=cnt)
  let len = self.len + 8 * cnt.to_uint64()
  let reg = self.reg.copy()

  // Padding
  data[cnt] = b'\x80'
  cnt += 1
  if cnt > 56 {
    SM3::transform(data, reg)
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
  SM3::transform(data, reg)

  // Write result to buffer
  arr_u32_to_u8be_into(reg.iter(), buffer, offset)
}

///|
pub impl CryptoHasher for SM3 with size(_self : SM3) -> Int {
  32
}

///|
pub impl CryptoHasher for SM3 with block_size(_self : SM3) -> Int {
  64
}

///|
pub impl CryptoHasher for SM3 with reset(self : SM3) -> Unit {
  self.reg[0] = 0x7380166F
  self.reg[1] = 0x4914B2B9
  self.reg[2] = 0x172442D7
  self.reg[3] = 0xDA8A0600
  self.reg[4] = 0xA96F30BC
  self.reg[5] = 0x163138AA
  self.reg[6] = 0xE38DEE4D
  self.reg[7] = 0xB0FB0E4E
  self.len = 0
  self.buf.fill(0)
  self.buf_index = 0
}

///|
/// Compute the SM3 digest from given SM3Context
pub impl CryptoHasher for SM3 with finalize_into(
  self : SM3,
  buffer : FixedArray[Byte],
  offset~ : Int,
) -> Unit {
  self._finalize_into(buffer, offset~)
}

///|
/// Compute the SM3 digest in `FixedArray[Byte]` of some `data`. Note that SM3 is big-endian.
pub fn[Data : ByteSource] sm3(data : Data) -> FixedArray[Byte] {
  SM3::new()..update(data).finalize()
}

///|
pub fn sm3_from_iter(data : Iter[Byte]) -> FixedArray[Byte] {
  SM3::new()..update_from_iter(data).finalize()
}

///|
test {
  inspect(
    bytes_to_hex_string(
      sm3(
        b"\x61\x62\x63", // abc in utf-8
      ),
    ),
    content="66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0",
  )
  inspect(
    bytes_to_hex_string(
      sm3(
        // abcd * 16 in utf-8
        b"\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64",
      ),
    ),
    content="debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732",
  )
  let hash1 = "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0"
  let ctx = SM3::new()
  ctx.update(b"\x61".to_fixedarray())
  ctx.update(b"\x62".to_fixedarray())
  ctx.update(b"\x63".to_fixedarray())
  assert_eq(hash1, bytes_to_hex_string(ctx.finalize()))
  let ctx = SM3::new()
  let data = b"\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64"
  for i = 0; i < data.length(); i = i + 1 {
    ctx.update(FixedArray::make(1, data[i]))
  }
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732",
  )
  let ctx = SM3::new()
  for i = 0; i < data.length(); i = i + 4 {
    ctx.update_from_iter(b"\x61\x62\x63\x64".iter())
  }
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732",
  )
}

///|
test "sm3 reentry" {
  let string = b"abcd"
  let ctx = SM3::new()
  ctx.update(string)
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="82ec580fe6d36ae4f81cae3c73f4a5b3b5a09c943172dc9053c69fd8e18dca1e",
  )
  ctx.update(string)
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="b58b85b795b34879c354428f7c78cd1486c4ef25ea4c5d68e611ff41c15731ef",
  )
  ctx.update(string)
  inspect(
    bytes_to_hex_string(ctx.finalize()),
    content="fd959b2560dadd0c0839144be6090cb665915156179c1fa6dc00292da7a2b9c2",
  )
}
