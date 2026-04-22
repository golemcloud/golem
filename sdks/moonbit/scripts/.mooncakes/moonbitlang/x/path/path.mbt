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
/// Terminology:
/// 
/// path component is original from [posix 4.13 pathname resolution](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap04.html#tag_04_13)

///|
let is_windows : Bool = @ffi.is_windows()

///|
/// A newtype wrapper provide path operation methods.
pub(all) struct Path(String) derive(Eq)

///|
pub impl Show for Path with output(path, logger) {
  logger.write_object(path.0)
}

///|
pub impl Show for Path with to_string(path) {
  path.0
}

///|
/// Returns the last path component of the given path.
///
/// property: `@path.join(@path.dirname(path), @path.basename(path)) == path`
/// 
/// Warning: basename function not respect this POSIX standard, but consistent with Python `os.path` module.
/// 
/// ```moonbit skip nocheck
/// let path : Path = "usr/local/bin"
/// inspect(path.basename(), content="bin")
/// ```
pub fn Path::basename(path : Path) -> StringView {
  if is_windows {
    @win32.Path::basename(path.0)
  } else {
    @posix.Path::basename(path.0)
  }
}

///|
/// Returns path without its last path component and trailing slashes.
/// 
/// property: `@path.join(@path.dirname(path), @path.basename(path)) == path`
/// 
/// Warning: basename function not respect this POSIX standard, but consistent with Python `os.path` module.
/// 
/// ```moonbit skip nocheck
/// let path : Path = "usr/local/bin"
/// inspect(path.dirname(), content="usr/local")
/// ```
pub fn Path::dirname(path : Path) -> Path {
  if is_windows {
    @win32.Path::dirname(path.0).0
  } else {
    @posix.Path::dirname(path.0).0
  }
}

///|
/// Returns the string of the basename after the last `.` delimiter.
/// 
/// ```moonbit skip nocheck
/// let path : Path = "archive.tar.gz"
/// inspect(path.extname(), content=".gz")
/// ```
pub fn Path::extname(path : Path) -> StringView {
  if is_windows {
    @win32.Path::extname(path.0)
  } else {
    @posix.Path::extname(path.0)
  }
}

///|
/// Returns whether the given path is absolute.
/// 
/// edge cases: when path is empty, return false
/// 
/// ```moonbit skip nocheck
/// let path1 : Path = "/usr/local/bin"
/// let path2 : Path = "usr/local/bin"
/// json_inspect(path1.is_absolute(), content=true)
/// json_inspect(path2.is_absolute(), content=false)
/// ```
pub fn Path::is_absolute(path : Path) -> Bool {
  if is_windows {
    @win32.Path::is_absolute(path.0)
  } else {
    @posix.Path::is_absolute(path.0)
  }
}

///|
/// 1. string concatenation with OS platform specific path separator
/// 2. if rhs is absolute or on a different drive in Windows, return rhs.
/// 
/// edge cases:
///  1. when lhs and rhs both are empty, return empty string
///  2. ignore empty string
/// 
/// ```moonbit skip nocheck
/// let path : Path = "/usr/local"
/// inspect(path.join("bin"), content="/usr/local/bin")
/// ```
pub fn Path::join(lhs : Path, rhs : Path) -> Path {
  if is_windows {
    @win32.Path::join(lhs.0, rhs.0).0
  } else {
    @posix.Path::join(lhs.0, rhs.0).to_string()
  }
}

///|
/// 1. resolve `.` by directly removing it
/// 2. resolve `..` by removing the preceding path component if exists, otherwise keep it.
/// 3. remove redundant slashes or backslashes
/// 4. preserve trailing slash or backslash
/// 
/// edge cases:
///   1. when path is empty, return empty string
///   2. when path is contains two leading forward slashes like `//`, return `//`. respect POSIX standard, this behavior is inconsistent with Node.js path module.
/// 
/// ```moonbit skip nocheck
/// let path : Path = "/usr//local/../bin/"
/// inspect(path.normalize(), content="/usr/bin")
/// ```
pub fn Path::normalize(path : Path) -> Path {
  if is_windows {
    @win32.Path::normalize(path.0).to_string()
  } else {
    @posix.Path::normalize(path.0).to_string()
  }
}

///|
/// Return the `to` relative path when `from` is the current working directory.
/// 
/// property: `@path.join(from,@path.relative(from,to)) == @path.normalize(to)`
/// 
/// edge cases: 
///  1. when `@path.normalize(from) == @path.normalize(to)`, return empty string
///  2. when `from` and `to` have different prefixes, return empty string
/// 
/// Warning: cwd is already resolve symbolic link and normalized, but path doesn't.
/// 
/// ```moonbit skip nocheck
/// let base : Path = "/usr/local/bin"
/// let path : Path = "/usr/local/share/doc"
/// inspect(path.relative(base~), content="../share/doc")
/// ```
pub fn Path::relative(path : Path, base~ : Path) -> Path {
  if is_windows {
    @win32.Path::relative(path.0, base=base.0).0
  } else {
    @posix.Path::relative(path.0, base=base.0).0
  }
}

///|
/// 1. if path is already absolute, return path normalized.
/// 2. if path is relative, join current working directory and path, and then normalize it.
///
/// Warning: cwd is already resolve symbolic link and normalized, but path doesn't.
/// 
/// ```moonbit skip skip nocheck
/// let path : Path = "project/src"
/// inspect(path.resolve(), content="/your/current_working_directory/project/src")
/// ```
pub fn Path::resolve(path : Path) -> Path {
  if is_windows {
    @win32.Path::resolve(path.0).to_string()
  } else {
    @posix.Path::resolve(path.0).to_string()
  }
}

///|
/// OS platform specific path delimiter for environment variables. e.g., PATH
pub let delimiter : Char = if is_windows { ';' } else { ':' }

///|
/// OS platform specific path component separator.
pub let sep : Char = if is_windows { '\\' } else { '/' }
