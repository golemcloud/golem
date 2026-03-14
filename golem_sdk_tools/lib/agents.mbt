///|
/// Represents a parsed parameter/return type from agent source code.
pub(all) enum ParamType {
  Simple(String)
  Optional(ParamType)
  List(ParamType)
  ResultType(ParamType, ParamType)
  Tuple(Array[ParamType])
  MultimodalType(ParamType)
  Parameterized(String, Array[ParamType])
} derive(Show, Eq)

///|
/// Encodes a qualified (cross-package) type name into the `Simple` string format.
pub fn encode_qualified(pkg : String, id : String) -> ParamType {
  Simple("@\{pkg}.\{id}")
}

///|
/// Tries to decode a `Simple` name as a qualified type.
/// Returns `Some((pkg, id))` for names like `"@pkg.Type"`, `None` otherwise.
pub fn decode_qualified(name : String) -> (String, String)? {
  if name.has_prefix("@") {
    match name.find(".") {
      Some(dot) => {
        let pkg = name.unsafe_substring(start=1, end=dot)
        let id = name.unsafe_substring(start=dot + 1, end=name.length())
        if pkg.length() > 0 && id.length() > 0 {
          Some((pkg, id))
        } else {
          None
        }
      }
      None => None
    }
  } else {
    None
  }
}

///|
/// Schema restriction options parsed from `#derive.text_languages` and `#derive.mime_types` annotations.
pub(all) struct SchemaOptions {
  text_languages : Array[String]
  binary_mime_types : Array[String]
} derive(Show, Eq)

///|
/// HTTP endpoint information parsed from `#derive.endpoint` annotations on methods.
pub(all) struct HttpEndpointInfo {
  method : String
  path : String
  headers : Array[(String, String)]
  auth : Bool?
  cors : Array[String]
} derive(Show, Eq)

///|
/// HTTP mount information parsed from `#derive.mount` annotations on agent structs.
pub(all) struct HttpMountInfo {
  path : String
  auth : Bool?
  phantom : Bool
  cors : Array[String]
  webhook_suffix : String?
} derive(Show, Eq)

///|
/// Parsed information about a public agent method.
pub(all) struct MethodInfo {
  name : String
  description : String
  prompt_hint : String?
  params : Array[(String, ParamType)]
  return_type : ParamType?
  param_schema_options : Map[String, SchemaOptions]
  http_endpoint : HttpEndpointInfo?
} derive(Show)

///|
/// Agent execution mode: durable (default) or ephemeral.
pub(all) enum AgentMode {
  Durable
  Ephemeral
} derive(Show, Eq)

///|
/// Snapshotting configuration parsed from `#derive.agent` attributes.
pub(all) enum AgentSnapshotting {
  Disabled
  Default
  Periodic(UInt64)
  EveryNInvocation(UInt)
} derive(Show, Eq)

///|
/// Complete parsed information about an agent, including its constructor, methods, and mode.
pub(all) struct AgentInfo {
  name : String
  description : String
  mode : AgentMode
  snapshotting : AgentSnapshotting
  constructor_params : Array[(String, ParamType)]
  constructor_description : String
  constructor_prompt_hint : String?
  methods : Array[MethodInfo]
  has_init : Bool
  constructor_schema_options : Map[String, SchemaOptions]
  has_snapshottable : Bool
  has_json_derives : Bool
  fields : Array[(String, Bool)]
  http_mount : HttpMountInfo?
} derive(Show)

///|
/// Error raised when agent source parsing fails.
pub suberror AgentParseError {
  AgentParseError(String)
} derive(Show)

