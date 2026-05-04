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
fn sha256test(s : String) -> String {
  @crypto.bytes_to_hex_string(@crypto.sha256(@encoding.encode(UTF16, s)))
}

///|
fn sha224test(s : String) -> String {
  @crypto.bytes_to_hex_string(@crypto.sha224(@encoding.encode(UTF16, s)))
}

// additional

///|
test "sha256-additional" {
  assert_eq(
    sha256test(""),
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  )
  assert_eq(
    sha256test("a"),
    "ffe9aaeaa2a2d5048174df0b80599ef0197ec024c4b051bc9860cff58ef7f9f3",
  )
  assert_eq(
    sha256test("message digest"),
    "44911ad1e18f35c0ee9c80582d1aa66c00a18f34a188676ed5f0dc94d05a4fd7",
  )
  assert_eq(
    sha256test("abcdefghijklmnopqrstuvwxyz"),
    "d79ca8ca68cac4c2d29a4167295303d2a7cf6caae18d1d71bc7566fcb29b5152",
  )
  assert_eq(
    sha256test("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
    "118c908d563bbe9c5b40ad59772cfa520e25dd4299860a59d3280f00e85d4d9d",
  )
  assert_eq(
    sha256test(
      "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
    ),
    "fb680c13708176267a6b3a9d8ae3691ea6ae30ba0f1c7fc12680bf47618abc7a",
  )
  assert_eq(
    sha256test(
      "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.",
    ),
    "0a53e29c821b12fe15c6ea83584079bab3c7b92f3805255b4c0b41c1ef12b6b8",
  )
}
