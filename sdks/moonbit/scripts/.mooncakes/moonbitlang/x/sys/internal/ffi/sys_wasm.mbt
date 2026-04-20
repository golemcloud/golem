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
using @common_ffi {type XExternStringArray}

///|
using @common_ffi {type XExternString}

///|
pub fn get_cli_args() -> Array[String] {
  let args = get_cli_args_ffi()
  @common_ffi.string_array_from_extern(args)
}

///|
fn get_cli_args_ffi() -> XExternStringArray = "__moonbit_fs_unstable" "args_get"

///|
pub fn get_env_var(key : String) -> String? {
  let key = @common_ffi.string_to_extern(key)
  if get_env_var_exists_ffi(key) {
    let val = get_env_var_ffi(key)
    Some(@common_ffi.string_from_extern(val))
  } else {
    None
  }
}

///|
fn get_env_var_ffi(key : XExternString) -> XExternString = "__moonbit_fs_unstable" "get_env_var"

///|
fn get_env_var_exists_ffi(key : XExternString) -> Bool = "__moonbit_fs_unstable" "get_env_var_exists"

///|
pub fn get_env_vars() -> Map[String, String] {
  let env = get_env_vars_ffi()
  let tmp = @common_ffi.string_array_from_extern(env)
  let res = {}
  for i = 0; i < tmp.length(); i = i + 2 {
    res[tmp[i]] = tmp[i + 1]
  }
  res
}

///|
fn get_env_vars_ffi() -> XExternStringArray = "__moonbit_fs_unstable" "get_env_vars"

///|
pub fn set_env_var(key : String, value : String) -> Unit {
  let key = @common_ffi.string_to_extern(key)
  let value = @common_ffi.string_to_extern(value)
  set_env_var_ffi(key, value)
}

///|
fn set_env_var_ffi(key : XExternString, value : XExternString) = "__moonbit_fs_unstable" "set_env_var"

///|
pub fn unset_env_var(key : String) -> Unit {
  let key = @common_ffi.string_to_extern(key)
  unset_env_var_ffi(key)
}

///|
fn unset_env_var_ffi(key : XExternString) = "__moonbit_fs_unstable" "unset_env_var"

///|
pub fn exit(code : Int) = "__moonbit_sys_unstable" "exit"
