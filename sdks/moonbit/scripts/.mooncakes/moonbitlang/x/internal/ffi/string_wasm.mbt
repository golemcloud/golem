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
pub(all) type XStringCreateHandle

///|
#external
pub(all) type XStringReadHandle

///|
#external
pub(all) type XExternString

///|
fn begin_create_string() -> XStringCreateHandle = "__moonbit_fs_unstable" "begin_create_string"

///|
fn string_append_char(handle : XStringCreateHandle, ch : Char) = "__moonbit_fs_unstable" "string_append_char"

///|
fn finish_create_string(handle : XStringCreateHandle) -> XExternString = "__moonbit_fs_unstable" "finish_create_string"

///|
pub fn string_to_extern(s : String) -> XExternString {
  let handle = begin_create_string()
  for i = 0; i < s.length(); i = i + 1 {
    string_append_char(handle, UInt16::unsafe_to_char(s.code_unit_at(i)))
  }
  finish_create_string(handle)
}

///|
fn begin_read_string(s : XExternString) -> XStringReadHandle = "__moonbit_fs_unstable" "begin_read_string"

///|
/// Read one char from the string, returns -1 if the end of the string is reached.
/// The number returned is the unicode codepoint of the character.
fn string_read_char(handle : XStringReadHandle) -> Int = "__moonbit_fs_unstable" "string_read_char"

///|
fn finish_read_string(handle : XStringReadHandle) = "__moonbit_fs_unstable" "finish_read_string"

///|
pub fn string_from_extern(e : XExternString) -> String {
  let buf = StringBuilder::new()
  let handle = begin_read_string(e)
  while true {
    let ch = string_read_char(handle)
    if ch == -1 {
      break
    } else {
      buf.write_char(Int::unsafe_to_char(ch))
    }
  }
  finish_read_string(handle)
  buf.to_string()
}
