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
fn __ci_is_windows() -> Bool {
  let env = @sys.get_env_vars()
  match env.get("OS_PLATFORM") {
    Some("windows-latest") => true
    None => false
    Some(_) => false
  }
}

///|
test "is_windows platform detection" {
  let expected = __ci_is_windows()
  let actual = @ffi.is_windows()
  assert_eq(actual, expected)
}
