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
test "hmac-sha256 short key" {
  let h = SHA256::new()
  let key = FixedArray::make(2, b'\x88').unsafe_reinterpret_as_bytes()
  let message = b"\x01\x02\x03"
  inspect(
    bytes_to_hex_string(hmac(h, key, message)),
    content="a4516484974d883f34f4ea0459359ca09019bd07409ead28c9785808acb5962b",
  )
}

///|
test "hmac-sha256 fit key" {
  let h = SHA256::new()
  let key = FixedArray::make(64, b'\x88').unsafe_reinterpret_as_bytes()
  let message = b"\x01\x02\x03"
  inspect(
    bytes_to_hex_string(hmac(h, key, message)),
    content="8ad8d49308ad8aeed002385f143de8a3f46a0de0feb15740c82eae99bdf36f89",
  )
}

///|
test "hmac-sha256 long key" {
  let h = SHA256::new()
  let key = FixedArray::make(128, b'\x88').unsafe_reinterpret_as_bytes()
  let message = b"\x01\x02\x03"
  inspect(
    bytes_to_hex_string(hmac(h, key, message)),
    content="7b50707a53d9ee7682f5527e061ec784ac80f0b95696b314030d3a08d109d960",
  )
}

///|
test "hmac-md5" {
  let h = MD5::new()
  let key = b"\x88\x88"
  let message = b"\x01\x02\x03"
  inspect(
    bytes_to_hex_string(hmac(h, key, message)),
    content="2df61055719493098ea2c97cdd656367",
  )
}

///|
test "hmac-sm3" {
  let h = SM3::new()
  let key = b"\x88\x88"
  let message = b"\x01\x02\x03"
  inspect(
    bytes_to_hex_string(hmac(h, key, message)),
    content="c4ffa25e4474ca5d732126bf5a0e87dfb83f697fdaddd0e40fdf087096b10724",
  )
}
