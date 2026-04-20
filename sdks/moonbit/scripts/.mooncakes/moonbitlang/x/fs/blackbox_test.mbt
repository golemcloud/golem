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
test "write_and_read" {
  let dir = "tmp_fs_write_and_read"
  if @fs.path_exists(dir) {
    @fs.remove_file("\{dir}/test_1.txt") catch {
      _ => ()
    }
    @fs.remove_file("\{dir}/test_2.txt") catch {
      _ => ()
    }
    @fs.remove_dir(dir) catch {
      _ => ()
    }
  }
  @fs.create_dir(dir)
  let path_1 = "\{dir}/test_1.txt"
  let path_2 = "\{dir}/test_2.txt"
  let content_1 = "Hello, World"
  let content_2 = "Hello, MoonBit"
  @fs.write_bytes_to_file(path_1, @encoding.encode(UTF16, content_1))
  let res_1 = @fs.read_file_to_bytes(path_1)
  inspect(res_1.to_unchecked_string(), content=content_1)
  @fs.write_string_to_file(path_2, content_2)
  let res_2 = @fs.read_file_to_string(path_2)
  inspect(res_2, content=content_2)
  @fs.remove_file(path_1)
  @fs.remove_file(path_2)
  @fs.remove_dir(dir)
}

///|
test "read_dir" {
  let dir = "tmp_fs_read_dir"
  if @fs.path_exists(dir) {
    @fs.remove_file("\{dir}/file_a.txt") catch {
      _ => ()
    }
    @fs.remove_file("\{dir}/file_b.txt") catch {
      _ => ()
    }
    @fs.remove_dir("\{dir}/.hidden_dir") catch {
      _ => ()
    }
    @fs.remove_dir("\{dir}/example_dir") catch {
      _ => ()
    }
    @fs.remove_dir(dir) catch {
      _ => ()
    }
  }
  @fs.create_dir(dir)
  @fs.create_dir("\{dir}/.hidden_dir")
  @fs.create_dir("\{dir}/example_dir")
  let file_a = "\{dir}/file_a.txt"
  let file_b = "\{dir}/file_b.txt"
  @fs.write_string_to_file(file_a, "a")
  @fs.write_string_to_file(file_b, "b")
  let files = @fs.read_dir(dir)
  files.sort()
  inspect(
    files,
    content=(
      #|["file_a.txt", "file_b.txt", ".hidden_dir", "example_dir"]
    ),
  )
  @fs.remove_file(file_a)
  @fs.remove_file(file_b)
  @fs.remove_dir("\{dir}/.hidden_dir")
  @fs.remove_dir("\{dir}/example_dir")
  @fs.remove_dir(dir)
}

///|
test "create_dir_and_remove_dir" {
  let dir = "tmp_fs_create_dir"
  if @fs.path_exists(dir) {
    @fs.remove_dir(dir) catch {
      _ => ()
    }
  }
  assert_false(@fs.path_exists(dir))
  @fs.create_dir(dir)
  assert_true(@fs.path_exists(dir))
  assert_true(@fs.is_dir(dir))
  assert_false(@fs.is_file(dir))
  @fs.remove_dir(dir)
  assert_false(@fs.path_exists(dir))
}
