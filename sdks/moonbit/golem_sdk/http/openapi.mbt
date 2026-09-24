///|
/// Safe categories only: provider document contents are never included.
pub suberror OpenApiJsonError {
  OpenApiJsonError(String)
} derive(Eq)

///|
/// Serializes a JSON OpenAPI document without fetching or resolving references.
/// The host validates the OpenAPI 3.1 subset and merges it with other providers.
/// Object keys are sorted; array order and opaque payloads are preserved.
pub fn openapi_json(document : Json) -> String raise OpenApiJsonError {
  guard document is Object(_) else { raise OpenApiJsonError("document-object") }
  let out = StringBuilder()
  let bytes = Ref(0)
  write_openapi_json(document, out, bytes, 0)
  out.to_string()
}

///|
fn append_openapi_json(
  out : StringBuilder,
  bytes : Ref[Int],
  text : String,
) -> Unit raise OpenApiJsonError {
  if text.length() > 1048576 {
    raise OpenApiJsonError("provider-size")
  }
  let encoded = @utf8.encode(text)
  bytes.val += encoded.length()
  if bytes.val > 1048576 {
    raise OpenApiJsonError("provider-size")
  }
  out.write_string(text)
}

///|
fn openapi_string(value : String) -> String raise OpenApiJsonError {
  if value.length() > 1048576 {
    raise OpenApiJsonError("provider-size")
  }
  let encoded = @utf8.encode(value)
  let decoded = @utf8.decode(encoded) catch {
    _ => raise OpenApiJsonError("invalid-unicode")
  }
  if decoded != value {
    raise OpenApiJsonError("invalid-unicode")
  }
  value.to_json().stringify()
}

///|
fn write_openapi_json(
  value : Json,
  out : StringBuilder,
  bytes : Ref[Int],
  depth : Int,
) -> Unit raise OpenApiJsonError {
  match value {
    Object(fields) => {
      if depth >= 64 {
        raise OpenApiJsonError("json-depth")
      }
      if fields.length() > 1048576 / 5 {
        raise OpenApiJsonError("provider-size")
      }
      append_openapi_json(out, bytes, "{")
      let keys = fields.keys().collect()
      keys.sort()
      for i, key in keys {
        if i > 0 {
          append_openapi_json(out, bytes, ",")
        }
        append_openapi_json(out, bytes, openapi_string(key))
        append_openapi_json(out, bytes, ":")
        write_openapi_json(fields[key], out, bytes, depth + 1)
      }
      append_openapi_json(out, bytes, "}")
    }
    Array(items) => {
      if depth >= 64 {
        raise OpenApiJsonError("json-depth")
      }
      append_openapi_json(out, bytes, "[")
      for i, item in items {
        if i > 0 {
          append_openapi_json(out, bytes, ",")
        }
        write_openapi_json(item, out, bytes, depth + 1)
      }
      append_openapi_json(out, bytes, "]")
    }
    String(text) => append_openapi_json(out, bytes, openapi_string(text))
    Number(number, ..) => {
      if number.is_nan() || number.is_inf() {
        raise OpenApiJsonError("invalid-number")
      }
      let token = value.stringify()
      if token.length() > 1048576 {
        raise OpenApiJsonError("provider-size")
      }
      if !is_json_number(token) {
        raise OpenApiJsonError("invalid-number")
      }
      append_openapi_json(out, bytes, token)
    }
    _ => append_openapi_json(out, bytes, value.stringify())
  }
}

///|
/// Keep exact number lexemes (including integers beyond Double precision), but
/// never trust Json::number's optional representation to be valid JSON syntax.
fn is_json_number(token : String) -> Bool {
  let n = token.length()
  let mut i = 0
  if i < n && token[i] == '-' {
    i += 1
  }
  if i >= n {
    return false
  }
  if token[i] == '0' {
    i += 1
  } else {
    if !(token[i] is ('1'..='9')) {
      return false
    }
    while i < n && token[i] is ('0'..='9') {
      i += 1
    }
  }
  if i < n && token[i] == '.' {
    i += 1
    let start = i
    while i < n && token[i] is ('0'..='9') {
      i += 1
    }
    if i == start {
      return false
    }
  }
  if i < n && token[i] is ('e' | 'E') {
    i += 1
    if i < n && token[i] is ('+' | '-') {
      i += 1
    }
    let start = i
    while i < n && token[i] is ('0'..='9') {
      i += 1
    }
    if i == start {
      return false
    }
  }
  i == n
}
