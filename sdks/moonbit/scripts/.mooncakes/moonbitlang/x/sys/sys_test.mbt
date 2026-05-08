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
test "get_env_vars" {
  let (k, v) = ("THIS_IS_A_TEST_FOR_ENV_VAR", "IT_WORKS")
  inspect(
    @sys.get_env_vars().get(k),
    content=(
      #|None
    ),
  )
  inspect(
    @sys.get_env_var(k),
    content=(
      #|None
    ),
  )
  @sys.set_env_var(k, "") // set to empty string first
  inspect(
    @sys.get_env_vars().get(k),
    content=(
      #|Some("")
    ),
  )
  @sys.set_env_var(k, v)
  inspect(
    @sys.get_env_vars().get(k),
    content=(
      #|Some("IT_WORKS")
    ),
  )
  inspect(
    @sys.get_env_var(k),
    content=(
      #|Some("IT_WORKS")
    ),
  )
  @sys.unset_env_var(k)
  inspect(
    @sys.get_env_vars().get(k),
    content=(
      #|None
    ),
  )
  inspect(
    @sys.get_env_var(k),
    content=(
      #|None
    ),
  )
}
