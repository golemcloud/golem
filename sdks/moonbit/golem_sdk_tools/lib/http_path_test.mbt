///|
test "simple literal path" {
  let (segments, query) = @lib.parse_path_and_query("/api/v1/items")
  debug_inspect(
    segments,
    content=(
      #|[
      #|  { kind: Literal("api") },
      #|  { kind: Literal("v1") },
      #|  { kind: Literal("items") },
      #|]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "path with variables" {
  let (segments, query) = @lib.parse_path_and_query("/api/{city}/weather")
  debug_inspect(
    segments,
    content=(
      #|[
      #|  { kind: Literal("api") },
      #|  { kind: PathVariable("city") },
      #|  { kind: Literal("weather") },
      #|]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "system variables - agent-type" {
  let (segments, query) = @lib.parse_path_and_query("/{agent-type}/status")
  debug_inspect(
    segments,
    content=(
      #|[{ kind: SystemVariable(AgentType) }, { kind: Literal("status") }]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "system variables - agent-version" {
  let (segments, query) = @lib.parse_path_and_query("/{agent-version}/info")
  debug_inspect(
    segments,
    content=(
      #|[{ kind: SystemVariable(AgentVersion) }, { kind: Literal("info") }]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "catch-all remaining path variable" {
  let (segments, query) = @lib.parse_path_and_query("/files/{*path}")
  debug_inspect(
    segments,
    content=(
      #|[{ kind: Literal("files") }, { kind: RemainingPathVariable("path") }]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "path with query" {
  let (segments, query) = @lib.parse_path_and_query(
    "/search?q={query}&limit={n}",
  )
  debug_inspect(
    segments,
    content=(
      #|[{ kind: Literal("search") }]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[
      #|  { query_param_name: "q", variable_name: "query" },
      #|  { query_param_name: "limit", variable_name: "n" },
      #|]
    ),
  )
}

///|
test "empty path" {
  let (segments, query) = @lib.parse_path_and_query("")
  debug_inspect(
    segments,
    content=(
      #|[]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "root path" {
  let (segments, query) = @lib.parse_path_and_query("/")
  debug_inspect(
    segments,
    content=(
      #|[]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "collapsed double slash" {
  let (segments, query) = @lib.parse_path_and_query("//")
  debug_inspect(
    segments,
    content=(
      #|[]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "query only" {
  let (segments, query) = @lib.parse_path_and_query("?q={v}")
  debug_inspect(
    segments,
    content=(
      #|[]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[{ query_param_name: "q", variable_name: "v" }]
    ),
  )
}

///|
test "error: catch-all not at end" {
  let result : Result[_, _] = try? @lib.parse_path_and_query("/{*rest}/more")
  debug_inspect(
    result,
    content=(
      #|Err(HttpPathParseError("remaining path variable {*...} must be the last segment"))
    ),
  )
}

///|
test "error: empty variable name" {
  let result : Result[_, _] = try? @lib.parse_path_and_query("/api/{}")
  debug_inspect(
    result,
    content=(
      #|Err(HttpPathParseError("empty variable name in path segment: {}"))
    ),
  )
}

///|
test "error: query value not a variable" {
  let result : Result[_, _] = try? @lib.parse_path_and_query("/search?q=hello")
  debug_inspect(
    result,
    content=(
      #|Err(HttpPathParseError("query value must be a variable in {name} format, got: hello"))
    ),
  )
}

///|
test "parse_path_only success" {
  let segments = @lib.parse_path_only("/api/{city}/weather")
  debug_inspect(
    segments,
    content=(
      #|[
      #|  { kind: Literal("api") },
      #|  { kind: PathVariable("city") },
      #|  { kind: Literal("weather") },
      #|]
    ),
  )
}

///|
test "parse_path_only error with query" {
  let result : Result[_, _] = try? @lib.parse_path_only("/search?q={v}")
  debug_inspect(
    result,
    content=(
      #|Err(HttpPathParseError("parse_path_only does not accept query parameters, use parse_path_and_query instead"))
    ),
  )
}

///|
test "catch-all at end with trailing slash" {
  let (segments, query) = @lib.parse_path_and_query("/files/{*path}/")
  debug_inspect(
    segments,
    content=(
      #|[{ kind: Literal("files") }, { kind: RemainingPathVariable("path") }]
    ),
  )
  debug_inspect(
    query,
    content=(
      #|[]
    ),
  )
}
