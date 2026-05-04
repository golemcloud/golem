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
#external
pub(all) type XStringArrayReadHandle

///|
#external
pub(all) type XExternStringArray

///|
fn begin_read_string_array(sa : XExternStringArray) -> XStringArrayReadHandle = "__moonbit_fs_unstable" "begin_read_string_array"

///|
fn string_array_read_string(handle : XStringArrayReadHandle) -> XExternString = "__moonbit_fs_unstable" "string_array_read_string"

///|
fn finish_read_string_array(handle : XStringArrayReadHandle) = "__moonbit_fs_unstable" "finish_read_string_array"

///|
pub fn string_array_from_extern(e : XExternStringArray) -> Array[String] {
  let buf = Array::new()
  let handle = begin_read_string_array(e)
  while true {
    let extern_str = string_array_read_string(handle)
    let str = string_from_extern(extern_str)
    // keep "ffi_end_of_/string_array" same with moonrun
    if str == "ffi_end_of_/string_array" {
      break
    } else {
      buf.push(str)
    }
  }
  finish_read_string_array(handle)
  buf
}
