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
/// ```moonbit check
/// test {
///   let path : Path = "usr\\local\\bin"
///   inspect(path.basename(), content="bin")
/// }
/// ```
pub fn Path::basename(path : Path) -> StringView {
  let prefix = path.prefix()
  let rest = path.0.view(start_offset=prefix.length())
  match rest.rev_find("\\") {
    None => rest
    Some(idx) if idx >= rest.length() => ""
    Some(idx) => try! rest[idx + 1:]
  }
}

///|
/// Returns path without its last path component and trailing backslashes.
/// 
/// property: `@path.join(@path.dirname(path), @path.basename(path)) == path`
/// 
/// Warning: basename function not respect this POSIX standard, but consistent with Python `os.path` module.
/// 
/// ```moonbit check
/// test {
///   let path : Path = "usr\\local\\bin"
///   inspect(path.dirname(), content="usr\\local")
/// }
/// ```
pub fn Path::dirname(path : Path) -> Path {
  let prefix = path.prefix()
  let rest = path.0.view(start_offset=prefix.length())
  let last_slash_pos = rest.rev_find("\\")
  match last_slash_pos {
    None => prefix.to_string()
    Some(idx) => {
      let dirname_with_trailing_slashes = try! rest[0:idx]
      let dirname_trim_end = dirname_with_trailing_slashes.trim_end(
        char_set="\\",
      )
      if dirname_trim_end.is_empty() {
        path.0
        .view(
          start_offset=0,
          end_offset=prefix.length() + dirname_with_trailing_slashes.length(),
        )
        .to_string()
      } else {
        path.0
        .view(
          start_offset=0,
          end_offset=prefix.length() + dirname_trim_end.length(),
        )
        .to_string()
      }
    }
  }
}

///|
test {
  let path : Path = "\\\\\\"
  inspect(path.prefix(), content="\\\\\\")
  // inspect(path.0.view)
}

///|
/// Returns the string of the basename after the last `.` delimiter.
/// 
/// ```moonbit check
/// test {
///   let path : Path = "archive.tar.gz"
///   inspect(path.extname(), content=".gz")
/// }
/// ```
pub fn Path::extname(path : Path) -> StringView {
  let basename = Path::basename(path)
  match basename.rev_find(".") {
    None => ""
    Some(idx) => try! basename[idx:]
  }
}

///|
/// Returns whether the given path is absolute.
/// 
/// edge cases: when path is empty, return false
/// 
/// ```moonbit check
/// test {
///   let path1 : Path = "C:\\Program Files\\App"
///   let path2 : Path = "Program Files\\App"
///   json_inspect(path1.is_absolute(), content=true)
///   json_inspect(path2.is_absolute(), content=false)
/// }
/// ```
pub fn Path::is_absolute(path : Path) -> Bool {
  let path = path.0
  lexmatch path with longest {
    ("\\\\\?\\" ("[^\\]+" as _symlink) "(\\)+", _rest) => true
    (
      "\\\\\?UNC\\"
      ("[^\\]+" as _hostname)
      "\\"
      ("[^\\]+" as _shared_folder)
      "(\\)+",
      _rest
    ) => true
    (
      "\\\\"
      ("[^\\]+" as _hostname)
      "\\"
      ("[^\\]+" as _shared_folder)
      "(\\)+",
      _rest
    ) => true
    ("\\\\\?\\Volume[{]" ("[^}]+" as _guid) "[}]", _rest) => true
    ("\\\\\.\\" ("[^\\]+" as _device) "(\\)+", _rest) => true
    ("\\\\\?\\" ("[a-zA-Z]" as _letter) ":(\\)+", _rest) => true
    ("\\", _rest) => true
    (("[a-zA-Z]" as _letter) ":(\\)+", _rest) => true
    (("[a-zA-Z]" as _letter) ":", _rest) => false
    ("[^a-zA-Z\\][^:][^\\]+(\\)+", _rest) => true
    _ => false
  }
}

///|
fn Path::prefix(path : Path) -> StringView {
  let path = path.0
  lexmatch path with longest {
    (("\\\\\?\\" ("[^\\]+" as _symlink) "(\\)+") as prefix, _rest) => prefix
    (
      (
        "\\\\\?UNC\\"
        ("[^\\]+" as _hostname)
        "\\"
        ("[^\\]+" as _shared_folder)
        "(\\)+"
      ) as prefix,
      _rest
    ) => prefix
    (
      ("\\\\" ("[^\\]+" as _hostname) "\\" ("[^\\]+" as _shared_folder) "(\\)*") as prefix,
      _rest
    ) => prefix
    (("\\\\\?\\Volume[{]" ("[^}]+" as _guid) "[}]") as prefix, _rest) => prefix
    (("\\\\\.\\" ("[^\\]+" as _device) "(\\)+") as prefix, _rest) => prefix
    (("\\\\\?\\" ("[a-zA-Z]" as _letter) ":(\\)+") as prefix, _rest) => prefix
    ("\\+" as prefix, _rest) => prefix
    ((("[a-zA-Z]" as _letter) ":(\\)+") as prefix, _rest) => prefix
    ((("[a-zA-Z]" as _letter) ":") as prefix, _rest) => prefix
    ("[^a-zA-Z\\][^:][^\\]+(\\)+" as prefix, _rest) => prefix
    _ => ""
  }
}

