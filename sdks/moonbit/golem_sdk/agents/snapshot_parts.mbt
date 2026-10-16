///|
pub(all) struct SnapshotPart {
  bytes : Bytes
  content_type : String
} derive(Debug, Eq)

///|
pub(all) struct MultipartSnapshot {
  state : Json
  parts : Map[String, SnapshotPart]
} derive(Debug, Eq)

///|
pub fn[S : ToJson] MultipartSnapshot::from_state(
  state : S,
  parts : Map[String, SnapshotPart],
) -> MultipartSnapshot {
  { state: state.to_json(), parts, }
}

///|
pub fn[S : FromJson] MultipartSnapshot::decode_state(
  self : MultipartSnapshot,
) -> Result[S, String] {
  Ok(@json.from_json(self.state)) catch {
    error => Err(error.to_string())
  }
}

///|
pub fn MultipartSnapshot::require_part(
  self : MultipartSnapshot,
  name : String,
  expected_content_type : String,
) -> Result[Bytes, String] {
  try {
    let expected = snapshot_content_type(expected_content_type)
    guard self.parts.get(name) is Some(part) else {
      return Err("Missing snapshot part '\{name}'")
    }
    let actual = snapshot_content_type(part.content_type)
    if actual != expected {
      Err(
        "Snapshot part '\{name}' has content type '\{actual}', expected '\{expected}'",
      )
    } else {
      Ok(part.bytes)
    }
  } catch {
    SnapshotCodecError(message) => Err(message)
  }
}

///|
priv suberror SnapshotCodecError {
  SnapshotCodecError(String)
} derive(Debug)

///|
fn snapshot_check(
  condition : Bool,
  message : String,
) -> Unit raise SnapshotCodecError {
  if !condition {
    raise SnapshotCodecError(message)
  }
}

///|
fn snapshot_name(name : String) -> Bool {
  if name.is_empty() {
    return false
  }
  for i in 0..<name.length() {
    let c = name[i]
    if !(c is ('A'..='Z' | 'a'..='z' | '0'..='9' | '_')) &&
      !(i > 0 && c is ('.' | '-')) {
      return false
    }
  }
  true
}

///|
fn snapshot_content_type(value : String) -> String raise SnapshotCodecError {
  let pieces = value.split("/").map(v => v.to_owned()).to_array()
  snapshot_check(pieces.length() == 2, "Invalid snapshot MIME")
  for piece in pieces {
    snapshot_check(!piece.is_empty(), "Invalid snapshot MIME")
    for c in piece {
      snapshot_check(
        c
        is ('A'..='Z'
        | 'a'..='z'
        | '0'..='9'
        | '!'
        | '#'
        | '$'
        | '%'
        | '&'
        | '\''
        | '*'
        | '+'
        | '.'
        | '^'
        | '_'
        | '`'
        | '|'
        | '~'
        | '-'),
        "Invalid snapshot MIME",
      )
    }
  }
  value.to_lower()
}

///|
fn snapshot_at(data : Bytes, marker : Bytes, pos : Int) -> Bool {
  if pos < 0 || pos + marker.length() > data.length() {
    return false
  }
  for i in 0..<marker.length() {
    if data[pos + i] != marker[i] {
      return false
    }
  }
  true
}

///|
fn snapshot_delimiter(
  data : Bytes,
  marker : Bytes,
  newline : Bytes,
  pos : Int,
) -> (Int, Bool)? {
  if !snapshot_at(data, marker, pos) {
    return None
  }
  let suffix = pos + marker.length()
  let closing = snapshot_at(data, b"--", suffix)
  let end = suffix + (if closing { 2 } else { 0 })
  if snapshot_at(data, newline, end) {
    Some((end + newline.length(), closing))
  } else if closing && end == data.length() {
    Some((end, closing))
  } else {
    None
  }
}

