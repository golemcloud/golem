///|
fn string_to_lower(s : String) -> String {
  let sb = StringBuilder::new()
  for ch in s {
    sb.write_char(ch.to_ascii_lowercase())
  }
  sb.to_string()
}

///|
fn collect_path_variable_names(
  segments : Array[PathSegmentInfo],
) -> Array[String] {
  let names : Array[String] = []
  for seg in segments {
    match seg.kind {
      PathVariable(name) => names.push(name)
      _ => ()
    }
  }
  names
}

///|
/// Validates all HTTP route annotations across all agents.
///
/// Checks mount paths, endpoint paths, variable bindings, and parameter
/// type constraints. Should be called after all agents have been parsed.
pub fn validate_http_routes(
  agents : Array[AgentInfo],
) -> Unit raise AgentParseError {
  for agent in agents {
    let mount_segments = match agent.http_mount {
      Some(mount) => {
        let segments = parse_path_only(mount.path) catch {
          HttpPathParseError(msg) =>
            raise AgentParseError(
              "Invalid mount path for agent '\{agent.name}': \{msg}",
            )
        }
        // Rule 1: Mount path cannot contain remaining-path-variable
        for seg in segments {
          match seg.kind {
            RemainingPathVariable(vname) =>
              raise AgentParseError(
                "Mount path for agent '\{agent.name}' cannot contain catch-all variable '\{vname}'",
              )
            _ => ()
          }
        }
        let mount_var_names = collect_path_variable_names(segments)
        // Rule 2: UnstructuredBinary constructor params cannot be mount path variables
        for v in mount_var_names {
          let param = agent.constructor_params
            .iter()
            .find_first(fn(p) { p.0 == v })
          match param {
            Some((_, pt)) =>
              if is_unstructured_binary_type(pt) {
                raise AgentParseError(
                  "Constructor param '\{v}' of agent '\{agent.name}' is UnstructuredBinary and cannot be a mount path variable",
                )
              }
            None => ()
          }
        }
        // Rule 8: Principal params cannot be mount path variables
        for v in mount_var_names {
          let param = agent.constructor_params
            .iter()
            .find_first(fn(p) { p.0 == v })
          match param {
            Some((_, pt)) =>
              if is_principal_type(pt) {
                raise AgentParseError(
                  "Mount path variable '\{v}' of agent '\{agent.name}' maps to a Principal param",
                )
              }
            None => ()
          }
        }
        // Rule 3: All mount path variables must exist in constructor params
        for v in mount_var_names {
          let exists = agent.constructor_params
            .iter()
            .find_first(fn(p) { p.0 == v })
          if exists is None {
            raise AgentParseError(
              "Mount path variable '\{v}' not found in constructor params of agent '\{agent.name}'",
            )
          }
        }
        // Rule 4: All non-Principal constructor params must appear as mount path variables
        let vis_params = visible_params(agent.constructor_params)
        for p in vis_params {
          let (pname, _) = p
          let bound = mount_var_names.iter().find_first(fn(v) { v == pname })
          if bound is None {
            raise AgentParseError(
              "Constructor param '\{pname}' of agent '\{agent.name}' is not bound by any mount path variable",
            )
          }
        }
        Some(segments)
      }
      None => None
    }
    for meth in agent.methods {
      // Rule 5: Methods with endpoint require agent mount
      match meth.http_endpoint {
        Some(endpoint) => {
          if mount_segments is None {
            raise AgentParseError(
              "Method '\{meth.name}' has #derive.endpoint but agent '\{agent.name}' has no #derive.mount",
            )
          }
          // Rule 6, 7, 9, 10: Validate endpoint variables
          let (path_segs, query_vars) = parse_path_and_query(endpoint.path) catch {
            HttpPathParseError(msg) =>
              raise AgentParseError(
                "Invalid endpoint path for method '\{meth.name}' on agent '\{agent.name}': \{msg}",
              )
          }
          let all_var_names : Array[String] = []
          // Path variables
          for seg in path_segs {
            match seg.kind {
              PathVariable(name) => all_var_names.push(name)
              RemainingPathVariable(name) => all_var_names.push(name)
              _ => ()
            }
          }
          // Query variables
          for qv in query_vars {
            all_var_names.push(qv.variable_name)
          }
          // Header variables
          for header in endpoint.headers {
            let (_, var_name) = header
            all_var_names.push(var_name)
          }
          // Rule 10: Duplicate header names (case-insensitive)
          let seen_headers : Array[String] = []
          for header in endpoint.headers {
            let (hname, _) = header
            let lower = string_to_lower(hname)
            if seen_headers.iter().find_first(fn(h) { h == lower }) is Some(_) {
              raise AgentParseError(
                "Duplicate header name '\{hname}' in endpoint of method '\{meth.name}' on agent '\{agent.name}'",
              )
            }
            seen_headers.push(lower)
          }
          // Rule 9: No duplicate binding of same method param
          let seen_vars : Array[String] = []
          for v in all_var_names {
            if seen_vars.iter().find_first(fn(s) { s == v }) is Some(_) {
              raise AgentParseError(
                "Parameter '\{v}' of method '\{meth.name}' on agent '\{agent.name}' is bound by multiple endpoint variables",
              )
            }
            seen_vars.push(v)
          }
          // Rule 6: All endpoint variables must exist in method params
          for v in all_var_names {
            let exists = meth.params.iter().find_first(fn(p) { p.0 == v })
            if exists is None {
              raise AgentParseError(
                "Endpoint variable '\{v}' not found in params of method '\{meth.name}' on agent '\{agent.name}'",
              )
            }
          }
          // Rule 7: Endpoint variables cannot bind to UnstructuredBinary or Principal params
          for v in all_var_names {
            let param = meth.params.iter().find_first(fn(p) { p.0 == v })
            match param {
              Some((_, pt)) => {
                if is_unstructured_binary_type(pt) {
                  raise AgentParseError(
                    "Endpoint variable '\{v}' in method '\{meth.name}' on agent '\{agent.name}' binds to a UnstructuredBinary param which is not HTTP-safe",
                  )
                }
                if is_principal_type(pt) {
                  raise AgentParseError(
                    "Endpoint variable '\{v}' in method '\{meth.name}' on agent '\{agent.name}' binds to a Principal param which is not HTTP-safe",
                  )
                }
              }
              None => ()
            }
          }
        }
        None => ()
      }
    }
  }
}
