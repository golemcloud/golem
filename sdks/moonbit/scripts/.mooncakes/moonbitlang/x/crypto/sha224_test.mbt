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
test "sha224-additional" {
  assert_eq(
    sha224test(""),
    "d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f",
  )
  assert_eq(
    sha224test("a"),
    "3118199937a95dd0dd06a74ac0bf1517e958f08ae87ef9d7e89f139a",
  )
  assert_eq(
    sha224test("message digest"),
    "dcf41f0075ccc16b44cc9f6d1900a34bbc2841893eaf4bf2f64edf95",
  )
  assert_eq(
    sha224test("abcdefghijklmnopqrstuvwxyz"),
    "1c3ace1531530751e862592a3fe33e682d7e36c2f9df58a384eb49bd",
  )
  assert_eq(
    sha224test("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"),
    "eefa179ecbcba0011f77a6ff43815e8b3a92932456b373ed69b639cb",
  )
  assert_eq(
    sha224test(
      "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
    ),
    "5d0b10a347de663745ae351d996e7d830daaa06379992a6017c9c99e",
  )
  assert_eq(
    sha224test(
      "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.",
    ),
    "f029eeb25a211521aeb144cb1b9ad38d2313a8312d13599ca6a8f39a",
  )
}
