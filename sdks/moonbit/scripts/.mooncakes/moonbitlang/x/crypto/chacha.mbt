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
fn flipWord(word : UInt) -> UInt {
  let b0 = (word & 0xff) << 24
  let b1 = (word & 0xff00) << 8
  let b2 = (word & 0xff0000) >> 8
  let b3 = (word & 0xff000000) >> 24
  b0 | b1 | b2 | b3
}

///|
test "flipWord" {
  let word = 0x12345678U
  let flipped = flipWord(word)
  inspect(flipped, content="2018915346")
}

///|
fn FixedArray::quarterRound(
  state : FixedArray[UInt],
  w : Int,
  x : Int,
  y : Int,
  z : Int,
) -> Unit {
  let mut a = state[w]
  let mut b = state[x]
  let mut c = state[y]
  let mut d = state[z]
  a += b
  d = d ^ a
  d = rotate_left_u(d, 16)
  c += d
  b = b ^ c
  b = rotate_left_u(b, 12)
  a += b
  d = d ^ a
  d = rotate_left_u(d, 8)
  c += d
  b = b ^ c
  b = rotate_left_u(b, 7)
  state[w] = a
  state[x] = b
  state[y] = c
  state[z] = d
}

///|
test "quarterRound" {
  let state = FixedArray::make(16, 0U)
  state[0] = 0x879531e0U
  state[1] = 0xc5ecf37dU
  state[2] = 0x516461b1U
  state[3] = 0xc9a62f8aU
  state[4] = 0x44c20ef3U
  state[5] = 0x3390af7fU
  state[6] = 0xd9fc690bU
  state[7] = 0x2a5f714cU
  state[8] = 0x53372767U
  state[9] = 0xb00a5631U
  state[10] = 0x974c541aU
  state[11] = 0x359e9963U
  state[12] = 0x5c971061U
  state[13] = 0x3d631689U
  state[14] = 0x2098d9d6U
  state[15] = 0x91dbd320U
  state.quarterRound(2, 7, 8, 13)
  inspect(
    state,
    content="[2274701792, 3320640381, 3182986972, 3383111562, 1153568499, 865120127, 3657197835, 3484200914, 3832277632, 2953467441, 2538361882, 899586403, 1553404001, 3435166841, 546888150, 2447102752]",
  )
}

///|
fn FixedArray::chachaBlockRound(state : FixedArray[UInt]) -> Unit {
  state
  ..quarterRound(0, 4, 8, 12)
  ..quarterRound(1, 5, 9, 13)
  ..quarterRound(2, 6, 10, 14)
  ..quarterRound(3, 7, 11, 15)
  ..quarterRound(0, 5, 10, 15)
  ..quarterRound(1, 6, 11, 12)
  ..quarterRound(2, 7, 8, 13)
  ..quarterRound(3, 4, 9, 14)
}

///|
test "chachaBlockRound" {
  let state = FixedArray::make(16, 0U)
  state[0] = 0x61707865U
  state[1] = 0x3320646eU
  state[2] = 0x79622d32U
  state[3] = 0x6b206574U
  state[4] = 0x03020100U
  state[5] = 0x07060504U
  state[6] = 0x0b0a0908U
  state[7] = 0x0f0e0d0cU
  state[8] = 0x13121110U
  state[9] = 0x17161514U
  state[10] = 0x1b1a1918U
  state[11] = 0x1f1e1d1cU
  state[12] = 0x00000001U
  state[13] = 0x00000000U
  state[14] = 0x00000000U
  state[15] = 0x00000000U
  state.chachaBlockRound()
  inspect(
    state,
    content="[986087425, 3489031050, 2890662805, 2683391196, 1720476390, 1116253759, 2262580386, 3212003942, 2202368212, 756352536, 496298475, 669838588, 567302638, 1860562437, 1434237441, 2097484794]",
  )
}

///|
fn FixedArray::chachaBlockLoop(state : FixedArray[UInt], n : UInt) -> Unit {
  for i in 0U..<n {
    state.chachaBlockRound()
  }
}

