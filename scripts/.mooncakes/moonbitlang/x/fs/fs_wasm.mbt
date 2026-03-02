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
using @ffi {type XExternString}

///|
using @ffi {type XExternByteArray}

///|
using @ffi {type XExternStringArray}

///|
fn get_error_message_ffi() -> XExternString = "__moonbit_fs_unstable" "get_error_message"

///|
fn get_error_message() -> String {
  @ffi.string_from_extern(get_error_message_ffi())
}

///|
fn get_file_content_ffi() -> XExternByteArray = "__moonbit_fs_unstable" "get_file_content"

///|
fn get_dir_files_ffi() -> XExternStringArray = "__moonbit_fs_unstable" "get_dir_files"

///|
fn read_file_to_bytes_ffi(path : XExternString) -> Int = "__moonbit_fs_unstable" "read_file_to_bytes_new"

///|
fn write_bytes_to_file_ffi(
  path : XExternString,
  content : XExternByteArray,
) -> Int = "__moonbit_fs_unstable" "write_bytes_to_file_new"

///|
fn read_file_to_bytes_internal(path : String) -> Bytes raise IOError {
  let res = read_file_to_bytes_ffi(@ffi.string_to_extern(path))
  guard res != -1 else { raise IOError(get_error_message()) }
  @ffi.byte_array_from_extern(get_file_content_ffi())
}

///|
fn read_file_to_string_internal(
  path : String,
  encoding? : String = "utf8",
) -> String raise IOError {
  guard encoding == "utf8" else {
    raise IOError(
      "Unsupported encoding: \{encoding}, only utf8 is supported for now",
    )
  }
  @ffi.utf8_bytes_to_mbt_string(read_file_to_bytes_internal(path))
}

///|
fn write_bytes_to_file_internal(
  path : String,
  content : Bytes,
) -> Unit raise IOError {
  let res = write_bytes_to_file_ffi(
    @ffi.string_to_extern(path),
    @ffi.byte_array_to_extern(content),
  )
  guard res != -1 else { raise IOError(get_error_message()) }
}

///|
fn write_string_to_file_internal(
  path : String,
  content : String,
  encoding? : String = "utf8",
) -> Unit raise IOError {
  guard encoding == "utf8" else {
    raise IOError(
      "Unsupported encoding: \{encoding}, only utf8 is supported for now",
    )
  }
  write_bytes_to_file_internal(
    path,
    @ffi.mbt_string_to_utf8_bytes(content, false),
  )
}

///|
fn path_exists_ffi(path : XExternString) -> Bool = "__moonbit_fs_unstable" "path_exists"

///|
fn path_exists_internal(path : String) -> Bool {
  path_exists_ffi(@ffi.string_to_extern(path))
}

///|
fn create_dir_ffi(path : XExternString) -> Int = "__moonbit_fs_unstable" "create_dir_new"

///|
fn create_dir_internal(path : String) -> Unit raise IOError {
  let res = create_dir_ffi(@ffi.string_to_extern(path))
  guard res != -1 else { raise IOError(get_error_message()) }
}

///|
fn read_dir_ffi(path : XExternString) -> Int = "__moonbit_fs_unstable" "read_dir_new"

///|
fn read_dir_internal(path : String) -> Array[String] raise IOError {
  let res = read_dir_ffi(@ffi.string_to_extern(path))
  guard res != -1 else { raise IOError(get_error_message()) }
  @ffi.string_array_from_extern(get_dir_files_ffi())
}

///|
fn is_file_ffi(path : XExternString) -> Int = "__moonbit_fs_unstable" "is_file_new"

///|
fn is_file_internal(path : String) -> Bool raise IOError {
  let res = is_file_ffi(@ffi.string_to_extern(path))
  guard res != -1 else { raise IOError(get_error_message()) }
  res == 1
}

///|
fn is_dir_ffi(path : XExternString) -> Int = "__moonbit_fs_unstable" "is_dir_new"

///|
fn is_dir_internal(path : String) -> Bool raise IOError {
  let res = is_dir_ffi(@ffi.string_to_extern(path))
  guard res != -1 else { raise IOError(get_error_message()) }
  res == 1
}

///|
fn remove_file_ffi(path : XExternString) -> Int = "__moonbit_fs_unstable" "remove_file_new"

///|
fn remove_file_internal(path : String) -> Unit raise IOError {
  let res = remove_file_ffi(@ffi.string_to_extern(path))
  guard res != -1 else { raise IOError(get_error_message()) }
}

///|
fn remove_dir_ffi(path : XExternString) -> Int = "__moonbit_fs_unstable" "remove_dir_new"

///|
fn remove_dir_internal(path : String) -> Unit raise IOError {
  let res = remove_dir_ffi(@ffi.string_to_extern(path))
  guard res != -1 else { raise IOError(get_error_message()) }
}
