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
test "basename/dirname" {
  inspect(Path::basename("a/b"), content="b")
  inspect(Path::dirname("a/b"), content="a")
  inspect(Path::basename("a/b/"))
  inspect(Path::dirname("a/b/"), content="a/b")
}

///|
test "dirname/edge cases" {
  // Root paths
  inspect(Path::dirname("/"), content="/")
  inspect(Path::dirname("///"), content="///")
  inspect(Path::dirname("////"), content="////")
  // POSIX platform specific cases
  inspect(Path::dirname("//"), content="//")
  // relative paths
  inspect(Path::dirname("a"), content="")
  // relative paths with trailing slashes
  inspect(Path::dirname("a/"), content="a")
  inspect(Path::dirname("a//"), content="a")
  inspect(Path::dirname("a///"), content="a")
  inspect(Path::dirname("a////"), content="a")

  // Root path with single directory
  inspect(Path::dirname("/a"), content="/")
  inspect(Path::dirname("///a"), content="///")
  // POSIX platform specific cases with single directory
  inspect(Path::dirname("//a"), content="//")
  // Empty path 
  inspect(Path::dirname(""), content="")
  // path with `.` and `..`, Path::dirname doesn't resolve them.
  inspect(Path::dirname("./a"), content=".")
  inspect(Path::dirname("../a"), content="..")
  inspect(Path::dirname("a/."), content="a")
  inspect(Path::dirname("a/.."), content="a")

  // Paths with multiple consecutive slashes
  inspect(Path::dirname("a//b"), content="a")
  inspect(Path::dirname("a///b"), content="a")
  inspect(Path::dirname("/a//b//c"), content="/a//b")
}

///|
test "basename/edge cases" {
  // Root paths
  inspect(Path::basename("/"), content="")
  inspect(Path::basename("///"), content="")
  inspect(Path::basename("////"), content="")
  // POSIX platform specific cases
  inspect(Path::basename("//"), content="")
  // relative paths
  inspect(Path::basename("a"), content="a")
  // relative paths with trailing slashes
  inspect(Path::basename("a/"), content="")
  inspect(Path::basename("a//"), content="")
  inspect(Path::basename("a///"), content="")
  inspect(Path::basename("a////"), content="")

  // Root path with single directory
  inspect(Path::basename("/a"), content="a")
  inspect(Path::basename("///a"), content="a")
  // POSIX platform specific cases with single directory
  inspect(Path::basename("//a"), content="a")
  // Empty path 
  inspect(Path::basename(""), content="")
  // path with `.` and `..`, Path::basename doesn't resolve them.
  inspect(Path::basename("./a"), content="a")
  inspect(Path::basename("../a"), content="a")
  inspect(Path::basename("a/."), content=".")
  inspect(Path::basename("a/.."), content="..")

  // Paths with multiple consecutive slashes
  inspect(Path::basename("a//b"), content="b")
  inspect(Path::basename("a///b"), content="b")
  inspect(Path::basename("/a//b//c"), content="c")
}

///|
test "extname" {
  inspect(Path::extname("project/main.mbt.md"), content=".md")
  inspect(Path::extname("project/"), content="")
}

///|
test "is_absolute" {
  json_inspect(Path::is_absolute("/home/user"), content=true)
  json_inspect(Path::is_absolute("home/user"), content=false)
  json_inspect(Path::is_absolute(""), content=false)
}

///|
test "join" {
  inspect(Path::join("a", "/b"), content="/b")
  inspect(Path::join("a/", "b"), content="a/b")
  inspect(Path::join("a", "b"), content="a/b")
  inspect(Path::join("", ""))
}

///|
test "relative" {
  let from = "/home/user_name"
  let to = "/home/user_name/proj_a"
  inspect(Path::relative(base=from, to), content="proj_a")
  let from = "/home/user_name/proj_a"
  let to = "/home/user_name"
  inspect(Path::relative(base=from, to), content="..")
  let from = "/home/user_name"
  let to = "/home/user_name"
  inspect(Path::relative(base=from, to), content="")
  let from = "/home/user_name/proj_a"
  let to = "/home/user_name/proj_b"
  inspect(Path::relative(base=from, to), content="../proj_b")
}

///|
test "normalize" {
  inspect(Path::normalize("a/./b/../c/"), content="a/c")
  inspect(Path::normalize("/a/b/../../c/."), content="/c")
  inspect(Path::normalize("a/b/c/.."), content="a/b")
  inspect(Path::normalize(""))
}

///|
test "resolve" {
  inspect(Path::resolve("/a/../b/c"), content="/b/c")
  inspect(Path::resolve("/a/b/c/../../.."), content="/")
}