///|
test "chachaBlockLoop" {
  let count = 1U
  let key = FixedArray::make(8, 0U)
  key[0] = 0x00010203U
  key[1] = 0x04050607U
  key[2] = 0x08090a0bU
  key[3] = 0x0c0d0e0fU
  key[4] = 0x10111213U
  key[5] = 0x14151617U
  key[6] = 0x18191a1bU
  key[7] = 0x1c1d1e1fU
  let state = FixedArray::make(16, 0U)
  state[0] = 0X61707865U
  state[1] = 0X3320646eU
  state[2] = 0X79622d32U
  state[3] = 0X6b206574U
  state[4] = flipWord(key[0])
  state[5] = flipWord(key[1])
  state[6] = flipWord(key[2])
  state[7] = flipWord(key[3])
  state[8] = flipWord(key[4])
  state[9] = flipWord(key[5])
  state[10] = flipWord(key[6])
  state[11] = flipWord(key[7])
  state[12] = count
  state[13] = 0
  state[14] = 0
  state[15] = 0
  inspect(
    state,
    content="[1634760805, 857760878, 2036477234, 1797285236, 50462976, 117835012, 185207048, 252579084, 319951120, 387323156, 454695192, 522067228, 1, 0, 0, 0]",
  )
  state.chachaBlockLoop(4)
  inspect(
    state,
    content="[2919080465, 647515738, 898727107, 777299107, 3407982512, 2489307765, 745530666, 2053399858, 1994399329, 139328223, 3709168053, 3118545354, 4170274417, 867477305, 1393604261, 3769539545]",
  )
}

///|
/// - chacha8: round = 4
/// - chacha12: round = 6
/// - chacha20: round = 10
fn chachaBlock(
  key : FixedArray[UInt],
  count : UInt,
  nonce : FixedArray[UInt],
  round : UInt,
  state : FixedArray[UInt],
) -> Unit {
  guard key.length() == 8
  guard nonce.length() == 3
  guard state.length() == 16
  let initial_state = FixedArray::make(16, 0U)
  initial_state[0] = 0X61707865U
  initial_state[1] = 0X3320646eU
  initial_state[2] = 0X79622d32U
  initial_state[3] = 0X6b206574U
  key.blit_to(initial_state, len=8, dst_offset=4)
  initial_state[12] = count
  nonce.blit_to(initial_state, len=3, dst_offset=13)
  initial_state.blit_to(state, len=16)
  state.chachaBlockLoop(round)
  for i in 0..<16 {
    state[i] += initial_state[i]
  }
}

///|
test "chachaBlock" {
  let key = FixedArray::make(8, 0U)
  key[0] = flipWord(0x00010203U)
  key[1] = flipWord(0x04050607U)
  key[2] = flipWord(0x08090a0bU)
  key[3] = flipWord(0x0c0d0e0fU)
  key[4] = flipWord(0x10111213U)
  key[5] = flipWord(0x14151617U)
  key[6] = flipWord(0x18191a1bU)
  key[7] = flipWord(0x1c1d1e1fU)
  let keyStream = FixedArray::make(16, 0U)
  chachaBlock(key, 1, [0, 0, 0], 4, keyStream)
  inspect(
    keyStream.map(flipWord),
    content="[1981443599, 3367155801, 4121555886, 386561433, 2964333518, 2044159387, 854489399, 1047687817, 1898967689, 4077872159, 3447861240, 3864461272, 1918276088, 967291955, 2780172371, 3650858720]",
  )

  // chachaBlock with different count and nonce
  chachaBlock(key, 2, [1, 1, 1], 6, keyStream)
  inspect(
    keyStream.map(flipWord),
    content="[3045418931, 1870601267, 1791152559, 1128609701, 2412731038, 2282963828, 1154847987, 1925524179, 4130821097, 3948066013, 1837610591, 4125132907, 3665873803, 4179097700, 1171003195, 102570666]",
  )
  chachaBlock(key, 2, [10, 10, 10], 6, keyStream)
  inspect(
    keyStream.map(flipWord),
    content="[1655341353, 1070113534, 1698503531, 4049551328, 4260831884, 3845590894, 3707238589, 4243437253, 44481274, 2151594893, 3326073229, 3492192335, 3505455555, 2232294460, 127722793, 1386042532]",
  )
}

///|
fn stateToBytes(state : FixedArray[UInt]) -> FixedArray[Byte] {
  let result = FixedArray::make(4 * state.length(), b'\x00')
  for i = 0; i < 16; i = i + 1 {
    let word = state[i]
    result[i * 4 + 0] = word.to_byte()
    result[i * 4 + 1] = (word >> 8).to_byte()
    result[i * 4 + 2] = (word >> 16).to_byte()
    result[i * 4 + 3] = (word >> 24).to_byte()
  }
  result
}

