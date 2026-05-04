///|
pub(all) struct PathSegmentInfo {
  kind : PathSegmentKind
} derive(Show, Eq)

///|
pub(all) enum PathSegmentKind {
  Literal(String)
  PathVariable(String)
  RemainingPathVariable(String)
  SystemVariable(SystemVariableKind)
} derive(Show, Eq)

///|
pub(all) enum SystemVariableKind {
  AgentType
  AgentVersion
} derive(Show, Eq)

///|
pub(all) struct QueryVarInfo {
  query_param_name : String
  variable_name : String
} derive(Show, Eq)

///|
pub suberror HttpPathParseError {
  HttpPathParseError(String)
} derive(Show)

///|
/// Parses an HTTP path and query string into structured segments and query variables.
///
/// Path segments are split on `/`, with empty segments collapsed. Variable segments
/// use `{name}` syntax. System variables `{agent-type}` and `{agent-version}` are
/// recognized. A remaining-path variable `{*name}` is only valid as the last segment.
///
/// Query variables must be in `key={varName}` format.
pub fn parse_path_and_query(
  path : String,
) -> (Array[PathSegmentInfo], Array[QueryVarInfo]) raise HttpPathParseError {
  if path == "" {
    return ([], [])
  }
  let qmark = path.find("?")
  let (path_part, query_part) = match qmark {
    Some(idx) =>
      (
        path.unsafe_substring(start=0, end=idx),
        Some(path.unsafe_substring(start=idx + 1, end=path.length())),
      )
    None => (path, None)
  }
  let segments = parse_path_segments(path_part)
  let query_vars = match query_part {
    Some(q) => parse_query_vars(q)
    None => []
  }
  (segments, query_vars)
}

///|
/// Convenience function that parses only path segments. Raises if a query string is present.
pub fn parse_path_only(
  path : String,
) -> Array[PathSegmentInfo] raise HttpPathParseError {
  if path.find("?") is Some(_) {
    raise HttpPathParseError(
      "parse_path_only does not accept query parameters, use parse_path_and_query instead",
    )
  }
  parse_path_segments(path)
}

///|
fn parse_path_segments(
  path_part : String,
) -> Array[PathSegmentInfo] raise HttpPathParseError {
  let segments : Array[PathSegmentInfo] = []
  let raw_parts : Array[String] = path_part
    .split("/")
    .map(fn(sv) { sv.to_string() })
    .collect()
  for i, part in raw_parts {
    if part == "" {
      continue
    }
    let seg = parse_single_segment(part)
    match seg.kind {
      RemainingPathVariable(_) => {
        let remaining_count = raw_parts.length() - i - 1
        let has_more = remaining_count > 0 &&
          not(remaining_count == 1 && raw_parts[raw_parts.length() - 1] == "")
        if has_more {
          raise HttpPathParseError(
            "remaining path variable {*...} must be the last segment",
          )
        }
      }
      _ => ()
    }
    segments.push(seg)
  }
  segments
}

///|
fn parse_single_segment(
  segment : String,
) -> PathSegmentInfo raise HttpPathParseError {
  if segment.has_prefix("{") && segment.has_suffix("}") {
    let inner = segment.unsafe_substring(start=1, end=segment.length() - 1)
    if inner == "" {
      raise HttpPathParseError("empty variable name in path segment: {}")
    }
    if inner == "agent-type" {
      return { kind: SystemVariable(AgentType) }
    }
    if inner == "agent-version" {
      return { kind: SystemVariable(AgentVersion) }
    }
    if inner.has_prefix("*") {
      let name = inner.unsafe_substring(start=1, end=inner.length())
      if name == "" {
        raise HttpPathParseError(
          "empty variable name in remaining path variable: {*}",
        )
      }
      return { kind: RemainingPathVariable(name) }
    }
    return { kind: PathVariable(inner) }
  }
  { kind: Literal(segment) }
}

///|
fn parse_query_vars(
  query : String,
) -> Array[QueryVarInfo] raise HttpPathParseError {
  let vars : Array[QueryVarInfo] = []
  if query == "" {
    return vars
  }
  let entries : Array[String] = query
    .split("&")
    .map(fn(sv) { sv.to_string() })
    .collect()
  for entry in entries {
    if entry == "" {
      continue
    }
    let eq_pos = match entry.find("=") {
      Some(idx) => idx
      None =>
        raise HttpPathParseError("query entry missing '=' separator: \{entry}")
    }
    let key = entry.unsafe_substring(start=0, end=eq_pos)
    let value = entry.unsafe_substring(start=eq_pos + 1, end=entry.length())
    if key == "" {
      raise HttpPathParseError("empty query parameter name in entry: \{entry}")
    }
    if not(value.has_prefix("{")) || not(value.has_suffix("}")) {
      raise HttpPathParseError(
        "query value must be a variable in {name} format, got: \{value}",
      )
    }
    let var_name = value.unsafe_substring(start=1, end=value.length() - 1)
    if var_name == "" {
      raise HttpPathParseError("empty variable name in query value: \{entry}")
    }
    vars.push({ query_param_name: key, variable_name: var_name })
  }
  vars
}
