///|
test "simple literal path" {
  let (segments, query) = @lib.parse_path_and_query("/api/v1/items")
  inspect(
    segments,
    content=(
      #|[{kind: Literal("api")}, {kind: Literal("v1")}, {kind: Literal("items")}]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "path with variables" {
  let (segments, query) = @lib.parse_path_and_query("/api/{city}/weather")
  inspect(
    segments,
    content=(
      #|[{kind: Literal("api")}, {kind: PathVariable("city")}, {kind: Literal("weather")}]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "system variables - agent-type" {
  let (segments, query) = @lib.parse_path_and_query("/{agent-type}/status")
  inspect(
    segments,
    content=(
      #|[{kind: SystemVariable(AgentType)}, {kind: Literal("status")}]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "system variables - agent-version" {
  let (segments, query) = @lib.parse_path_and_query("/{agent-version}/info")
  inspect(
    segments,
    content=(
      #|[{kind: SystemVariable(AgentVersion)}, {kind: Literal("info")}]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "catch-all remaining path variable" {
  let (segments, query) = @lib.parse_path_and_query("/files/{*path}")
  inspect(
    segments,
    content=(
      #|[{kind: Literal("files")}, {kind: RemainingPathVariable("path")}]
    ),
  )
  inspect(
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
  inspect(
    segments,
    content=(
      #|[{kind: Literal("search")}]
    ),
  )
  inspect(
    query,
    content=(
      #|[{query_param_name: "q", variable_name: "query"}, {query_param_name: "limit", variable_name: "n"}]
    ),
  )
}

///|
test "empty path" {
  let (segments, query) = @lib.parse_path_and_query("")
  inspect(
    segments,
    content=(
      #|[]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "root path" {
  let (segments, query) = @lib.parse_path_and_query("/")
  inspect(
    segments,
    content=(
      #|[]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "collapsed double slash" {
  let (segments, query) = @lib.parse_path_and_query("//")
  inspect(
    segments,
    content=(
      #|[]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "query only" {
  let (segments, query) = @lib.parse_path_and_query("?q={v}")
  inspect(
    segments,
    content=(
      #|[]
    ),
  )
  inspect(
    query,
    content=(
      #|[{query_param_name: "q", variable_name: "v"}]
    ),
  )
}

///|
test "error: catch-all not at end" {
  let result : Result[_, _] = try? @lib.parse_path_and_query("/{*rest}/more")
  inspect(
    result,
    content=(
      #|Err(HttpPathParseError("remaining path variable {*...} must be the last segment"))
    ),
  )
}

///|
test "error: empty variable name" {
  let result : Result[_, _] = try? @lib.parse_path_and_query("/api/{}")
  inspect(
    result,
    content=(
      #|Err(HttpPathParseError("empty variable name in path segment: {}"))
    ),
  )
}

///|
test "error: query value not a variable" {
  let result : Result[_, _] = try? @lib.parse_path_and_query("/search?q=hello")
  inspect(
    result,
    content=(
      #|Err(HttpPathParseError("query value must be a variable in {name} format, got: hello"))
    ),
  )
}

///|
test "parse_path_only success" {
  let segments = @lib.parse_path_only("/api/{city}/weather")
  inspect(
    segments,
    content=(
      #|[{kind: Literal("api")}, {kind: PathVariable("city")}, {kind: Literal("weather")}]
    ),
  )
}

///|
test "parse_path_only error with query" {
  let result : Result[_, _] = try? @lib.parse_path_only("/search?q={v}")
  inspect(
    result,
    content=(
      #|Err(HttpPathParseError("parse_path_only does not accept query parameters, use parse_path_and_query instead"))
    ),
  )
}

///|
test "catch-all at end with trailing slash" {
  let (segments, query) = @lib.parse_path_and_query("/files/{*path}/")
  inspect(
    segments,
    content=(
      #|[{kind: Literal("files")}, {kind: RemainingPathVariable("path")}]
    ),
  )
  inspect(
    query,
    content=(
      #|[]
    ),
  )
}

///|
test "http_reorder_params: no endpoint returns original order" {
  let vis : Array[(String, @lib.ParamType)] = [
    ("temperature", Simple("Double")),
    ("source", Simple("String")),
  ]
  let result = @lib.http_reorder_params(vis, None)
  inspect(
    result.map(fn(p) { p.0 }),
    content=(
      #|["temperature", "source"]
    ),
  )
}

///|
test "http_reorder_params: header-bound param moves before body param" {
  let vis : Array[(String, @lib.ParamType)] = [
    ("temperature", Simple("Double")),
    ("source", Simple("String")),
  ]
  let endpoint : @lib.HttpEndpointInfo = {
    method: "post",
    path: "/set",
    headers: [("X-Source", "source")],
    auth: None,
    cors: [],
  }
  let result = @lib.http_reorder_params(vis, Some(endpoint))
  inspect(
    result.map(fn(p) { p.0 }),
    content=(
      #|["source", "temperature"]
    ),
  )
}

///|
test "http_reorder_params: query-bound param stays first" {
  let vis : Array[(String, @lib.ParamType)] = [("unit", Simple("String"))]
  let endpoint : @lib.HttpEndpointInfo = {
    method: "get",
    path: "/current?unit={unit}",
    headers: [],
    auth: None,
    cors: [],
  }
  let result = @lib.http_reorder_params(vis, Some(endpoint))
  inspect(
    result.map(fn(p) { p.0 }),
    content=(
      #|["unit"]
    ),
  )
}

///|
test "http_reorder_params: mixed path, header, and body params" {
  let vis : Array[(String, @lib.ParamType)] = [
    ("body_data", Simple("String")),
    ("city", Simple("String")),
    ("extra_body", Simple("Int")),
    ("api_key", Simple("String")),
  ]
  let endpoint : @lib.HttpEndpointInfo = {
    method: "post",
    path: "/{city}/update",
    headers: [("X-Api-Key", "api_key")],
    auth: None,
    cors: [],
  }
  let result = @lib.http_reorder_params(vis, Some(endpoint))
  // HTTP-bound (city from path, api_key from header) first, then body params
  inspect(
    result.map(fn(p) { p.0 }),
    content=(
      #|["city", "api_key", "body_data", "extra_body"]
    ),
  )
}