///|
test "stateToBytes" {
  let state = FixedArray::make(16, 0U)
  state[0] = 0x879531e0
  state[1] = 0xc5ecf37d
  state[2] = 0x516461b1
  state[3] = 0xc9a62f8a
  state[4] = 0x44c20ef3
  state[5] = 0x3390af7f
  state[6] = 0xd9fc690b
  state[7] = 0x2a5f714c
  state[8] = 0x53372767
  state[9] = 0xb00a5631
  state[10] = 0x974c541a
  state[11] = 0x359e9963
  state[12] = 0x5c971061
  state[13] = 0x3d631689
  state[14] = 0x2098d9d6
  state[15] = 0x91dbd320
  let bytes = stateToBytes(state)
  inspect(
    bytes_to_hex_string(bytes),
    content="e03195877df3ecc5b16164518a2fa6c9f30ec2447faf90330b69fcd94c715f2a6727375331560ab01a544c9763999e356110975c8916633dd6d9982020d3db91",
  )
}

///|
/// Encrypts a block of data using the ChaCha8 algorithm.
/// - [key] must be 8 32-bit unsigned integers.
/// - [counter] is the counter value.
/// - [block] is the block of data to be encrypted.
/// - [nonce] is default to 0
/// - Returns the encrypted block of data.
#deprecated("Use ChaCha::chacha8 and ChaCha::transform instead")
pub fn[Data : ByteSource] chacha8(
  key : FixedArray[UInt],
  counter : UInt,
  block : Data,
  nonce? : UInt = 0,
) -> FixedArray[Byte] raise Error {
  if key.length() != 8 {
    fail("Invalid key length -- key must be 8 32-bit unsigned integers")
  }
  chacha(key.map(flipWord), counter, block, 4, [nonce, nonce, nonce])
}

///|
/// Encrypts a block of data using the ChaCha12 algorithm.
/// - [key] must be 8 32-bit unsigned integers.
/// - [counter] is the counter value.
/// - [block] is the block of data to be encrypted.
/// - [nonce] is default to 0
/// - Returns the encrypted block of data.
#deprecated("Use ChaCha::chacha12 and ChaCha::transform instead")
pub fn[Data : ByteSource] chacha12(
  key : FixedArray[UInt],
  counter : UInt,
  block : Data,
  nonce? : UInt = 0,
) -> FixedArray[Byte] raise Error {
  if key.length() != 8 {
    fail("Invalid key length -- key must be 8 32-bit unsigned integers")
  }
  chacha(key.map(flipWord), counter, block, 6, [nonce, nonce, nonce])
}

///|
/// Encrypts a block of data using the ChaCha20 algorithm.
/// - [key] must be 8 32-bit unsigned integers.
/// - [counter] is the counter value.
/// - [block] is the block of data to be encrypted.
/// - [nonce] is default to 0
/// - Returns the encrypted block of data.
#deprecated("Use ChaCha::chacha20 and ChaCha::transform instead")
pub fn[Data : ByteSource] chacha20(
  key : FixedArray[UInt],
  counter : UInt,
  block : Data,
  nonce? : UInt = 0,
) -> FixedArray[Byte] raise Error {
  if key.length() != 8 {
    fail("Invalid key length -- key must be 8 32-bit unsigned integers")
  }
  chacha(key.map(flipWord), counter, block, 10, [nonce, nonce, nonce])
}

///|
#coverage.skip
fn[Data : ByteSource] chacha(
  key : FixedArray[UInt],
  counter : UInt,
  block : Data,
  round : UInt,
  nonce : FixedArray[UInt],
) -> FixedArray[Byte] {
  if block.length() == 0 {
    return FixedArray::make(0, Byte::default())
  }
  let buffer = FixedArray::make(block.length(), Byte::default())
  let keyStream = FixedArray::make(16, 0U)
  for i = 0; i < block.length(); i = i + 64 {
    chachaBlock(
      key,
      counter + i.reinterpret_as_uint() / 64,
      nonce,
      round,
      keyStream,
    )
    let pad = stateToBytes(keyStream)
    let len = @cmp.minimum(block.length() - i, 64)
    for j in 0..<len {
      buffer[i + j] = pad[j] ^ block[i + j]
    }
  }
  buffer
}

