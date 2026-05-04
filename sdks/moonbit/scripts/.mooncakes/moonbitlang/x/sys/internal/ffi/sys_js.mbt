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
pub fn get_cli_args() -> Array[String] {
  get_cli_args_internal()
}

///|
extern "js" fn get_cli_args_internal() -> Array[String] =
  #| function() {
  // // node has argv[0] as the node executable
  #|  return process.argv.slice(1);
  #| }

///|
pub fn get_env_var(key : String) -> String? {
  if is_env_var_exists_internal(key) {
    Some(get_env_var_internal(key))
  } else {
    None
  }
}

///|
extern "js" fn is_env_var_exists_internal(key : String) -> Bool =
  #| function(key) {
  #|   return key in process.env;
  #| }

///|
extern "js" fn get_env_var_internal(key : String) -> String =
  #| function(key) {
  #|   const env = process.env;
  #|   return env[key] || "";
  #| }

///|
pub fn get_env_vars() -> Map[String, String] {
  let tmp = get_env_vars_internal()
  let res = {}
  for i = 0; i < tmp.length(); i = i + 2 {
    res[tmp[i]] = tmp[i + 1]
  }
  res
}

///|
extern "js" fn get_env_vars_internal() -> Array[String] =
  #| function() {
  #|   const env = process.env;
  #|   const result = [];
  #|   for (const key in env) {
  #|     result.push(key);
  #|     result.push(env[key]);
  #|   }
  #|   return result;
  #| }

///|
pub fn set_env_var(key : String, value : String) -> Unit {
  set_env_var_internal(key, value)
}

///|
extern "js" fn set_env_var_internal(key : String, value : String) -> Unit =
  #| function(key, value) {
  #|   process.env[key] = value;
  #| }

///|
pub fn unset_env_var(key : String) -> Unit {
  unset_env_var_internal(key)
}

///|
extern "js" fn unset_env_var_internal(key : String) -> Unit =
  #| function(key) {
  #|   delete process.env[key];
  #| }

///|
pub extern "js" fn exit(code : Int) -> Unit =
  #| function(code) {
  #|   process.exit(code);
  #| }
