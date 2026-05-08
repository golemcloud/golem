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
fn internal_get_cli_args() -> FixedArray[Bytes] = "$moonbit.get_cli_args"

///|
pub fn get_cli_args() -> Array[String] {
  Array::from_fixed_array(
    internal_get_cli_args().map(
      // Here we assume that the CLI arguments are encoded in well-formed UTF-8.
      // TODO: Handle other encodings.
      arg => @common_ffi.utf8_bytes_to_mbt_string(arg),
    ),
  )
}

///|
pub fn get_env_var(key : String) -> String? {
  let key_bytes = @common_ffi.mbt_string_to_utf8_bytes(key, true)
  if get_env_var_exists_ffi(key_bytes) {
    Some(@common_ffi.utf8_bytes_to_mbt_string(get_env_var_ffi(key_bytes)))
  } else {
    None
  }
}

///|
#borrow(_key)
extern "C" fn get_env_var_ffi(_key : Bytes) -> Bytes = "get_env_var"

///|
#borrow(_key)
extern "C" fn get_env_var_exists_ffi(_key : Bytes) -> Bool = "get_env_var_exists"

///|
pub fn get_env_vars() -> Map[String, String] {
  let env_vars = get_env_vars_ffi().map(env => @common_ffi.utf8_bytes_to_mbt_string(
    env,
  ))
  let res = {}
  for i = 0; i < env_vars.length(); i = i + 2 {
    res[env_vars[i]] = env_vars[i + 1]
  }
  res
}

///|
extern "C" fn get_env_vars_ffi() -> FixedArray[Bytes] = "get_env_vars"

///|
pub fn set_env_var(key : String, value : String) -> Unit {
  set_env_var_ffi(
    @common_ffi.mbt_string_to_utf8_bytes(key, true),
    @common_ffi.mbt_string_to_utf8_bytes(value, true),
  )
}

///|
#borrow(_key, _value)
extern "C" fn set_env_var_ffi(_key : Bytes, _value : Bytes) -> Unit = "set_env_var"

///|
pub fn unset_env_var(key : String) -> Unit {
  unset_env_var_ffi(@common_ffi.mbt_string_to_utf8_bytes(key, true))
}

///|
#borrow(_key)
extern "C" fn unset_env_var_ffi(_key : Bytes) -> Unit = "unset_env_var"

///|
pub extern "C" fn exit(code : Int) = "exit"
