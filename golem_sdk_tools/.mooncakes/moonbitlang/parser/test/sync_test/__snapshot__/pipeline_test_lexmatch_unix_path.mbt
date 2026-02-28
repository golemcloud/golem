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
}

///|
/// [Posix 4.13 Pathname Resolution](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap04.html#tag_04_13)
/// 
/// 
priv struct UnixPath {
  prefix : UnixPrefix
  components : Array[StringView]
}

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
priv suberror PathError {
  PathError(start~ : Int, message~ : String)
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

///|
fn main {
  let path = try! UnixPath::parse("/../folder/file")
  println(path)
  let path = try! UnixPath::parse("/folder/file")
  println(path)
  let path = try! UnixPath::parse("/")
  println(path)
}