///|
fn snapshot_boundary(mime : String) -> String raise SnapshotCodecError {
  let fields = mime.split(";").map(v => v.trim().to_owned()).to_array()
  snapshot_check(
    fields.length() == 2 && fields[0].to_lower() == "multipart/mixed",
    "Invalid multipart MIME",
  )
  guard fields[1].split_once("=") is Some((key, raw)) else {
    raise SnapshotCodecError("Missing boundary")
  }
  snapshot_check(
    key.trim().to_owned().to_lower() == "boundary",
    "Missing boundary",
  )
  let raw = raw.trim().to_owned()
  let value = if raw.has_prefix("\"") {
    snapshot_check(
      raw.length() >= 2 && raw.has_suffix("\""),
      "Invalid boundary quoting",
    )
    raw.unsafe_substring(start=1, end=raw.length() - 1)
  } else {
    for c in raw {
      snapshot_check(
        c is ('A'..='Z' | 'a'..='z' | '0'..='9' | '\'' | '+' | '_' | '.' | '-'),
        "Invalid unquoted boundary",
      )
    }
    raw
  }
  snapshot_check(
    !value.is_empty() && value.length() <= 70,
    "Invalid boundary length",
  )
  for c in value {
    snapshot_check(
      c
      is ('A'..='Z'
      | 'a'..='z'
      | '0'..='9'
      | '\''
      | '('
      | ')'
      | '+'
      | '_'
      | ','
      | '.'
      | '/'
      | ':'
      | '='
      | '?'
      | '-'),
      "Invalid boundary",
    )
  }
  value
}

///|
fn snapshot_parse_parts(
  data : Bytes,
  mime : String,
) -> Array[(String, SnapshotPart)] raise SnapshotCodecError {
  let marker = @utf8.encode("--" + snapshot_boundary(mime))
  let start = if snapshot_at(data, b"\r\n", 0) {
    2
  } else if snapshot_at(data, b"\n", 0) {
    1
  } else {
    0
  }
  let suffix = start + marker.length()
  let suffix = suffix + (if snapshot_at(data, b"--", suffix) { 2 } else { 0 })
  let newline = if snapshot_at(data, b"\r\n", suffix) ||
    (suffix == data.length() && start == 2) {
    b"\r\n"
  } else {
    b"\n"
  }
  snapshot_check(
    start == 0 || start == newline.length(),
    "Invalid leading newline",
  )
  guard snapshot_delimiter(data, marker, newline, start)
    is Some((initial_pos, initial_close)) else {
    raise SnapshotCodecError("Invalid first delimiter")
  }
  let mut pos = initial_pos
  let mut closing = initial_close
  let names : Map[String, Bool] = Map([])
  let result : Array[(String, SnapshotPart)] = []
  while !closing {
    let headers : Map[String, String] = Map([])
    while true {
      let mut end = pos
      while end < data.length() && data[end] != b'\n' {
        end += 1
      }
      snapshot_check(end < data.length(), "Truncated headers")
      let line_end = if end > pos && data[end - 1] == b'\r' {
        end - 1
      } else {
        end
      }
      let bytes = data[pos:line_end].to_owned()
      pos = end + 1
      if bytes.is_empty() {
        break
      }
      snapshot_check(bytes[0] != b' ', "Folded header")
      for b in bytes {
        snapshot_check(b >= 32 && b <= 126, "Invalid header")
      }
      let line = @utf8.decode(bytes) catch {
        _ => raise SnapshotCodecError("Invalid header UTF-8")
      }
      guard line.split_once(":") is Some((key, value)) else {
        raise SnapshotCodecError("Invalid header")
      }
      let key = key.to_owned().to_lower()
      snapshot_check(
        !headers.contains(key) &&
        (key == "content-type" || key == "content-disposition"),
        "Duplicate or unknown header",
      )
      headers[key] = value.trim().to_owned()
    }
    snapshot_check(headers.length() == 2, "Missing structural header")
    guard headers["content-disposition"].split_once(";") is Some((kind, param)) else {
      raise SnapshotCodecError("Invalid disposition")
    }
    snapshot_check(
      kind.to_owned().to_lower() == "attachment",
      "Invalid disposition",
    )
    guard param.trim().to_owned().split_once("=") is Some((key, raw)) else {
      raise SnapshotCodecError("Missing part name")
    }
    snapshot_check(key.to_owned().to_lower() == "name", "Invalid disposition")
    let raw = raw.to_owned()
    snapshot_check(
      raw.length() > 2 && raw.has_prefix("\"") && raw.has_suffix("\""),
      "Invalid part name quoting",
    )
    let name = raw.unsafe_substring(start=1, end=raw.length() - 1)
    snapshot_check(
      !name.contains("\"") && !name.contains("\\") && !names.contains(name),
      "Invalid or duplicate part name",
    )
    names[name] = true
    let mut next : (Int, Int, Bool)? = None
    for i in pos..<data.length() {
      if snapshot_at(data, newline, i) &&
        snapshot_delimiter(data, marker, newline, i + newline.length())
        is Some((end, close)) {
        next = Some((i, end, close))
        break
      }
    }
    guard next is Some((body_end, end, close)) else {
      raise SnapshotCodecError("Missing closing delimiter")
    }
    result.push(
      (
        name,
        {
          bytes: data[pos:body_end].to_owned(),
          content_type: headers["content-type"],
        },
      ),
    )
    pos = end
    closing = close
  }
  snapshot_check(pos == data.length(), "Multipart epilogue")
  result
}

