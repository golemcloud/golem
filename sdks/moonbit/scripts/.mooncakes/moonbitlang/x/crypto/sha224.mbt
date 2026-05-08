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
pub fn[Data : ByteSource] sha224(data : Data) -> FixedArray[Byte] {
  let ret = FixedArray::make(28, Byte::default())
  SHA256::new(reg=[
    0xc1059ed8, 0x367cd507, 0x3070dd17, 0xf70e5939, 0xffc00b31, 0x68581511, 0x64f98fa7,
    0xbefa4fa4,
  ])
  ..update(data)
  .._finalize_into(ret, size=7)
  ret
}

///|
pub fn sha224_from_iter(data : Iter[Byte]) -> FixedArray[Byte] {
  let ret = FixedArray::make(28, Byte::default())
  SHA256::new(reg=[
    0xc1059ed8, 0x367cd507, 0x3070dd17, 0xf70e5939, 0xffc00b31, 0x68581511, 0x64f98fa7,
    0xbefa4fa4,
  ])
  ..update_from_iter(data)
  .._finalize_into(ret, size=7)
  ret
}

///|
test {
  // Sha224
  assert_eq(
    bytes_to_hex_string(sha224(b"\x61\x62\x63".to_fixedarray())),
    "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7",
  )
  assert_eq(
    bytes_to_hex_string(
      sha224(
        b"\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64\x61\x62\x63\x64".to_fixedarray(),
      ),
    ),
    "fffd3b5e90c6fc3cb77c3d04b1f7660b16a426ef6655384e8486ae1f",
  )
}