///|
/// Converts a MoonBit syntax AST type to the SDK's `ParamType` representation.
pub fn syntax_type_to_param_type(
  ty : @syntax.Type,
) -> ParamType raise AgentParseError {
  match ty {
    Option(ty~, ..) => Optional(syntax_type_to_param_type(ty))
    Tuple(tys~, ..) => {
      let elems : Array[ParamType] = []
      tys.each(fn(t) {
        let pt = syntax_type_to_param_type(t) catch { _ => Simple("Unknown") }
        elems.push(pt)
      })
      Tuple(elems)
    }
    Name(constr_id~, tys~, ..) =>
      match constr_id.id {
        Ident(name~) =>
          if name == "Array" {
            match tys {
              More(inner, ..) => List(syntax_type_to_param_type(inner))
              Empty =>
                raise AgentParseError("Array type requires a type parameter")
            }
          } else if name == "Result" {
            match tys {
              More(ok, tail~) =>
                match tail {
                  More(err, ..) =>
                    ResultType(
                      syntax_type_to_param_type(ok),
                      syntax_type_to_param_type(err),
                    )
                  Empty =>
                    raise AgentParseError(
                      "Result type requires two type parameters",
                    )
                }
              Empty =>
                raise AgentParseError(
                  "Result type requires two type parameters",
                )
            }
          } else if tys.is_empty() {
            Simple(name)
          } else {
            let params : Array[ParamType] = []
            tys.each(fn(t) {
              let pt = syntax_type_to_param_type(t) catch {
                _ => Simple("Unknown")
              }
              params.push(pt)
            })
            Parameterized(name, params)
          }
        Dot(pkg~, id~) =>
          if pkg == "types" && id == "Multimodal" {
            match tys {
              More(inner, ..) =>
                MultimodalType(syntax_type_to_param_type(inner))
              Empty =>
                raise AgentParseError(
                  "Multimodal type requires a type parameter",
                )
            }
          } else if tys.is_empty() {
            encode_qualified(pkg, id)
          } else {
            let params : Array[ParamType] = []
            tys.each(fn(t) {
              let pt = syntax_type_to_param_type(t) catch {
                _ => Simple("Unknown")
              }
              params.push(pt)
            })
            Parameterized("@\{pkg}.\{id}", params)
          }
      }
    _ => raise AgentParseError("Unsupported type syntax")
  }
}

///|
/// Extracts the content of a doc comment as a trimmed string.
fn extract_doc_string(doc : @syntax.DocString) -> String {
  let sb = StringBuilder::new()
  let mut first = true
  doc.content.each(fn(line) {
    if first {
      first = false
    } else {
      sb.write_char('\n')
    }
    sb.write_string(line)
  })
  sb.to_string().trim().to_string()
}

