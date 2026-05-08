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
priv enum UnixPrefix {
  /// Syntax: `/` or `/{3,}`
  ///
  /// A pathname consisting of a single <slash> shall resolve to the root directory of the process.
  Root
  /// Syntax: `//`
  ///
  /// [Posix 4.13 Pathname Resolution](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap04.html#tag_04_13)
  ///
  /// > If a pathname begins with two successive <slash> characters, 
  /// > the first component following the leading <slash> characters may be interpreted in an implementation-defined manner,
  /// > although more than two leading <slash> characters shall be treated as a single <slash> character.
  SlashSlash
  None
} derive(ToJson, Hash, Eq)

///|
/// [Posix 4.13 Pathname Resolution](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap04.html#tag_04_13)
/// 
/// 
priv struct UnixPath {
  prefix : UnixPrefix
  components : Array[StringView]
} derive(ToJson, Hash, Eq)

///|
fn unix_path_component_from(s : StringView) -> StringView raise PathError {
  guard !s.is_empty() else {
    raise PathError::PathError(
      start=0,
      message="Unix path: empty path component not allowed",
    )
  }
  guard s.get_char(0) != Some('-') else {
    raise PathError::PathError(
      start=0,
      message="Unix path: component '\{s}' cannot start with '-' (not portable according to POSIX standard)",
    )
  }
  for i, ch in s {
    guard ch != '/' else {
      raise PathError::PathError(
        start=i,
        message="Unix path: forward slash '/' at position \{i} not allowed in path component",
      )
    }
    guard ch != '\u0000' else {
      raise PathError::PathError(
        start=i,
        message="Unix path: null character (\\u0000) at position \{i} not allowed in path component",
      )
    }
  }
  s
}

///|
priv suberror PathError {
  PathError(start~ : Int, message~ : String)
} derive(Show)

///|
impl Show for UnixPath with output(self : UnixPath, logger : &Logger) -> Unit {
  let { prefix, components } = self
  match prefix {
    Root => logger.write_char('/')
    SlashSlash => logger.write_string("//")
    None => ()
  }
  match components {
    [] => ()
    [x, .. xs] => {
      logger.write_string(x.to_string())
      for x in xs {
        logger.write_char('/')
        logger.write_string(x.to_string())
      }
    }
  }
}

///|
fn parse_unix_relative_path(
  s : StringView,
  start~ : Int,
  root_eliminate~ : Bool,
) -> Array[StringView] raise PathError {
  let mut rest = s
  let components : Array[StringView] = []
  let mut start = start
  for {
    lexmatch rest with longest {
      // pattern: <path_component><slashes>
      // slashes elimination
      ((("[^/]+" as s) "(/)+") as all, r) => {
        match s {
          "." => ()
          ".." =>
            match (components.pop(), root_eliminate) {
              (Some(".."), _) => {
                components.push("..")
                components.push("..")
              }
              (None, true) => ()
              (None, false) => components.push("..")
              (Some(_), _) => ()
            }
          _ => components.push(s)
        }
        start += all.char_length()
        rest = r
      }
      // pattern: <dot-dot>
      "\.\." => {
        match (components.pop(), root_eliminate) {
          (Some(".."), _) => {
            components.push("..")
            components.push("..")
          }
          (None, true) => ()
          (None, false) => components.push("..")
          (Some(_), _) => ()
        }
        break
      }
      // pattern: <dot>
      "\." => break
      ("[^/]+" as filename) => {
        components.push(filename)
        break
      }
      "$" => break
      _ => raise PathError(start~, message="Invalid Unix path")
    }
  }
  components
}

///|
fn UnixPath::parse(path : StringView) -> UnixPath raise PathError {
  lexmatch path with longest {
    // current path
    // syntax: `./<rest_of_path>`
    // slashes elimination 
    ("\./+" as prefix, rest) => {
      let directory = parse_unix_relative_path(
        rest,
        start=prefix.char_length(),
        root_eliminate=false,
      )
      UnixPath::{ prefix: None, components: directory }
    }
    // syntax: `<path_component>/<rest_of_path>`
    // slashes elimination
    ("[^/]+/+", _) => {
      let directory = parse_unix_relative_path(
        path,
        start=0,
        root_eliminate=false,
      )
      UnixPath::{ prefix: None, components: directory }
    }
    // root path 
    // syntax: `/<rest_of_path>` or `/{3,}<rest_of_path>`
    //
    // slashes elimination
    ("/|/{3,}", rest) => {
      let directory = parse_unix_relative_path(
        rest,
        start=1,
        root_eliminate=true,
      )
      UnixPath::{ prefix: Root, components: directory }
    }
    // syntax: `//<rest_of_path>
    // 
    ("//", rest) => {
      let directory = parse_unix_relative_path(
        rest,
        start=1,
        root_eliminate=true,
      )
      UnixPath::{ prefix: SlashSlash, components: directory }
    }
    non_prefix_hint_path => {
      let directory = parse_unix_relative_path(
        non_prefix_hint_path,
        start=0,
        root_eliminate=false,
      )
      UnixPath::{ prefix: None, components: directory }
    }
  }
}
