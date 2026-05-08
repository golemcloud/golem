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
priv type Handler

///|
#borrow(path, mode)
extern "C" fn fopen_ffi(path : Bytes, mode : Bytes) -> Handler = "moonbitlang_x_fs_fopen_ffi"

///|
extern "C" fn is_null(ptr : Handler) -> Int = "moonbitlang_x_fs_is_null"

///|
#borrow(ptr)
extern "C" fn fread_ffi(
  ptr : Bytes,
  size : Int,
  nitems : Int,
  stream : Handler,
) -> Int = "moonbitlang_x_fs_fread_ffi"

///|
#borrow(ptr)
extern "C" fn fwrite_ffi(
  ptr : Bytes,
  size : Int,
  nitems : Int,
  stream : Handler,
) -> Int = "moonbitlang_x_fs_fwrite_ffi"

///|
extern "C" fn get_error_message_ffi() -> Bytes = "moonbitlang_x_fs_get_error_message"

///|
extern "C" fn fseek_ffi(file : Handler, offset : Int, whence : Int) -> Int = "moonbitlang_x_fs_fseek_ffi"

///|
extern "C" fn ftell_ffi(file : Handler) -> Int = "moonbitlang_x_fs_ftell_ffi"

///|
extern "C" fn fflush_ffi(stream : Handler) -> Int = "moonbitlang_x_fs_fflush_ffi"

///|
extern "C" fn fclose_ffi(file : Handler) -> Int = "moonbitlang_x_fs_fclose_ffi"

///|
fn get_error_message() -> String {
  @ffi.utf8_bytes_to_mbt_string(get_error_message_ffi())
}

///|
fn read_file_to_bytes_internal(path : String) -> Bytes raise IOError {
  let file = fopen_ffi(@ffi.mbt_string_to_utf8_bytes(path, true), b"rb\x00")
  guard is_null(file) == 0 else { raise IOError(get_error_message()) }
  guard fseek_ffi(file, 0, 2) == 0 else { raise IOError(get_error_message()) }
  let size = ftell_ffi(file)
  guard size != -1 else { raise IOError(get_error_message()) }
  guard fseek_ffi(file, 0, 0) == 0 else { raise IOError(get_error_message()) }
  let bytes = Bytes::make(size, 0)
  let bytes_read = fread_ffi(bytes, 1, size, file)
  guard bytes_read == size else { raise IOError(get_error_message()) }
  guard fclose_ffi(file) == 0 else { raise IOError(get_error_message()) }
  bytes
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
  let file = fopen_ffi(@ffi.mbt_string_to_utf8_bytes(path, true), b"wb\x00")
  guard is_null(file) == 0 else { raise IOError(get_error_message()) }
  let bytes_written = fwrite_ffi(content, 1, content.length(), file)
  guard bytes_written == content.length() else {
    raise IOError(get_error_message())
  }
  guard fflush_ffi(file) == 0 else { raise IOError(get_error_message()) }
  guard fclose_ffi(file) == 0 else { raise IOError(get_error_message()) }
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
  let bytes = @ffi.mbt_string_to_utf8_bytes(content, false)
  write_bytes_to_file_internal(path, bytes)
}

///|
#borrow(path)
extern "C" fn stat_ffi(path : Bytes) -> Int = "moonbitlang_x_fs_stat_ffi"

///|
fn path_exists_internal(path : String) -> Bool {
  stat_ffi(@ffi.mbt_string_to_utf8_bytes(path, true)) != -1
}

///|
#borrow(path)
extern "C" fn read_dir_ffi(path : Bytes) -> Handler = "moonbitlang_x_fs_read_dir_ffi"

///|
fn handler_to_fixed_array(handler : Handler) -> FixedArray[Bytes] = "%identity"

///|
fn read_dir_internal(path : String) -> Array[String] raise IOError {
  let res = read_dir_ffi(@ffi.mbt_string_to_utf8_bytes(path, true))
  guard is_null(res) == 0 else { raise IOError(get_error_message()) }
  let files = Array::from_fixed_array(handler_to_fixed_array(res))
  files.map(@ffi.utf8_bytes_to_mbt_string)
}

///|
#borrow(path)
extern "C" fn create_dir_ffi(path : Bytes) -> Int = "moonbitlang_x_fs_create_dir_ffi"

///|
fn create_dir_internal(path : String) -> Unit raise IOError {
  guard create_dir_ffi(@ffi.mbt_string_to_utf8_bytes(path, true)) == 0 else {
    raise IOError(get_error_message())
  }
}

///|
#borrow(path)
extern "C" fn is_dir_ffi(path : Bytes) -> Int = "moonbitlang_x_fs_is_dir_ffi"

///|
fn is_dir_internal(path : String) -> Bool raise IOError {
  let res = is_dir_ffi(@ffi.mbt_string_to_utf8_bytes(path, true))
  guard res != -1 else { raise IOError(get_error_message()) }
  res == 1
}

///|
#borrow(path)
extern "C" fn is_file_ffi(path : Bytes) -> Int = "moonbitlang_x_fs_is_file_ffi"

///|
fn is_file_internal(path : String) -> Bool raise IOError {
  let res = is_file_ffi(@ffi.mbt_string_to_utf8_bytes(path, true))
  guard res != -1 else { raise IOError(get_error_message()) }
  res == 1
}

///|
#borrow(path)
extern "C" fn remove_dir_ffi(path : Bytes) -> Int = "moonbitlang_x_fs_remove_dir_ffi"

///|
fn remove_dir_internal(path : String) -> Unit raise IOError {
  guard remove_dir_ffi(@ffi.mbt_string_to_utf8_bytes(path, true)) == 0 else {
    raise IOError(get_error_message())
  }
}

///|
#borrow(path)
extern "C" fn remove_file_ffi(path : Bytes) -> Int = "moonbitlang_x_fs_remove_file_ffi"

///|
fn remove_file_internal(path : String) -> Unit raise IOError {
  guard remove_file_ffi(@ffi.mbt_string_to_utf8_bytes(path, true)) == 0 else {
    raise IOError(get_error_message())
  }
}
