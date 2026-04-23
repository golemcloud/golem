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
/// Gets the command line arguments passed to the program.
///
/// This function returns `argv[0]` as the name of the program, and the rest are
/// the other arguments passed to the program (C-style `argv` list). This list
/// of arguments does not contain the runtime used to run the program, nor any
/// arguments meant to be consumed by the runtime.
pub fn get_cli_args() -> Array[String] {
  @ffi.get_cli_args()
}

///|
/// Gets the environment variables of the current process.
pub fn get_env_vars() -> Map[String, String] {
  @ffi.get_env_vars()
}

///|
/// Gets the environment variable of specific key of the current process.
/// If the environment variable is not set or exists, returns `None`.
pub fn get_env_var(key : String) -> String? {
  @ffi.get_env_var(key)
}

///|
/// Sets an environment variable.
pub fn set_env_var(key : String, value : String) -> Unit {
  @ffi.set_env_var(key, value)
}

///|
/// Unsets an environment variable.
pub fn unset_env_var(key : String) -> Unit {
  @ffi.unset_env_var(key)
}

///|
/// Exits the program immediately with the given exit code.
pub fn exit(code : Int) -> Unit {
  @ffi.exit(code)
}
