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
test "from_hex" {
  let hex = "13cb501c-1234-5678-9034-abcdef937e20"
  inspect(@uuid.from_hex(hex), content=hex)
  inspect(
    @uuid.from_hex("12345678-1234-1234-1234-ABCDEF098765"),
    content="12345678-1234-1234-1234-abcdef098765",
  )
  assert_true((try? @uuid.from_hex("13cb")) is Err(_))
  assert_true((try? @uuid.from_hex("13cb501c-99a8-c889-3234")) is Err(_))
}

///|
test "ops" {
  let hex = "72212911-64d1-c441-e87d-de89b955ea34"
  let u1 = @uuid.from_hex(hex)
  let u1x = @uuid.from_hex(hex)
  let u2 = @uuid.from_hex("8e325b13-2bf1-d7f6-d2b0-c85a371c1878")
  inspect(u1 == u1x, content="true")
  inspect(u1.compare(u2), content="1")
  inspect(u2.compare(u1), content="-1")
  inspect(u1.compare(u1x), content="0")
  assert_eq(u1, u1x)
}
