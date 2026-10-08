///|
pub(all) enum HttpFileMapping {
  Exact(Array[String], String)
  Subtree(Array[String], String)
} derive(Debug, Eq)

///|
fn http_fail(category : String) -> Unit raise AgentParseError {
  raise AgentParseError(category)
}

///|
fn valid_file_segment(segment : String) -> Bool {
  segment != "" &&
  segment != "." &&
  segment != ".." &&
  !segment
  .iter()
  .any(c => c.to_int() <= 31 || c.to_int() == 127 || c == '/' || c == '\\')
}

///|
fn http_hex(c : UInt16) -> Int raise AgentParseError {
  match c {
    '0'..='9' => c.to_int() - 48
    'a'..='f' => c.to_int() - 87
    'A'..='F' => c.to_int() - 55
    _ => {
      http_fail("source-path")
      0
    }
  }
}

///|
fn decode_http_segment(raw : String) -> String raise AgentParseError {
  let bytes : Array[Byte] = []
  let mut i = 0
  while i < raw.length() {
    let c = raw[i]
    if c == '%' {
      if i + 2 >= raw.length() {
        http_fail("source-path")
      }
      bytes.push((http_hex(raw[i + 1]) * 16 + http_hex(raw[i + 2])).to_byte())
      i += 3
    } else {
      if !(c is ('a'..='z' | 'A'..='Z' | '0'..='9')) &&
        !"-._~!$&'()*+,;=:@".iter().any(ch => ch.to_int() == c.to_int()) {
        http_fail("source-path")
      }
      bytes.push(c.to_byte())
      i += 1
    }
  }
  let decoded = @utf8.decode(Bytes::from_array(bytes)) catch {
    _ => {
      http_fail("source-path")
      ""
    }
  }
  if !valid_file_segment(decoded) {
    http_fail("source-path")
  }
  decoded
}

///|
/// Decodes literal declaration segments once, without collapsing separators.
fn literal_http_path(path : String) -> Array[String] raise AgentParseError {
  if path == "/" {
    return []
  }
  if !path.has_prefix("/") || path.has_suffix("/") {
    http_fail("source-path")
  }
  let result : Array[String] = []
  for part in path.split("/").drop(1) {
    result.push(decode_http_segment(part.to_owned()))
  }
  result
}

///|
pub fn compile_http_file_mapping(
  source : String,
  target : String,
) -> HttpFileMapping raise AgentParseError {
  let subtree = source.has_suffix("/*")
  let prefix = if subtree {
    let p = source.unsafe_substring(start=0, end=source.length() - 2)
    if p.has_suffix("/") {
      http_fail("source-path")
    }
    if p == "" {
      "/"
    } else {
      p
    }
  } else {
    source
  }
  if prefix.contains("*") {
    http_fail("source-wildcard")
  }
  if prefix
    .iter()
    .any(c => c == '$' || c == '?' || c == '#' || c == '{' || c == '}') {
    http_fail("source-path")
  }
  let segments = literal_http_path(prefix)
  let root = if subtree {
    if !target.has_suffix("/$1") {
      http_fail("target-placeholder")
    }
    let p = target.unsafe_substring(start=0, end=target.length() - 3)
    if p.has_suffix("/") {
      http_fail("target-path")
    }
    if p == "" {
      "/"
    } else {
      p
    }
  } else {
    target
  }
  if root.contains("$") {
    http_fail("target-placeholder")
  }
  if !root.has_prefix("/") ||
    (!subtree && root == "/") ||
    root
    .iter()
    .any(c => c.to_int() <= 31 || (c.to_int() >= 127 && c.to_int() <= 159)) {
    http_fail("target-path")
  }
  // Reject unpaired UTF-16 surrogates rather than silently encoding replacements.
  let encoded = @utf8.encode(root)
  let decoded = @utf8.decode(encoded) catch { _ => "" }
  if decoded != root {
    http_fail("target-path")
  }
  if root != "/" &&
    root.split("/").drop(1).any(s => !valid_file_segment(s.to_owned())) {
    http_fail("target-path")
  }
  if subtree {
    Subtree(segments, root)
  } else {
    Exact(segments, root)
  }
}

///|
pub fn compile_http_file_mappings(
  pairs : Array[(String, String)],
) -> Array[HttpFileMapping] raise AgentParseError {
  let result : Array[HttpFileMapping] = []
  for pair in pairs {
    let mapping = compile_http_file_mapping(pair.0, pair.1)
    if result.contains(mapping) {
      http_fail("duplicate-mapping")
    }
    result.push(mapping)
  }
  result
}