///|
fn join_rest(lhs : StringView, rhs : StringView) -> StringView? {
  lexmatch rhs with longest {
    ("\\\\\?\\" ("[^\\]+" as _symlink) "(\\)+", _rest) => None
    (
      "\\\\\?UNC\\"
      ("[^\\]+" as _hostname)
      "\\"
      ("[^\\]+" as _shared_folder)
      "(\\)+",
      _rest
    ) => None
    (
      "\\\\"
      ("[^\\]+" as _hostname)
      "\\"
      ("[^\\]+" as _shared_folder)
      "(\\)+",
      _rest
    ) => None
    ("\\\\\?\\Volume[{]" ("[^}]+" as _guid) "[}]", _rest) => None
    ("\\\\\.\\" ("[^\\]+" as _device) "(\\)+", _rest) => None
    ("\\\\\?\\" ("[a-zA-Z]" as _letter) ":(\\)+", _rest) => None
    ("\\", _rest) => None
    (("[a-zA-Z]" as _letter) ":(\\)+", _rest) => None
    (("[a-zA-Z]" as rhs_letter) ":", rhs_rest) =>
      lexmatch lhs with longest {
        (("[a-zA-Z]" as lhs_letter) ":", _rest) =>
          if lhs_letter == rhs_letter {
            Some(rhs_rest)
          } else {
            None
          }
        _ => None
      }
    ("[^a-zA-Z\\][^:][^\\]+(\\)+", _rest) => None
    rhs_rest => Some(rhs_rest)
  }
}

///|
/// 1. string concatenation with OS platform specific path separator
/// 2. if rhs is absolute or on a different drive, return rhs
/// 
/// edge cases:
///  1. when lhs and rhs both are empty, return empty string
///  2. ignore empty string
/// 
/// ```moonbit check
/// test {
///   let path1 : Path = "C::\\Program Files"
///   let path2 : Path = "App"
///   inspect(path1.join(path2), content="C::\\Program Files\\App")
/// }
/// ```
pub fn Path::join(lhs : Path, rhs : Path) -> Path {
  let lhs = lhs.0
  let rhs = rhs.0
  match join_rest(lhs, rhs) {
    None => rhs
    Some(rhs) =>
      match lhs {
        [] => rhs.to_string()
        [.., '\\'] as all => {
          let builder = StringBuilder::new()
          builder.write_stringview(all)
          builder.write_stringview(rhs)
          builder.to_string()
        }
        without_tailing_slashes => {
          let builder = StringBuilder::new()
          builder.write_stringview(without_tailing_slashes)
          builder.write_char('\\')
          builder.write_stringview(rhs)
          builder.to_string()
        }
      }
  }
}

///|
/// 1. resolve `.` by directly removing it
/// 2. resolve `..` by removing the preceding path component if exists, otherwise keep it.
/// 3. remove redundant backslashes 
/// 4. preserve trailing slash or backslash
/// 
/// edge cases:
///   1. when path is empty, return empty string
/// 
/// ```moonbit check
/// test {
///   let path : Path = "C:\\usr\\\\local\\..\\bin\\"
///   inspect(path.normalize(), content="C:\\usr\\bin")
/// }
/// ```
pub fn Path::normalize(path : Path) -> Path {
  let path = path.0
  // if failed to normalize, return the original path
  WinPath::parse(path).to_string() catch {
    _ => path
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
/// ```moonbit check
/// test {
///   let path : Path = "C:\\Users\\Alice\\Documents\\Project"
///   let base : Path = "C:\\Users\\Alice"
///   inspect(path.relative(base~), content="Documents\\Project")
/// }
/// ```
pub fn Path::relative(path : Path, base~ : Path) -> Path {
  let cwd = @env.current_dir().unwrap()
  let from = try! WinPath::parse(Path::join(cwd, base).0)
  let to = try! WinPath::parse(Path::join(cwd, path).0)
  if from.prefix == to.prefix {
    let mut cnt = 0
    // remove the common path component prefix
    loop (from.components[:], to.components[:]) {
      ([x, .. xs], [y, .. ys]) if x == y => {
        cnt += 1
        continue (xs, ys)
      }
      _ => ()
    }
    let components : Array[StringView] = []
    let goto_parent_cnt = from.components.length() - cnt
    for _ in 0..<goto_parent_cnt {
      components.push("..")
    }
    let rest_path = to.components[cnt:]
    for comp in rest_path {
      components.push(comp)
    }
    let path = WinPath::{ prefix: None, components }
    path.to_string()
  } else {
    ""
  }
}

///|
/// 1. if path is absolute, return path normalized.
/// 2. if path is relative, join current working directory and path, and then normalize it.
///
/// Warning: cwd is already resolve symbolic link and normalized, but path doesn't.
/// 
/// ```moonbit skip nocheck
/// let path : Path = "Documents\\Project"
/// inspect(path.resolve(), content="C:\\your\\current_working_directory\\Documents\\Project")
/// ```
pub fn Path::resolve(path : Path) -> Path {
  if Path::is_absolute(path) {
    Path::normalize(path)
  } else {
    Path::join(@env.current_dir().unwrap(), path)
  }
}

///|
/// OS platform specific path delimiter for environment variables. e.g., PATH
pub let delimiter : Char = if true { ';' } else { ':' }

///|
/// OS platform specific path component separator.
pub let sep : Char = if true { '\\' } else { '/' }
