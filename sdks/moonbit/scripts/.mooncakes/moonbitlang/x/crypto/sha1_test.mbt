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
test "sha1" {
  let a = "The quick brown fox jumps over the lazy dog"
  inspect(
    @crypto.bytes_to_hex_string(@crypto.sha1(@encoding.encode(UTF16, a))),
    content="bd136cb58899c93173c33a90dde95ead0d0cf6df",
  )
  let a = []
  for i = 0; i < 1000; i = i + 1 {
    a.push(i.to_byte())
  }
  inspect(
    @crypto.bytes_to_hex_string(@crypto.sha1(FixedArray::from_array(a))),
    content="af0b191c2de46fe13fe0908f5a6a4e90e0cafc46",
  )
  let a = []
  for i = 0; i < 8192; i = i + 1 {
    a.push((i / 3).to_byte())
  }
  inspect(
    @crypto.bytes_to_hex_string(@crypto.sha1(FixedArray::from_array(a))),
    content="2d3fd26de8e0e1625911bb0417005813e6130aa9",
  )
  let a = []
  for i = 0; i < 8190; i = i + 1 {
    a.push((i / 5).to_byte())
  }
  inspect(
    @crypto.bytes_to_hex_string(@crypto.sha1(FixedArray::from_array(a))),
    content="c3be02b5a0cdbd6bd436159d860dfa99570f8efa",
  )
  let a = []
  for i = 0; i < 8182; i = i + 1 {
    a.push((i / 7).to_byte())
  }
  inspect(
    @crypto.bytes_to_hex_string(@crypto.sha1(FixedArray::from_array(a))),
    content="64ee7b834ef006980ead69a2ea09ce8c0194e728",
  )
  let a = []
  for i = 0; i < 8172; i = i + 1 {
    a.push((i / 11).to_byte())
  }
  inspect(
    @crypto.bytes_to_hex_string(@crypto.sha1(FixedArray::from_array(a))),
    content="f27a48085ff2d00692599510e7627f3ce468c1a6",
  )
}