///|
pub fn encode_multipart_snapshot(
  snapshot : MultipartSnapshot,
  principal : @common.Principal,
) -> Result[(Bytes, String), String] {
  try {
    let envelope : Json = Json::object({
      "version": Json::number(1),
      "principal": principal_to_json(principal),
      "state": snapshot.state,
    })
    let parts : Array[(String, SnapshotPart)] = [
      (
        "state",
        {
          bytes: @utf8.encode(envelope.stringify()),
          content_type: "application/json",
        },
      ),
    ]
    for name, part in snapshot.parts {
      snapshot_check(snapshot_name(name), "Invalid snapshot part name")
      parts.push(
        (
          "part:" + name,
          {
            bytes: part.bytes,
            content_type: snapshot_content_type(part.content_type),
          },
        ),
      )
    }
    let mut nonce = 0
    let mut boundary = ""
    while true {
      boundary = "golem-snapshot-\{nonce}"
      let marker = @utf8.encode("--" + boundary)
      let mut collision = false
      for entry in parts {
        let part = entry.1
        let framed = part.bytes + b"\r\n"
        if snapshot_delimiter(framed, marker, b"\r\n", 0) is Some(_) {
          collision = true
        }
        for i in 0..<framed.length() {
          if snapshot_at(framed, b"\r\n", i) &&
            snapshot_delimiter(framed, marker, b"\r\n", i + 2) is Some(_) {
            collision = true
          }
        }
      }
      if !collision {
        break
      }
      nonce += 1
    }
    let out = @buffer.Buffer()
    for entry in parts {
      let (name, part) = entry
      out.write_string_utf8(
        "--\{boundary}\r\nContent-Disposition: attachment; name=\"\{name}\"\r\nContent-Type: \{part.content_type}\r\n\r\n",
      )
      out.write_bytes(part.bytes)
      out.write_bytes(b"\r\n")
    }
    out.write_string_utf8("--\{boundary}--\r\n")
    Ok((out.to_bytes(), "multipart/mixed; boundary=\{boundary}"))
  } catch {
    SnapshotCodecError(message) => Err(message)
  }
}

///|
// Called only after JSON syntax validation; retains raw SDK record fields and duplicate keys.
fn snapshot_raw_object(
  text : String,
) -> Map[String, String] raise SnapshotCodecError {
  let text = text.trim().to_owned()
  snapshot_check(text.has_prefix("{"), "Metadata must be an object")
  let fields : Map[String, String] = Map([])
  let mut pos = 1
  while pos < text.length() - 1 {
    while text[pos] is (' ' | '\t' | '\r' | '\n') {
      pos += 1
    }
    if text[pos] == '}' {
      break
    }
    let key_start = pos
    pos = snapshot_string_end(text, pos)
    let key = @json.parse(text.unsafe_substring(start=key_start, end=pos)) catch {
      _ => raise SnapshotCodecError("Invalid JSON key")
    }
    guard key is String(key) else {
      raise SnapshotCodecError("Invalid JSON key")
    }
    snapshot_check(!fields.contains(key), "Duplicate metadata field")
    while text[pos] is (' ' | '\t' | '\r' | '\n') {
      pos += 1
    }
    pos += 1
    while text[pos] is (' ' | '\t' | '\r' | '\n') {
      pos += 1
    }
    let start = pos
    let mut depth = 0
    while pos < text.length() {
      match text[pos] {
        '"' => pos = snapshot_string_end(text, pos)
        '[' | '{' => {
          depth += 1
          pos += 1
        }
        ']' | '}' if depth > 0 => {
          depth -= 1
          pos += 1
        }
        ',' | '}' if depth == 0 => break
        _ => pos += 1
      }
    }
    fields[key] = text.unsafe_substring(start~, end=pos).trim().to_owned()
    pos += 1
  }
  fields
}