///|
struct ChaCha {
  key : FixedArray[UInt]
  nonce : FixedArray[UInt]
  key_stream : FixedArray[UInt]
  mut counter : UInt
  mut offset : Int
  round : UInt
}

///|
fn[K : ByteSource, N : ByteSource] ChaCha::new(
  key : K,
  nonce : N,
  round : UInt,
  counter : UInt,
) -> ChaCha raise Error {
  if key.length() != 32 {
    fail("Invalid key length -- key must be 256 bits")
  }
  if nonce.length() != 12 {
    fail("Invalid nonce length -- nonce must be 96 bits")
  }
  let key_ = FixedArray::make(8, 0U)
  for i in 0..<8 {
    key_[i] = (key[i * 4].to_uint() << 0) |
      (key[i * 4 + 1].to_uint() << 8) |
      (key[i * 4 + 2].to_uint() << 16) |
      (key[i * 4 + 3].to_uint() << 24)
  }
  let nonce_ = FixedArray::make(3, 0U)
  for i in 0..<3 {
    nonce_[i] = nonce[i * 4].to_uint() |
      (nonce[i * 4 + 1].to_uint() << 8) |
      (nonce[i * 4 + 2].to_uint() << 16) |
      (nonce[i * 4 + 3].to_uint() << 24)
  }
  ChaCha::{
    key: key_,
    nonce: nonce_,
    key_stream: FixedArray::make(16, 0U),
    round,
    offset: 64,
    counter,
  }
}

///|
/// Creates a ChaCha8 encryption context following the RFC 8439 standard.
/// - [key] must be 256-bit (32 bytes), in little-endian order.
/// - [nonce] must be a 96-bit (12 bytes) bytes, in little-endian order.
/// - [counter] is the counter value, defaulting to 0.
///
/// raise Error if the length of key or nonce is invalid.
pub fn[K : ByteSource, N : ByteSource] ChaCha::chacha8(
  key : K,
  nonce : N,
  counter? : UInt = 0,
) -> ChaCha raise Error {
  ChaCha::new(key, nonce, 4, counter)
}

///|
/// Creates a ChaCha12 encryption context following the RFC 8439 standard.
/// - [key] must be 256-bit (32 bytes), in little-endian order.
/// - [nonce] must be a 96-bit (12 bytes) bytes, in little-endian order.
/// - [counter] is the counter value, defaulting to 0.
///
/// raise Error if the length of key or nonce is invalid.
pub fn[K : ByteSource, N : ByteSource] ChaCha::chacha12(
  key : K,
  nonce : N,
  counter? : UInt = 0,
) -> ChaCha raise Error {
  ChaCha::new(key, nonce, 6, counter)
}

///|
/// Creates a ChaCha20 encryption context following the RFC 8439 standard.
/// - [key] must be 256-bit (32 bytes), in little-endian order.
/// - [nonce] must be a 96-bit (12 bytes) bytes, in little-endian order.
/// - [counter] is the counter value, defaulting to 0.
///
/// raise Error if the length of key or nonce is invalid.
pub fn[K : ByteSource, N : ByteSource] ChaCha::chacha20(
  key : K,
  nonce : N,
  counter? : UInt = 0,
) -> ChaCha raise Error {
  ChaCha::new(key, nonce, 10, counter)
}

///|
/// Transforms the given data using the ChaCha encryption algorithm.
/// - [data] is the data to be transformed.
/// - [target] is the output buffer to store the transformed data.
/// - [offset] is the offset in the target buffer where the transformed data will be written
/// 
/// If the length of [data] is less then the length of [target] minus [offset], it will panic.
pub fn[D : ByteSource] ChaCha::transform(
  self : Self,
  data : D,
  target : FixedArray[Byte],
  offset? : Int = 0,
) -> Unit {
  for i in 0..<data.length() {
    if self.offset == 64 {
      chachaBlock(
        self.key,
        self.counter,
        self.nonce,
        self.round,
        self.key_stream,
      )
      self.offset = 0
      self.counter += 1
    }
    target[offset + i] = (self.key_stream[self.offset / 4] >>
      (self.offset % 4 * 8)).to_byte() ^
      data[i]
    self.offset += 1
  }
}