///|
/// Extracts the agent mode (durable or ephemeral) from `#derive.agent` attributes.
fn extract_agent_mode(
  attrs : @list.List[@attribute.Attribute],
) -> AgentMode raise AgentParseError {
  let mut result = Durable
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "agent" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) =>
              if s == "ephemeral" {
                result = Ephemeral
              } else if s == "durable" {
                result = Durable
              }
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Extracts the snapshotting configuration from `#derive.agent` attributes.
/// Recognized labeled prop: `snapshotting="disabled"`, `snapshotting="default"`,
/// `snapshotting="periodic(N)"`, `snapshotting="every_n(N)"`.
fn extract_agent_snapshotting(
  attrs : @list.List[@attribute.Attribute],
) -> AgentSnapshotting raise AgentParseError {
  let mut result = Disabled
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "agent" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Labeled("snapshotting", String(s)) => {
              let parsed = parse_snapshotting_value(s) catch {
                e => abort(e.to_string())
              }
              result = parsed
            }
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Parses a snapshotting value string like "disabled", "default",
/// "periodic(100)", "periodic(10 minutes)", "every_n(5)".
fn parse_snapshotting_value(
  s : String,
) -> AgentSnapshotting raise AgentParseError {
  if s == "disabled" {
    Disabled
  } else if s == "default" {
    Default
  } else if s.has_prefix("periodic(") && s.has_suffix(")") {
    let inner = s.unsafe_substring(
      start="periodic(".length(),
      end=s.length() - 1,
    )
    let seconds = parse_duration_seconds(inner) catch {
      e => raise AgentParseError(e.to_string())
    }
    Periodic(seconds)
  } else if s.has_prefix("every_n(") && s.has_suffix(")") {
    let num_str = s.unsafe_substring(
      start="every_n(".length(),
      end=s.length() - 1,
    )
    match parse_uint(num_str) {
      Some(n) => EveryNInvocation(n)
      None =>
        raise AgentParseError(
          "Invalid every_n snapshotting value: '\{num_str}' is not a valid unsigned integer",
        )
    }
  } else {
    raise AgentParseError(
      "Unknown snapshotting value '\{s}'. Expected: disabled, default, periodic(<duration>), or every_n(N)",
    )
  }
}

///|
/// Parses a human-readable duration string into seconds (UInt64).
///
/// Supported formats:
/// - Plain number: `"100"` → 100 seconds
/// - Single unit: `"10 minutes"` → 600 seconds
/// - Multiple units: `"1 hour 30 minutes"` → 5400 seconds
///
/// Supported units (singular and plural):
/// `s`, `sec`, `secs`, `second`, `seconds`,
/// `m`, `min`, `mins`, `minute`, `minutes`,
/// `h`, `hr`, `hrs`, `hour`, `hours`,
/// `d`, `day`, `days`.
pub fn parse_duration_seconds(s : String) -> UInt64 raise AgentParseError {
  let trimmed = s.trim().to_string()
  // Try plain number first
  match parse_uint64(trimmed) {
    Some(n) => return n
    None => ()
  }
  // Parse as "<number> <unit> [<number> <unit> ...]"
  let tokens = split_whitespace(trimmed)
  if tokens.length() == 0 {
    raise AgentParseError("Empty duration string")
  }
  if tokens.length() % 2 != 0 {
    raise AgentParseError(
      "Invalid duration '\{trimmed}': expected '<number> <unit>' pairs (e.g. '10 minutes', '1 hour 30 seconds')",
    )
  }
  let mut total : UInt64 = 0
  let mut i = 0
  while i < tokens.length() {
    let num_str = tokens[i]
    let unit_str = tokens[i + 1]
    let num = match parse_uint64(num_str) {
      Some(n) => n
      None =>
        raise AgentParseError(
          "Invalid duration: '\{num_str}' is not a valid number",
        )
    }
    let multiplier = duration_unit_to_seconds(unit_str) catch {
      e => raise AgentParseError(e.to_string())
    }
    total = total + num * multiplier
    i = i + 2
  }
  total
}

///|
/// Returns the number of seconds for a given time unit string.
fn duration_unit_to_seconds(unit : String) -> UInt64 raise AgentParseError {
  if unit == "s" ||
    unit == "sec" ||
    unit == "secs" ||
    unit == "second" ||
    unit == "seconds" {
    1UL
  } else if unit == "m" ||
    unit == "min" ||
    unit == "mins" ||
    unit == "minute" ||
    unit == "minutes" {
    60UL
  } else if unit == "h" ||
    unit == "hr" ||
    unit == "hrs" ||
    unit == "hour" ||
    unit == "hours" {
    3600UL
  } else if unit == "d" || unit == "day" || unit == "days" {
    86400UL
  } else {
    raise AgentParseError(
      "Unknown duration unit '\{unit}'. Expected: s/sec/second(s), m/min/minute(s), h/hr/hour(s), d/day(s)",
    )
  }
}

///|
/// Splits a string by whitespace into non-empty tokens.
fn split_whitespace(s : String) -> Array[String] {
  let tokens : Array[String] = []
  let buf = StringBuilder::new()
  for ch in s {
    if ch == ' ' || ch == '\t' {
      if buf.to_string().length() > 0 {
        tokens.push(buf.to_string())
        buf.reset()
      }
    } else {
      buf.write_char(ch)
    }
  }
  if buf.to_string().length() > 0 {
    tokens.push(buf.to_string())
  }
  tokens
}

///|
/// Parses a decimal string as a UInt64. Returns None if the string is empty or contains non-digits.
fn parse_uint64(s : String) -> UInt64? {
  if s.length() == 0 {
    return None
  }
  let mut result : UInt64 = 0
  for ch in s {
    if ch >= '0' && ch <= '9' {
      result = result * 10 + (ch.to_int() - '0'.to_int()).to_uint64()
    } else {
      return None
    }
  }
  Some(result)
}

///|
/// Parses a decimal string as a UInt. Returns None if the string is empty or contains non-digits.
fn parse_uint(s : String) -> UInt? {
  if s.length() == 0 {
    return None
  }
  let mut result : UInt = 0
  for ch in s {
    if ch >= '0' && ch <= '9' {
      result = result * 10 + (ch.to_int() - '0'.to_int()).reinterpret_as_uint()
    } else {
      return None
    }
  }
  Some(result)
}

///|
/// Extracts the prompt hint string from a `#derive.prompt_hint` attribute, if present.
fn extract_prompt_hint(attrs : @list.List[@attribute.Attribute]) -> String? {
  let mut result : String? = None
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "prompt_hint" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => result = Some(s)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Returns true if the parameter type is a `Multimodal[T]`.
pub fn is_multimodal_type(pt : ParamType) -> Bool {
  match pt {
    MultimodalType(_) => true
    _ => false
  }
}

///|
/// Returns true if the parameter type cannot be serialized for RPC transport.
/// Currently always returns false.
pub fn is_rpc_incompatible_type(_pt : ParamType) -> Bool {
  false
}

///|
/// Returns true if the parameter type is `UnstructuredText`.
pub fn is_unstructured_text_type(pt : ParamType) -> Bool {
  match pt {
    Simple(name) =>
      name == "UnstructuredText" || name == "@types.UnstructuredText"
    _ => false
  }
}

///|
/// Returns true if the parameter type is `UnstructuredBinary`.
pub fn is_unstructured_binary_type(pt : ParamType) -> Bool {
  match pt {
    Simple(name) =>
      name == "UnstructuredBinary" || name == "@types.UnstructuredBinary"
    _ => false
  }
}

///|
/// Returns true if the parameter type is `Principal` or `@common.Principal`.
pub fn is_principal_type(pt : ParamType) -> Bool {
  match pt {
    Simple(name) => name == "Principal" || name == "@common.Principal"
    _ => false
  }
}

///|
/// Filters out `Principal` parameters from a parameter list.
pub fn visible_params(
  params : Array[(String, ParamType)],
) -> Array[(String, ParamType)] {
  params.filter(fn(p) { not(is_principal_type(p.1)) })
}

///|
/// Returns the index of the first `Principal` parameter, if any.
pub fn find_principal_index(params : Array[(String, ParamType)]) -> Int? {
  for i, p in params {
    if is_principal_type(p.1) {
      return Some(i)
    }
  }
  None
}

///|
/// Validates that a `Multimodal` type is not nested inside another container type.
fn validate_no_nested_multimodal(
  param_name : String,
  pt : ParamType,
) -> Unit raise AgentParseError {
  match pt {
    Simple(_) | MultimodalType(_) => ()
    Parameterized(_, params) =>
      for p in params {
        validate_no_multimodal_inside("Parameterized", param_name, p)
      }
    Optional(inner) =>
      validate_no_multimodal_inside("Option", param_name, inner)
    List(inner) => validate_no_multimodal_inside("Array", param_name, inner)
    ResultType(ok, err) => {
      validate_no_multimodal_inside("Result", param_name, ok)
      validate_no_multimodal_inside("Result", param_name, err)
    }
    Tuple(elems) =>
      for elem in elems {
        validate_no_multimodal_inside("Tuple", param_name, elem)
      }
  }
}

///|
/// Recursively checks that no `Multimodal` type appears inside a wrapper type.
fn validate_no_multimodal_inside(
  wrapper : String,
  param_name : String,
  pt : ParamType,
) -> Unit raise AgentParseError {
  match pt {
    MultimodalType(_) =>
      raise AgentParseError(
        "Multimodal cannot be nested inside \{wrapper} for parameter '\{param_name}'; it must be a direct parameter or return type",
      )
    Simple(_) => ()
    Parameterized(_, params) =>
      for p in params {
        validate_no_multimodal_inside(wrapper, param_name, p)
      }
    Optional(inner) => validate_no_multimodal_inside(wrapper, param_name, inner)
    List(inner) => validate_no_multimodal_inside(wrapper, param_name, inner)
    ResultType(ok, err) => {
      validate_no_multimodal_inside(wrapper, param_name, ok)
      validate_no_multimodal_inside(wrapper, param_name, err)
    }
    Tuple(elems) =>
      for elem in elems {
        validate_no_multimodal_inside(wrapper, param_name, elem)
      }
  }
}

///|
/// Extracts `#derive.text_languages` and `#derive.mime_types` restrictions from attributes, grouped by parameter name.
fn extract_schema_restrictions(
  attrs : @list.List[@attribute.Attribute],
) -> Map[String, SchemaOptions] {
  let result : Map[String, SchemaOptions] = {}
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "text_languages" }, props)) => {
        let strings : Array[String] = []
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => strings.push(s)
            _ => ()
          }
        })
        if strings.length() >= 2 {
          let param_name = strings[0]
          let codes : Array[String] = []
          for i in 1..<strings.length() {
            codes.push(strings[i])
          }
          match result.get(param_name) {
            Some(existing) =>
              result[param_name] = {
                text_languages: existing.text_languages + codes,
                binary_mime_types: existing.binary_mime_types,
              }
            None =>
              result[param_name] = {
                text_languages: codes,
                binary_mime_types: [],
              }
          }
        }
      }
      Some(Apply({ qual: Some("derive"), name: "mime_types" }, props)) => {
        let strings : Array[String] = []
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => strings.push(s)
            _ => ()
          }
        })
        if strings.length() >= 2 {
          let param_name = strings[0]
          let mimes : Array[String] = []
          for i in 1..<strings.length() {
            mimes.push(strings[i])
          }
          match result.get(param_name) {
            Some(existing) =>
              result[param_name] = {
                text_languages: existing.text_languages,
                binary_mime_types: existing.binary_mime_types + mimes,
              }
            None =>
              result[param_name] = {
                text_languages: [],
                binary_mime_types: mimes,
              }
          }
        }
      }
      _ => ()
    }
  })
  result
}

