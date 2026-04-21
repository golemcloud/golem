///|
/// Converts a `PathSegmentInfo` to an AST expression for `@common.PathSegment`.
fn emit_path_segment_expr(seg : PathSegmentInfo) -> @syntax.Expr {
  match seg.kind {
    Literal(s) =>
      make_apply(make_qualified_constr("common", "PathSegment", "Literal"), [
        make_string_expr(s),
      ])
    PathVariable(name) =>
      make_apply(
        make_qualified_constr("common", "PathSegment", "PathVariable"),
        [make_record([make_field("variable_name", make_string_expr(name))])],
      )
    RemainingPathVariable(name) =>
      make_apply(
        make_qualified_constr("common", "PathSegment", "RemainingPathVariable"),
        [make_record([make_field("variable_name", make_string_expr(name))])],
      )
    SystemVariable(kind) => {
      let variant = match kind {
        AgentType => "AGENT_TYPE"
        AgentVersion => "AGENT_VERSION"
      }
      make_apply(
        make_qualified_constr("common", "PathSegment", "SystemVariable"),
        [make_qualified_constr("common", "SystemVariable", variant)],
      )
    }
  }
}

///|
/// Builds an AST expression for `AuthDetails?`.
fn emit_auth_details_expr(auth : Bool?) -> @syntax.Expr {
  match auth {
    None => make_constr_no_args("None")
    Some(required) =>
      make_apply(make_constr_no_args("Some"), [
        make_record([make_field("required", make_bool_expr(required))]),
      ])
  }
}

///|
/// Builds an AST expression for `@common.CorsOptions`.
fn emit_cors_expr(patterns : Array[String]) -> @syntax.Expr {
  make_record([
    make_field("allowed_patterns", make_array(patterns.map(make_string_expr))),
  ])
}

///|
/// Builds an AST expression for `@common.HttpMethod`.
fn emit_http_method_expr(http_method : String) -> @syntax.Expr {
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
    Some(v) => make_qualified_constr("common", "HttpMethod", v)
    None =>
      make_apply(make_qualified_constr("common", "HttpMethod", "Custom"), [
        make_string_expr(http_method),
      ])
  }
}

///|
/// Parses a path string with `parse_path_only` and returns an array expression of path segment expressions.
fn emit_path_segments_array(path : String) -> @syntax.Expr {
  let segments = parse_path_only(path) catch {
    e => abort("Failed to parse path '\{path}': \{e}")
  }
  make_array(segments.map(emit_path_segment_expr))
}

///|
/// Builds the `HttpMountDetails` expression from an `HttpMountInfo?`.
fn emit_http_mount_expr(mount : HttpMountInfo?) -> @syntax.Expr {
  match mount {
    None => make_constr_no_args("None")
    Some(info) => {
      let webhook_suffix_expr = match info.webhook_suffix {
        None => make_array([])
        Some(suffix) => emit_path_segments_array(suffix)
      }
      make_apply(make_constr_no_args("Some"), [
        make_record([
          make_field("path_prefix", emit_path_segments_array(info.path)),
          make_field("auth_details", emit_auth_details_expr(info.auth)),
          make_field("phantom_agent", make_bool_expr(info.phantom)),
          make_field("cors_options", emit_cors_expr(info.cors)),
          make_field("webhook_suffix", webhook_suffix_expr),
        ]),
      ])
    }
  }
}

///|
/// Collects the set of HTTP-bound parameter names from an endpoint definition.
/// A parameter is HTTP-bound if it appears as a path variable, query variable,
/// or header variable in the endpoint.
fn collect_http_bound_names(endpoint : HttpEndpointInfo) -> Map[String, Unit] {
  let bound : Map[String, Unit] = {}
  // Parse path and query together
  let (segments, query_vars) = parse_path_and_query(endpoint.path) catch {
    _ => ([], [])
  }
  // Path variables
  for seg in segments {
    match seg.kind {
      PathVariable(name) | RemainingPathVariable(name) => bound.set(name, ())
      _ => ()
    }
  }
  // Query variables
  for qv in query_vars {
    bound.set(qv.variable_name, ())
  }
  // Header variables
  for h in endpoint.headers {
    bound.set(h.1, ())
  }
  bound
}

///|
/// Reorders visible parameters so that HTTP-bound parameters (path, query, header)
/// come first (in their original relative order), followed by body parameters
/// (also in original relative order). This matches the Golem runtime's
/// `build_http_agent_method_parameters` which processes bound params in pass 1
/// then body params in pass 2.
///
/// For methods without HTTP endpoints, returns the original order unchanged.
pub fn http_reorder_params(
  vis : Array[(String, ParamType)],
  endpoint : HttpEndpointInfo?,
) -> Array[(String, ParamType)] {
  match endpoint {
    None => vis
    Some(ep) => {
      let bound = collect_http_bound_names(ep)
      let http_params : Array[(String, ParamType)] = []
      let body_params : Array[(String, ParamType)] = []
      for p in vis {
        if bound.contains(p.0) {
          http_params.push(p)
        } else {
          body_params.push(p)
        }
      }
      http_params.append(body_params)
      http_params
    }
  }
}

///|
/// Builds the `http_endpoint` array expression from an `HttpEndpointInfo?`.
fn emit_http_endpoint_array(endpoint : HttpEndpointInfo?) -> @syntax.Expr {
  match endpoint {
    None => make_array([])
    Some(ep) => {
      let (segments, query_vars) = parse_path_and_query(ep.path) catch {
        e => abort("Failed to parse endpoint path '\{ep.path}': \{e}")
      }
      let header_exprs = ep.headers.map(fn(h) {
        make_record([
          make_field("header_name", make_string_expr(h.0)),
          make_field("variable_name", make_string_expr(h.1)),
        ])
      })
      let query_exprs = query_vars.map(fn(q) {
        make_record([
          make_field("query_param_name", make_string_expr(q.query_param_name)),
          make_field("variable_name", make_string_expr(q.variable_name)),
        ])
      })
      make_array([
        make_record([
          make_field("http_method", emit_http_method_expr(ep.method)),
          make_field(
            "path_suffix",
            make_array(segments.map(emit_path_segment_expr)),
          ),
          make_field("header_vars", make_array(header_exprs)),
          make_field("query_vars", make_array(query_exprs)),
          make_field("auth_details", emit_auth_details_expr(ep.auth)),
          make_field("cors_options", emit_cors_expr(ep.cors)),
        ]),
      ])
    }
  }
}
