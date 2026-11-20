///|
pub(all) enum ToolHttpInfo {
  Prefix(String)
  Endpoint(HttpEndpointInfo, String?, Array[(String, String)])
} derive(Debug, Eq)

///|
fn parse_tool_mount(
  attrs : @list.List[@attribute.Attribute],
) -> String? raise ToolParseError {
  let mut mount : String? = None
  for attr in attrs {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "mount", }, props)) =>
        match props.to_array() {
          [Expr(String(path))] if mount is None => mount = Some(path)
          _ =>
            tool_fail("A tool mount requires one path and no admission options")
        }
      _ if attr.raw.has_prefix("#derive.mount_") =>
        tool_fail("Tool HTTP admission and CORS belong to the export")
      _ => ()
    }
  }
  mount
}

///|
fn parse_tool_http(
  attrs : @list.List[@attribute.Attribute],
) -> ToolHttpInfo? raise ToolParseError {
  let mut prefix : String? = None
  let mut route_id : String? = None
  let response_headers : Array[(String, String)] = []
  for attr in attrs {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "endpoint", }, props)) =>
        for prop in props {
          match prop {
            Labeled("prefix", String(value)) => {
              if prefix is Some(_) {
                tool_fail("Duplicate HTTP prefix")
              }
              prefix = Some(value)
            }
            Labeled("route_id", String(value)) => {
              if route_id is Some(_) {
                tool_fail("Duplicate HTTP route ID")
              }
              route_id = Some(value)
            }
            Labeled("mode", String(_)) => ()
            Labeled(name, String(_)) if http_methods.contains(name) => ()
            _ => tool_fail("Unknown tool endpoint option")
          }
        }
      Some(
        Apply(
          { qual: Some("derive"), name: "endpoint_response_header", },
          props
        )
      ) => {
        let values = props.to_array()
        match values {
          [Expr(String(name)), Expr(String(value))] =>
            response_headers.push((name, value))
          _ =>
            tool_fail(
              "endpoint_response_header requires a header name and value",
            )
        }
      }
      Some(
        Apply(
          { qual: Some("derive"), name: "endpoint_auth" | "endpoint_cors", },
          _
        )
      ) => tool_fail("Tool HTTP admission and CORS belong to the export")
      _ => ()
    }
  }
  match prefix {
    Some(path) => {
      if attrs.filter(a => a.raw.has_prefix("#derive.endpoint")).length() != 1 ||
        attrs.any(a => a.raw.has_prefix("#derive.durable_stream")) ||
        route_id is Some(_) ||
        !response_headers.is_empty() {
        tool_fail("A tool prefix cannot contain endpoint options")
      }
      for attr in attrs {
        match attr.parsed {
          Some(Apply({ qual: Some("derive"), name: "endpoint", }, props)) if props.length() !=
            1 => tool_fail("A tool prefix cannot contain endpoint options")
          _ => ()
        }
      }
      Some(Prefix(path))
    }
    None => {
      let endpoint = build_endpoint_info(attrs) catch {
        error => raise ToolParseError(repr(error))
      }
      if endpoint is None &&
        (route_id is Some(_) || !response_headers.is_empty()) {
        tool_fail("HTTP endpoint options require an endpoint")
      }
      endpoint.map(ep => Endpoint(ep, route_id, response_headers))
    }
  }
}

///|
fn tool_path_str(path : String, prefix~ : Bool) -> String {
  let (segments, query) = parse_path_and_query(path) catch {
    error => abort(repr(error))
  }
  if prefix && !query.is_empty() {
    abort("Tool mounts and prefixes cannot contain query bindings")
  }
  let parts = segments.map(segment => {
    match segment.kind {
      Literal(value) =>
        "@coreHttp.PathSegment::Literal(\"\{escape_string_lit(value)}\")"
      PathVariable(name) =>
        "@coreHttp.PathSegment::PathVariable({ variable_name: \"\{escape_string_lit(name)}\" })"
      RemainingPathVariable(name) =>
        "@coreHttp.PathSegment::RemainingPathVariable({ variable_name: \"\{escape_string_lit(name)}\" })"
      SystemVariable(_) =>
        abort("Tool HTTP paths cannot contain system variables")
    }
  })
  "[" + parts.join(", ") + "]"
}

///|
fn tool_mount_str(path : String?) -> String {
  path
  .map(p => "Some({ path_prefix: \{tool_path_str(p, prefix=true)} })")
  .unwrap_or("None")
}

///|
fn tool_http_str(http : ToolHttpInfo?) -> String {
  match http {
    None => "None"
    Some(Prefix(path)) =>
      "Some(@toolCommon.CommandHttp::Prefix({ path_suffix: \{tool_path_str(path, prefix=true)} }))"
    Some(Endpoint(ep, route_id, headers)) => {
      let (_, query) = parse_path_and_query(ep.path) catch {
        error => abort(repr(error))
      }
      let query = query
        .map(q => {
          "{ query_param_name: \"\{escape_string_lit(q.query_param_name)}\", variable_name: \"\{escape_string_lit(q.variable_name)}\" }"
        })
        .join(", ")
      let input_headers = ep.headers
        .map(h => {
          "{ header_name: \"\{escape_string_lit(h.0)}\", variable_name: \"\{escape_string_lit(h.1)}\" }"
        })
        .join(", ")
      let response_headers = headers
        .map(h => {
          "{ name: \"\{escape_string_lit(h.0)}\", value: \"\{escape_string_lit(h.1)}\" }"
        })
        .join(", ")
      let method = match ep.http_method {
        Concrete(name) => neutral_http_method_str(name)
        Any => abort("Tools require a concrete HTTP method")
      }
      let mode = ep.mode
        .map(value => {
          let variant = match value {
            "plain" => "PLAIN"
            "durableStreams" => "DURABLE_STREAMS"
            "sse" => "SSE"
            "webSocket" => "WEB_SOCKET"
            _ => abort("Invalid HTTP mode")
          }
          "Some(@coreHttp.HttpRouteMode::\{variant})"
        })
        .unwrap_or("None")
      "Some(@toolCommon.CommandHttp::Endpoint({ route_id: \{opt_string_lit(route_id)}, http_method: \{method}, path_suffix: \{tool_path_str(ep.path, prefix=false)}, query_vars: [\{query}], header_vars: [\{input_headers}], response_headers: [\{response_headers}], mode: \{mode}, durable_streams: \{durable_streams_str(ep.durable_streams)} }))"
    }
  }
}