///|
/// Extracts the mount path from a `#derive.mount("path")` attribute.
fn extract_mount_path(attrs : @list.List[@attribute.Attribute]) -> String? {
  let mut result : String? = None
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "mount" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => result = Some(s)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Extracts the mount auth flag from a `#derive.mount_auth(true/false)` attribute.
fn extract_mount_auth(attrs : @list.List[@attribute.Attribute]) -> Bool? {
  let mut result : Bool? = None
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "mount_auth" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(Bool(b)) => result = Some(b)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Extracts CORS origins from a `#derive.mount_cors("o1", "o2")` attribute.
fn extract_mount_cors(
  attrs : @list.List[@attribute.Attribute],
) -> Array[String] {
  let result : Array[String] = []
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "mount_cors" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => result.push(s)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Extracts the webhook suffix from a `#derive.mount_webhook("path")` attribute.
fn extract_mount_webhook(attrs : @list.List[@attribute.Attribute]) -> String? {
  let mut result : String? = None
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "mount_webhook" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => result = Some(s)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Extracts the phantom flag from a `#derive.mount_phantom(true/false)` attribute. Default: false.
fn extract_mount_phantom(attrs : @list.List[@attribute.Attribute]) -> Bool {
  let mut result = false
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "mount_phantom" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(Bool(b)) => result = b
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
let http_methods : Array[String] = [
  "get", "post", "put", "delete", "patch", "head", "options", "trace", "connect",
]

///|
/// Extracts the HTTP endpoint from a `#derive.endpoint(get="/path")` attribute.
fn extract_endpoint(
  attrs : @list.List[@attribute.Attribute],
) -> HttpEndpointInfo? raise AgentParseError {
  let mut result : HttpEndpointInfo? = None
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "endpoint" }, props)) => {
        let found : Array[(String, String)] = []
        props.each(fn(prop) {
          match prop {
            Labeled(label, String(path)) =>
              if http_methods.contains(label) {
                found.push((label, path))
              }
            _ => ()
          }
        })
        if found.length() > 1 {
          raise AgentParseError(
            "Multiple HTTP methods in #derive.endpoint: found " +
            found.length().to_string(),
          )
        } else if found.length() == 0 {
          raise AgentParseError(
            "#derive.endpoint requires exactly one HTTP method (e.g. get=\"/path\")",
          )
        } else {
          let (method, path) = found[0]
          result = Some({ method, path, headers: [], auth: None, cors: [] })
        }
      }
      _ => ()
    }
  })
  result
}

