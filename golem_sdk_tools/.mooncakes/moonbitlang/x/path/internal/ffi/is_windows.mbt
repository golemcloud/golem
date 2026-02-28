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
#cfg(target="js")
pub extern "js" fn is_windows() -> Bool =
  #|function () {
  #|  let process = require("process");
  #|  return process.platform === "win32"; 
  #|};
  #|

///|
#cfg(any(target="wasm", target="wasm-gc"))
pub fn is_windows() -> Bool = "__moonbit_sys_unstable" "is_windows"

///|
#cfg(target="native")
pub extern "C" fn is_windows() -> Bool = "moonbitlang_x_path_is_windows"
