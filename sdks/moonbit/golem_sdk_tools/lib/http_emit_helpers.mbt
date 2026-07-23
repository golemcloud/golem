// String-template emitters for the HTTP metadata carried by an agent's
// `@agents.AgentTypeDef` (method `http_endpoint : Array[@common.HttpEndpointDetails]`
// and agent `http_mount : @common.HttpMountDetails?`). These are embedded inside
// the type-annotated `AgentTypeDef` literal, so plain record literals (`{ ... }`)
// infer their `@common.*` struct types from the field positions; only the enum
// constructors (`HttpMethod`, `PathSegment`, `SystemVariable`) are package-qualified.
//
// NOTE (parameter order): the new schema model preserves the user-supplied
// declaration order for method parameters (matching the Rust/TS SDKs). The Golem
// runtime's `build_http_agent_method_parameters` classifies path/query/header vs
// body but keeps declaration order, and auto-injected `Principal` params keep
// their declared slot (excluded from the wire record out-of-band). So generated
// dispatch decodes parameters positionally in declaration order — there is no
// HTTP-specific reordering.

///|
/// Renders a single `@common.PathSegment` constructor expression.
fn path_segment_str(seg : PathSegmentInfo) -> String {
  match seg.kind {
    Literal(s) => "@common.PathSegment::Literal(\"\{escape_string_lit(s)}\")"
    PathVariable(name) =>
      "@common.PathSegment::PathVariable({ variable_name: \"\{escape_string_lit(name)}\" })"
    RemainingPathVariable(name) =>
      "@common.PathSegment::RemainingPathVariable({ variable_name: \"\{escape_string_lit(name)}\" })"
    SystemVariable(kind) => {
      let variant = match kind {
        AgentType => "AGENT_TYPE"
        AgentVersion => "AGENT_VERSION"
      }
      "@common.PathSegment::SystemVariable(@common.SystemVariable::\{variant})"
    }
  }
}

///|
/// Renders an `Array[@common.PathSegment]` literal from parsed path segments.
fn path_segments_str(segments : Array[PathSegmentInfo]) -> String {
  let parts = segments.map(path_segment_str)
  "[" + parts.join(", ") + "]"
}

///|
/// Renders an `@common.HttpMethod` constructor expression.
fn http_method_str(http_method : String) -> String {
  let lower = string_to_lower(http_method)
  let variant = if lower == "get" {
    Some("Get")
  } else if lower == "head" {
    Some("Head")
  } else if lower == "post" {
    Some("Post")
  } else if lower == "put" {
    Some("Put")
  } else if lower == "delete" {
    Some("Delete")
  } else if lower == "connect" {
    Some("Connect")
  } else if lower == "options" {
    Some("Options")
  } else if lower == "trace" {
    Some("Trace")
  } else if lower == "patch" {
    Some("Patch")
  } else {
    None
  }
  match variant {
    Some(v) => "@common.HttpMethod::\{v}"
    None => "@common.HttpMethod::Custom(\"\{escape_string_lit(http_method)}\")"
  }
}

///|
/// Renders an `@common.AuthDetails?` value.
fn auth_details_str(auth : Bool?) -> String {
  match auth {
    None => "None"
    Some(required) => "Some({ required: \{required} })"
  }
}

///|
/// Renders an `@common.CorsOptions` record literal.
fn cors_options_str(patterns : Array[String]) -> String {
  let parts = patterns.map(fn(p) { "\"\{escape_string_lit(p)}\"" })
  "{ allowed_patterns: [" + parts.join(", ") + "] }"
}

///|
/// Renders a method's `http_endpoint` field — an `Array[@common.HttpEndpointDetails]`
/// with zero or one entry. Empty when the method has no `#derive.endpoint`.
fn http_endpoint_str(endpoint : HttpEndpointInfo?) -> String {
  match endpoint {
    None => "[]"
    Some(ep) => {
      let (segments, query_vars) = parse_path_and_query(ep.path) catch {
        e =>
          abort(
            "Failed to parse endpoint path '\{ep.path}': \{@debug.to_string(e)}",
          )
      }
      let path_suffix = path_segments_str(segments)
      let header_parts = ep.headers.map(fn(h) {
        "{ header_name: \"\{escape_string_lit(h.0)}\", variable_name: \"\{escape_string_lit(h.1)}\" }"
      })
      let header_vars = "[" + header_parts.join(", ") + "]"
      let query_parts = query_vars.map(fn(q) {
        "{ query_param_name: \"\{escape_string_lit(q.query_param_name)}\", variable_name: \"\{escape_string_lit(q.variable_name)}\" }"
      })
      let query = "[" + query_parts.join(", ") + "]"
      "[{ http_method: \{http_method_str(ep.method)}, path_suffix: \{path_suffix}, header_vars: \{header_vars}, query_vars: \{query}, auth_details: \{auth_details_str(ep.auth)}, cors_options: \{cors_options_str(ep.cors)} }]"
    }
  }
}

///|
/// Renders an agent's `http_mount` field — an `@common.HttpMountDetails?`.
fn http_mount_str(mount : HttpMountInfo?) -> String {
  match mount {
    None => "None"
    Some(info) => {
      let segments = parse_path_only(info.path) catch {
        e =>
          abort(
            "Failed to parse mount path '\{info.path}': \{@debug.to_string(e)}",
          )
      }
      let path_prefix = path_segments_str(segments)
      let webhook_suffix = match info.webhook_suffix {
        None => "[]"
        Some(suffix) => {
          let s = parse_path_only(suffix) catch {
            e =>
              abort(
                "Failed to parse webhook suffix '\{suffix}': \{@debug.to_string(e)}",
              )
          }
          path_segments_str(s)
        }
      }
      "Some({ path_prefix: \{path_prefix}, auth_details: \{auth_details_str(info.auth)}, phantom_agent: \{info.phantom}, cors_options: \{cors_options_str(info.cors)}, webhook_suffix: \{webhook_suffix} })"
    }
  }
}