///|
/// Extracts endpoint headers from `#derive.endpoint_header("Header-Name", "param")` attributes.
fn extract_endpoint_headers(
  attrs : @list.List[@attribute.Attribute],
) -> Array[(String, String)] {
  let result : Array[(String, String)] = []
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "endpoint_header" }, props)) => {
        let strings : Array[String] = []
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => strings.push(s)
            _ => ()
          }
        })
        if strings.length() >= 2 {
          result.push((strings[0], strings[1]))
        }
      }
      _ => ()
    }
  })
  result
}

///|
/// Extracts the endpoint auth flag from a `#derive.endpoint_auth(true/false)` attribute.
fn extract_endpoint_auth(attrs : @list.List[@attribute.Attribute]) -> Bool? {
  let mut result : Bool? = None
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "endpoint_auth" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(Bool(b)) => result = Some(b)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Extracts CORS origins from a `#derive.endpoint_cors("o1", "o2")` attribute.
fn extract_endpoint_cors(
  attrs : @list.List[@attribute.Attribute],
) -> Array[String] {
  let result : Array[String] = []
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "endpoint_cors" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => result.push(s)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
/// Builds a composite `HttpEndpointInfo` from individual endpoint attributes.
fn build_endpoint_info(
  attrs : @list.List[@attribute.Attribute],
) -> HttpEndpointInfo? raise AgentParseError {
  let endpoint = extract_endpoint(attrs)
  match endpoint {
    Some(ep) => {
      let headers = extract_endpoint_headers(attrs)
      let auth = extract_endpoint_auth(attrs)
      let cors = extract_endpoint_cors(attrs)
      Some({ ..ep, headers, auth, cors })
    }
    None => None
  }
}

///|
/// Extracts method parameter names and types from a function declaration AST.
fn extract_method_params(
  fun_decl : @syntax.FunDecl,
) -> Array[(String, ParamType)] raise AgentParseError {
  let params : Array[(String, ParamType)] = []
  match fun_decl.decl_params {
    Some(ps) =>
      ps.each(fn(p) {
        match p {
          Positional(binder~, ty=Some(ty)) | Labelled(binder~, ty=Some(ty)) => {
            let param_name = binder.name
            let param_type = syntax_type_to_param_type(ty) catch {
              _ => Simple("Unknown")
            }
            params.push((param_name, param_type))
          }
          _ => ()
        }
      })
    None => ()
  }
  params
}

///|
/// Extracts the return type from a function declaration, returning `None` for `Unit`.
fn extract_return_type(
  fun_decl : @syntax.FunDecl,
) -> ParamType? raise AgentParseError {
  match fun_decl.return_type {
    Some(ty) => {
      let pt = syntax_type_to_param_type(ty)
      if pt == Simple("Unit") {
        None
      } else {
        Some(pt)
      }
    }
    None => None
  }
}

///|
/// Parses agent definitions from source files. Finds `#derive.agent` structs,
/// their constructors, and public methods.
pub fn parse_agents(
  files : Array[(String, String)],
) -> Array[AgentInfo] raise AgentParseError {
  let agent_names : Map[String, String] = {}
  let agent_modes : Map[String, AgentMode] = {}
  let agent_snapshotting : Map[String, AgentSnapshotting] = {}
  let agent_constructors : Map[
    String,
    (Array[(String, ParamType)], String, String?, Map[String, SchemaOptions]),
  ] = {}
  let agent_methods : Map[String, Array[MethodInfo]] = {}
  let agent_snapshottable : Map[String, Bool] = {}
  let agent_json_derives : Map[String, Bool] = {}
  let agent_fields : Map[String, Array[(String, Bool)]] = {}
  let agent_mounts : Map[String, HttpMountInfo?] = {}
  let mut has_init = false
  // Parse all files first, then run passes over all AST nodes
  // to avoid file-ordering dependencies
  let all_impls : Array[@syntax.Impl] = []
  for pair in files {
    let (_filename, content) = pair
    let (impls, _reports) = @parser.parse_string(content)
    impls.each(fn(impl_) { all_impls.push(impl_) })
  }
  // First pass: find #derive.agent annotated structs
  all_impls.each(fn(impl_) {
    match impl_ {
      TopTypeDef(type_decl) => {
        let mut is_agent = false
        type_decl.attrs.each(fn(attr) {
          if attr.raw == "#derive.agent" ||
            attr.raw.has_prefix("#derive.agent(") {
            is_agent = true
          }
        })
        if is_agent {
          let name = type_decl.tycon
          let description = extract_doc_string(type_decl.doc)
          let desc = if description == "" { name } else { description }
          let mode = extract_agent_mode(type_decl.attrs) catch { _ => Durable }
          let snapshotting = extract_agent_snapshotting(type_decl.attrs) catch {
            _ => Disabled
          }
          agent_names[name] = desc
          agent_modes[name] = mode
          agent_snapshotting[name] = snapshotting
          agent_methods[name] = []
          // Check for derive(ToJson) and derive(FromJson)
          let mut has_to_json = false
          let mut has_from_json = false
          type_decl.deriving.each(fn(d) {
            match d.type_name.name {
              Ident(name~) =>
                if name == "ToJson" {
                  has_to_json = true
                } else if name == "FromJson" {
                  has_from_json = true
                }
              Dot(id~, ..) =>
                if id == "FromJson" {
                  has_from_json = true
                } else if id == "ToJson" {
                  has_to_json = true
                }
            }
          })
          agent_json_derives[name] = has_to_json && has_from_json
          // Extract struct field names and mutability
          let field_info : Array[(String, Bool)] = []
          match type_decl.components {
            Record(fields~, ..) =>
              fields.each(fn(field) {
                field_info.push((field.name.label, field.mut_))
              })
            _ => ()
          }
          agent_fields[name] = field_info
          // Extract HTTP mount info
          let mount_path = extract_mount_path(type_decl.attrs)
          let mount_info = match mount_path {
            Some(path) =>
              Some({
                path,
                auth: extract_mount_auth(type_decl.attrs),
                phantom: extract_mount_phantom(type_decl.attrs),
                cors: extract_mount_cors(type_decl.attrs),
                webhook_suffix: extract_mount_webhook(type_decl.attrs),
              })
            None => None
          }
          agent_mounts[name] = mount_info
        }
      }
      _ => ()
    }
  })
  // Second pass: find ::new constructors, public methods, and fn init
  all_impls.each(fn(impl_) {
    match impl_ {
      TopFuncDef(fun_decl~, ..) => {
        // Check for fn init
        if fun_decl.type_name is None && fun_decl.name.name == "init" {
          has_init = true
        }
        match fun_decl.type_name {
          Some({ name: Ident(name~), .. }) =>
            if agent_names.contains(name) {
              if fun_decl.name.name == "new" {
                // Constructor
                let params = extract_method_params(fun_decl) catch { _ => [] }
                let doc = extract_doc_string(fun_decl.doc)
                let desc = if doc == "" { "Creates a new \{name}" } else { doc }
                let hint = extract_prompt_hint(fun_decl.attrs)
                let restrictions = extract_schema_restrictions(fun_decl.attrs)
                agent_constructors[name] = (params, desc, hint, restrictions)
              } else if fun_decl.vis is Pub(..) {
                // Public method — check if first param is Self
                let mut is_method = false
                match fun_decl.decl_params {
                  Some(ps) => {
                    let mut first = true
                    ps.each(fn(p) {
                      if first {
                        first = false
                        match p {
                          Positional(ty=Some(Name(constr_id~, ..)), ..) =>
                            match constr_id.id {
                              Ident(name~) =>
                                if name == "Self" {
                                  is_method = true
                                }
                              _ => ()
                            }
                          _ => ()
                        }
                      }
                    })
                  }
                  None => ()
                }
                if is_method {
                  // Extract params (skip self)
                  let all_params = extract_method_params(fun_decl) catch {
                    _ => []
                  }
                  // Remove the first param (self : Self)
                  let params : Array[(String, ParamType)] = []
                  for i in 1..<all_params.length() {
                    params.push(all_params[i])
                  }
                  let return_type = extract_return_type(fun_decl) catch {
                    _ => None
                  }
                  let doc = extract_doc_string(fun_decl.doc)
                  let method_name = fun_decl.name.name
                  let desc = if doc == "" { method_name } else { doc }
                  let hint = extract_prompt_hint(fun_decl.attrs)
                  let restrictions = extract_schema_restrictions(fun_decl.attrs)
                  let endpoint = build_endpoint_info(fun_decl.attrs)
                  match agent_methods.get(name) {
                    Some(methods) =>
                      methods.push({
                        name: method_name,
                        description: desc,
                        prompt_hint: hint,
                        params,
                        return_type,
                        param_schema_options: restrictions,
                        http_endpoint: endpoint,
                      })
                    None => ()
                  }
                }
              }
            }
          _ => ()
        }
      }
      _ => ()
    }
  })
  // Third pass: detect Snapshottable trait implementations
  all_impls.each(fn(impl_) {
    match impl_ {
      TopImpl(trait_~, self_ty~, ..) => {
        let is_snapshottable = match trait_.name {
          Ident(name~) => name == "Snapshottable"
          Dot(pkg~, id~) => id == "Snapshottable" && pkg == "agents"
        }
        if is_snapshottable {
          match self_ty {
            Some(Name(constr_id~, ..)) =>
              match constr_id.id {
                Ident(name~) =>
                  if agent_names.contains(name) {
                    agent_snapshottable[name] = true
                  }
                _ => ()
              }
            _ => ()
          }
        }
      }
      _ => ()
    }
  })
  // Validate
  let agents : Array[AgentInfo] = []
  for name, description in agent_names {
    match agent_constructors.get(name) {
      Some((params, ctor_desc, ctor_hint, ctor_restrictions)) =>
        agents.push({
          name,
          description,
          mode: agent_modes.get(name).unwrap_or(Durable),
          snapshotting: agent_snapshotting.get(name).unwrap_or(Disabled),
          constructor_params: params,
          constructor_description: ctor_desc,
          constructor_prompt_hint: ctor_hint,
          methods: agent_methods.get(name).unwrap_or([]),
          has_init,
          constructor_schema_options: ctor_restrictions,
          has_snapshottable: agent_snapshottable.get(name).unwrap_or(false),
          has_json_derives: agent_json_derives.get(name).unwrap_or(false),
          fields: agent_fields.get(name).unwrap_or([]),
          http_mount: agent_mounts.get(name).unwrap_or(None),
        })
      None => raise AgentParseError("Agent '\{name}' has no ::new constructor")
    }
  }
  // Validate multimodal and restriction targets
  for agent in agents {
    // Validate constructor params
    for param in agent.constructor_params {
      let (pname, pt) = param
      validate_no_nested_multimodal(pname, pt)
    }
    // Validate multimodal is sole non-Principal constructor param
    let ctor_visible = visible_params(agent.constructor_params)
    let has_multimodal_ctor = ctor_visible
      .iter()
      .find_first(fn(p) { is_multimodal_type(p.1) })
    if has_multimodal_ctor is Some(_) && ctor_visible.length() != 1 {
      raise AgentParseError(
        "Multimodal parameter in constructor of agent '\{agent.name}' must be the only non-Principal parameter",
      )
    }
    for rname, _ in agent.constructor_schema_options {
      let param_type = agent.constructor_params
        .iter()
        .find_first(fn(p) { p.0 == rname })
      if param_type is None {
        raise AgentParseError(
          "#derive.text_languages/#derive.mime_types references unknown constructor parameter '\{rname}'",
        )
      }
    }
    // Validate methods
    for meth in agent.methods {
      for param in meth.params {
        let (pname, pt) = param
        validate_no_nested_multimodal(pname, pt)
      }
      match meth.return_type {
        Some(rt) => validate_no_nested_multimodal("return", rt)
        None => ()
      }
      // Validate multimodal params (only non-Principal params count)
      let meth_visible = visible_params(meth.params)
      let has_multimodal_param = meth_visible
        .iter()
        .find_first(fn(p) { is_multimodal_type(p.1) })
      if has_multimodal_param is Some(_) && meth_visible.length() != 1 {
        raise AgentParseError(
          "Multimodal parameter in method '\{meth.name}' must be the only non-Principal parameter",
        )
      }
      for rname, _ in meth.param_schema_options {
        let param_type = meth.params.iter().find_first(fn(p) { p.0 == rname })
        if param_type is None {
          raise AgentParseError(
            "#derive.text_languages/#derive.mime_types references unknown parameter '\{rname}' of method '\{meth.name}'",
          )
        }
      }
      // Validate at most one Principal param per method
      let principal_count = meth.params
        .filter(fn(p) { is_principal_type(p.1) })
        .length()
      if principal_count > 1 {
        raise AgentParseError(
          "Method '\{meth.name}' of agent '\{agent.name}' has \{principal_count} Principal parameters; at most one is allowed",
        )
      }
    }
    // Validate at most one Principal param per constructor
    let ctor_principal_count = agent.constructor_params
      .filter(fn(p) { is_principal_type(p.1) })
      .length()
    if ctor_principal_count > 1 {
      raise AgentParseError(
        "Constructor of agent '\{agent.name}' has \{ctor_principal_count} Principal parameters; at most one is allowed",
      )
    }
  }
  validate_http_routes(agents)
  agents
}