///|
test "extname edge cases" {
  // Empty path and no extension
  inspect(Path::extname(""), content="")
  inspect(Path::extname("file"), content="")
  inspect(Path::extname("/path/file"), content="")

  // Hidden files
  inspect(Path::extname(".hidden"), content=".hidden")
  inspect(Path::extname(".hidden.txt"), content=".txt")
  inspect(Path::extname("/path/.hidden"), content=".hidden")
  inspect(Path::extname("/path/.hidden.txt"), content=".txt")

  // Dot files and directories
  inspect(Path::extname("."), content=".")
  inspect(Path::extname(".."), content=".")
  inspect(Path::extname("./file"), content="")
  inspect(Path::extname("../file"), content="")
  inspect(Path::extname("/a/."), content=".")
  inspect(Path::extname("/a/.."), content=".")

  // Multiple extensions
  inspect(Path::extname("file.tar.gz"), content=".gz")
  inspect(Path::extname("archive.tar.bz2"), content=".bz2")
  inspect(Path::extname("config.json.bak"), content=".bak")

  // Extensions with special characters
  inspect(Path::extname("file.c++"), content=".c++")
  inspect(Path::extname("file.123"), content=".123")
  inspect(Path::extname("file.-"), content=".-")

  // Paths with trailing slashes
  inspect(Path::extname("file.txt/"), content="")
  inspect(Path::extname("/path/file.txt/"), content="")

  // Extensions at different positions
  inspect(Path::extname(".txt"), content=".txt")
  inspect(Path::extname("a.b.c"), content=".c")
  inspect(Path::extname("a."), content=".")
  inspect(Path::extname("/path/a."), content=".")

  // Very long extensions
  inspect(Path::extname("file.verylongextension"), content=".verylongextension")
}

///|
test "is_absolute edge cases" {
  // Empty and whitespace
  json_inspect(Path::is_absolute(""), content=false)
  json_inspect(Path::is_absolute(" "), content=false)
  json_inspect(Path::is_absolute("  /"), content=false)

  // Various absolute path formats
  json_inspect(Path::is_absolute("/"), content=true)
  json_inspect(Path::is_absolute("//"), content=true)
  json_inspect(Path::is_absolute("///"), content=true)
  json_inspect(Path::is_absolute("/a"), content=true)
  json_inspect(Path::is_absolute("/a/b"), content=true)

  // Relative paths
  json_inspect(Path::is_absolute("a"), content=false)
  json_inspect(Path::is_absolute("./a"), content=false)
  json_inspect(Path::is_absolute("../a"), content=false)
  json_inspect(Path::is_absolute("a/b"), content=false)
  json_inspect(Path::is_absolute("a/../b"), content=false)

  // Edge cases with dots
  json_inspect(Path::is_absolute("."), content=false)
  json_inspect(Path::is_absolute(".."), content=false)
  json_inspect(Path::is_absolute("./"), content=false)
  json_inspect(Path::is_absolute("../"), content=false)
  json_inspect(Path::is_absolute("/."), content=true)
  json_inspect(Path::is_absolute("/.."), content=true)
}

///|
test "join edge cases" {
  // Empty strings
  inspect(Path::join("", ""), content="")
  inspect(Path::join("a", ""), content="a/")
  inspect(Path::join("", "b"), content="b")

  // Both paths are absolute
  inspect(Path::join("/a", "/b"), content="/b")
  inspect(Path::join("/a/c", "/b"), content="/b")

  // Various separator combinations
  inspect(Path::join("a", "b/c"), content="a/b/c")
  inspect(Path::join("a/", "b/c"), content="a/b/c")
  inspect(Path::join("a//", "b"), content="a//b")
  inspect(Path::join("a///", "b"), content="a///b")

  // Relative path components
  inspect(Path::join("a", "./b"), content="a/./b")
  inspect(Path::join("a", "../b"), content="a/../b")
  inspect(Path::join("./a", "b"), content="./a/b")
  inspect(Path::join("../a", "b"), content="../a/b")

  // Root paths
  inspect(Path::join("/", "a"), content="/a")
  inspect(Path::join("/", "/a"), content="/a")
  inspect(Path::join("//", "a"), content="//a")

  // Hidden files
  inspect(Path::join("a", ".hidden"), content="a/.hidden")
  inspect(Path::join("/a", ".hidden"), content="/a/.hidden")

  // Complex combinations
  inspect(Path::join("a/b", "../c"), content="a/b/../c")
  inspect(Path::join("/a/b", "../c"), content="/a/b/../c")
  inspect(Path::join("", "/abs"), content="/abs")

  // Multiple consecutive separators
  inspect(Path::join("a//b", "c"), content="a//b/c")
  inspect(Path::join("a", "b//c"), content="a/b//c")
}

