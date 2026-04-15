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
pub(all) type XByteArrayCreateHandle

///|
#external
pub(all) type XByteArrayReadHandle

///|
#external
pub(all) type XExternByteArray

///|
fn begin_read_byte_array(s : XExternByteArray) -> XByteArrayReadHandle = "__moonbit_fs_unstable" "begin_read_byte_array"

///|
fn byte_array_read_byte(handle : XByteArrayReadHandle) -> Int = "__moonbit_fs_unstable" "byte_array_read_byte"

///|
fn finish_read_byte_array(handle : XByteArrayReadHandle) = "__moonbit_fs_unstable" "finish_read_byte_array"

///|
fn begin_create_byte_array() -> XByteArrayCreateHandle = "__moonbit_fs_unstable" "begin_create_byte_array"

///|
fn byte_array_append_byte(handle : XByteArrayCreateHandle, ch : Int) = "__moonbit_fs_unstable" "byte_array_append_byte"

///|
fn finish_create_byte_array(
  handle : XByteArrayCreateHandle,
) -> XExternByteArray = "__moonbit_fs_unstable" "finish_create_byte_array"

///|
pub fn byte_array_to_extern(s : Bytes) -> XExternByteArray {
  let handle = begin_create_byte_array()
  for i = 0; i < s.length(); i = i + 1 {
    byte_array_append_byte(handle, s[i].to_int())
  }
  finish_create_byte_array(handle)
}

///|
pub fn byte_array_from_extern(e : XExternByteArray) -> Bytes {
  let buf = Array::new()
  let handle = begin_read_byte_array(e)
  while true {
    let ch = byte_array_read_byte(handle)
    if ch == -1 {
      break
    } else {
      buf.push(ch.to_byte())
    }
  }
  finish_read_byte_array(handle)
  Bytes::from_array(buf)
}
