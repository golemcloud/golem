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
fn sm3test(s : String) -> String {
  @crypto.bytes_to_hex_string(@crypto.sm3(@encoding.encode(UTF16, s)))
}

// additional

///|
test "sm3-additional" {
  assert_eq(
    sm3test(""),
    "1ab21d8355cfa17f8e61194831e81a8f22bec8c728fefb747ed035eb5082aa2b",
  )
  assert_eq(
    sm3test("a"),
    "f4ccf00ef22555dd42706fd542022232a726a16062134c3c0ffe8fc7fa6cfe83",
  )
  assert_eq(
    sm3test("message digest"),
    "2f2fba46cd6817494d1451bb2a9e9a82d8c60aa355e8e52889b5f0dd32492f7f",
  )
  assert_eq(
    sm3test("abcdefghijklmnopqrstuvwxyz"),
    "0470a95f1d8c774444b3d25b23fc064ac928909ddc1dd4766ca9d5e0c6210ba3",
  )
  assert_eq(
    sm3test("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
    "829b71880a21666e5a415d21f7f9fa10b5a8e7ad31f9e013fac1b849820c538b",
  )
  assert_eq(
    sm3test(
      "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
    ),
    "e9ccf13e608c6a60e2677b932e3713e6bb146b3096aa20a3542ee694e7052474",
  )
  assert_eq(
    sm3test(
      "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.",
    ),
    "2bc9534464a997501e6e319279df0801ee45d9c53c02c7315df19a4c3af59ac4",
  )
}