///|
test "normalize edge cases" {
  // Empty path
  inspect(Path::normalize(""), content="")

  // Simple dot resolution
  inspect(Path::normalize("."), content="")
  inspect(Path::normalize("./"), content="")
  inspect(Path::normalize("a/."), content="a")
  inspect(Path::normalize("a/./"), content="a")
  inspect(Path::normalize("./a"), content="a")
  inspect(Path::normalize("./a/"), content="a")
  inspect(Path::normalize("a/./b"), content="a/b")

  // Double dot resolution
  inspect(Path::normalize(".."), content="..")
  inspect(Path::normalize("../"), content="..")
  inspect(Path::normalize("a/.."), content="")
  inspect(Path::normalize("a/../"), content="")
  inspect(Path::normalize("../a"), content="../a")
  inspect(Path::normalize("a/../b"), content="b")
  inspect(Path::normalize("a/b/.."), content="a")
  inspect(Path::normalize("a/b/../"), content="a")
  inspect(Path::normalize("a/b/../c"), content="a/c")

  // Multiple consecutive separators
  inspect(Path::normalize("a//b"), content="a/b")
  inspect(Path::normalize("a///b"), content="a/b")
  inspect(Path::normalize("//a//b"), content="//a/b")
  inspect(Path::normalize("///a///b"), content="/a/b")

  // Complex combinations
  inspect(Path::normalize("a/./b/../c"), content="a/c")
  inspect(Path::normalize("./a/../b/./c"), content="b/c")
  inspect(Path::normalize("a/b/../../c"), content="c")
  inspect(Path::normalize("a/b/../../../c"), content="../c")

  // Root paths with dots
  inspect(Path::normalize("/."), content="/")
  inspect(Path::normalize("/./"), content="/")
  inspect(Path::normalize("/.."), content="/")
  inspect(Path::normalize("/../"), content="/")
  inspect(Path::normalize("/a/.."), content="/")
  inspect(Path::normalize("/a/../"), content="/")
  inspect(Path::normalize("/a/../b"), content="/b")

  // Preserve trailing slashes where appropriate
  inspect(Path::normalize("a/b/c/"), content="a/b/c")
  inspect(Path::normalize("a/./b/../c/"), content="a/c")

  // Hidden files
  inspect(Path::normalize("./.hidden"), content=".hidden")
  inspect(Path::normalize("a/./.hidden"), content="a/.hidden")
  inspect(Path::normalize("a/../.hidden"), content=".hidden")
}

///|
test "relative edge cases" {
  // Same paths
  let same1 = "/home/user"
  let same2 = "/home/user"
  inspect(Path::relative(base=same1, same2), content="")

  // Same paths with different normalization
  inspect(Path::relative(base="/a/b", "/a/./b"), content="")
  inspect(Path::relative(base="/a/b", "/a/c/../b"), content="")

  // Root to various paths
  inspect(Path::relative(base="/", "/home"), content="home")
  inspect(Path::relative(base="/home", "/"), content="..")

  // Deep nesting
  inspect(Path::relative(base="/a/b/c/d", "/a/b/e/f"), content="../../e/f")
  inspect(Path::relative(base="/a/b/c/d/e", "/a/f"), content="../../../../f")

  // Hidden files and directories
  inspect(
    Path::relative(base="/home/.config", "/home/.local"),
    content="../.local",
  )
  inspect(Path::relative(base="/home", "/home/.hidden"), content=".hidden")

  // Paths with dots
  inspect(Path::relative(base="/home/user", "/home/user/."), content="")
  inspect(Path::relative(base="/home/user/.", "/home/user"), content="")
  inspect(Path::relative(base="/home/user", "/home/user/.."), content="..")

  // Complex relative navigation
  inspect(Path::relative(base="/a/b/c", "/a/b/c/../d"), content="../d")
  inspect(Path::relative(base="/a/b/c/../d", "/a/b/c"), content="../c")
}

///|
test "resolve edge cases" {
  // Already absolute paths should be normalized
  inspect(Path::resolve("/a/./b"), content="/a/b")
  inspect(Path::resolve("/a/../b"), content="/b")
  inspect(Path::resolve("/a/b/../../c"), content="/c")
}

///|
test "constants and separators" {
  // Test delimiter constant
  inspect(delimiter, content=":")

  // Test separator constant
  inspect(sep, content="/")
}