///|
fn snapshot_string_end(text : String, start : Int) -> Int {
  let mut pos = start + 1
  while text[pos] != '"' {
    if text[pos] == '\\' {
      pos += 1
    }
    pos += 1
  }
  pos + 1
}

///|
pub fn decode_multipart_snapshot(
  data : Bytes,
  mime : String,
) -> Result[(@common.Principal, MultipartSnapshot), String] {
  try {
    let parts = snapshot_parse_parts(data, mime)
    guard parts.search_by(p => p.0 == "state") is Some(index) else {
      raise SnapshotCodecError("Missing state part")
    }
    let part = parts[index].1
    snapshot_check(
      part.content_type == "application/json",
      "Invalid state MIME",
    )
    let text = @utf8.decode(part.bytes) catch {
      _ => raise SnapshotCodecError("Invalid JSON UTF-8")
    }
    let json = @json.parse(text) catch {
      _ => raise SnapshotCodecError("Invalid JSON envelope")
    }
    guard json is Object(fields) else {
      raise SnapshotCodecError("Envelope must be an object")
    }
    let raw = snapshot_raw_object(text)
    snapshot_check(
      raw.get("version") == Some("1"),
      "Multipart version must be integer 1",
    )
    guard raw.get("principal") is Some(principal_raw) &&
      fields.get("principal") is Some(principal_json) &&
      fields.get("state") is Some(state) else {
      raise SnapshotCodecError("Incomplete envelope")
    }
    let principal_fields = snapshot_raw_object(principal_raw)
    guard principal_json is Object(principal_map) else {
      raise SnapshotCodecError("Principal must be an object")
    }
    if principal_map.get("tag") != Some(Json::string("anonymous")) {
      guard principal_fields.get("val") is Some(value) else {
        raise SnapshotCodecError("Missing principal payload")
      }
      ignore(snapshot_raw_object(value))
      if principal_map.get("tag") == Some(Json::string("oidc")) {
        guard principal_map.get("val") is Some(Object(value)) else {
          raise SnapshotCodecError("Invalid principal payload")
        }
        for
          name in [
            "email", "name", "givenName", "familyName", "picture", "preferredUsername",
          ] {
          snapshot_check(
            value.get(name) is (None | Some(Null) | Some(String(_))),
            "Invalid optional principal string",
          )
        }
        snapshot_check(
          value.get("emailVerified")
          is (None | Some(Null) | Some(True) | Some(False)),
          "Invalid optional principal boolean",
        )
      }
    }
    let principal = principal_from_json(principal_json) catch {
      _ => raise SnapshotCodecError("Invalid principal")
    }
    let user_parts : Map[String, SnapshotPart] = Map([])
    for entry in parts {
      let (wire_name, part) = entry
      if wire_name == "state" {
        continue
      }
      snapshot_check(
        wire_name.has_prefix("part:"),
        "Unsupported part namespace",
      )
      let name = wire_name.unsafe_substring(start=5, end=wire_name.length())
      snapshot_check(snapshot_name(name), "Invalid snapshot part name")
      user_parts[name] = {
        bytes: part.bytes,
        content_type: snapshot_content_type(part.content_type),
      }
    }
    Ok((principal, { state, parts: user_parts, }))
  } catch {
    SnapshotCodecError(message) => Err(message)
  }
}
