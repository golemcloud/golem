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
fn md5test(s : String) -> String {
  bytes_to_hex_string(@crypto.md5(@encoding.encode(UTF16, s)))
}

///|
test "md5_simple" {
  let hash = md5(
    @encoding.encode(UTF16, "The quick brown fox jumps over the lazy dog"),
  )
  inspect(bytes_to_hex_string(hash), content="b0986ae6ee1eefee8a4a399090126837")
}

///|
test "md5_rfc1321" { // testsuites in RFC1321
  let hash = md5test("")
  inspect(hash, content="d41d8cd98f00b204e9800998ecf8427e")
  let hash = md5test("a")
  inspect(hash, content="4144e195f46de78a3623da7364d04f11")
  let hash = md5test("abc")
  inspect(hash, content="ce1473cf80c6b3fda8e3dfc006adc315")
  let hash = md5test("message digest")
  inspect(hash, content="6f9ab83227f65f9b86c380e2c9c33031")
  let hash = md5test("abcdefghijklmnopqrstuvwxyz")
  inspect(hash, content="35020d67a52d8e915330f0a77f676bbf")
  let hash = md5test(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
  )
  inspect(hash, content="86056d805455c8448f6c09404c3db624")
  let hash = md5test(
    "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
  )
  inspect(hash, content="903f43f5c1f384fc267110bf07caec04")
}

///|
test "md5_additional" { // Additional testsuites
  let hash = md5test(
    "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.",
  )
  inspect(hash, content="28a4570fd742bc88258e159f8773d9c8")
  let a = Array::make(1000, b'\x00')
  for index = 0; index < 1000; index = index + 1 {
    a[index] = (index % 256).to_byte()
  }
  let hash = @crypto.md5(FixedArray::from_array(a))
  inspect(bytes_to_hex_string(hash), content="cbecbdb0fdd5cec1e242493b6008cc79")
}
