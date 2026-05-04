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
test "Unix absolute path" {
  let path = UnixPath::parse("/../folder/file")
  inspect(path, content="/folder/file")
  let path = UnixPath::parse("/folder/file")
  inspect(path, content="/folder/file")
  let path = UnixPath::parse("/")
  inspect(path, content="/")
}

///|
test "Unix path normalization" {
  let path = UnixPath::parse("./a/b/./c/../d/../..")
  inspect(path, content="a")
  let path = UnixPath::parse("/a/b/..")
  inspect(path, content="/a")
  let path = UnixPath::parse("/../..")
  inspect(path, content="/")
  let path = UnixPath::parse("/../a/b/../c/../../..")
  inspect(path, content="/")
  let path = UnixPath::parse("/../a/b/../c/../..")
  inspect(path, content="/")
  let path = UnixPath::parse("/../a/b/../c")
  inspect(path, content="/a/c")
}

///|
test "Unix relative path" {
  let path = UnixPath::parse("./folder/file")
  inspect(path, content="folder/file")
  let path = UnixPath::parse("containers/finger_tree")
  inspect(path, content="containers/finger_tree")
  let path = UnixPath::parse("containers/finger_tree/finger_tree.mbt")
  inspect(path, content="containers/finger_tree/finger_tree.mbt")
}

///|
test "Unix inner path normalization for slashes" {
  let path = UnixPath::parse("./a//b////c")
  inspect(path, content="a/b/c")
}

///|
test "Unix slashes normalization" {
  let path = UnixPath::parse("///home/username/project")
  inspect(path, content="/home/username/project")
  let path = UnixPath::parse(".//a/b//c/e////d")
  inspect(path, content="a/b/c/e/d")
}

///|
test "Unix multiple tailing slashes normalization" {
  let path = UnixPath::parse("/folder//")
  inspect(path, content="/folder")
  let path = UnixPath::parse("./a/b/c///")
  inspect(path, content="a/b/c")
  let path = UnixPath::parse("./a/b/c////")
  inspect(path, content="a/b/c")
}

///|
test "Unix edge cases" {
  let path = UnixPath::parse("/")
  inspect(path, content="/")
  let path = UnixPath::parse("./")
  inspect(path)
  let path = UnixPath::parse("//")
  inspect(path, content="//")
  let path = UnixPath::parse("///")
  inspect(path, content="/")
}
