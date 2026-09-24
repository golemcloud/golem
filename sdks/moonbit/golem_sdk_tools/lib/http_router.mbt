///|
fn has_http_annotation(
  attrs : @list.List[@attribute.Attribute],
  name : String,
) -> Bool {
  attrs.any(a => {
    a.raw == "#derive.\{name}" || a.raw.has_prefix("#derive.\{name}(")
  })
}

///|
pub fn AgentInfo::is_http_router(self : AgentInfo) -> Bool {
  self.http_mount is Some({ router: true, .. })
}

///|
fn validate_http_type_annotations(
  attrs : @list.List[@attribute.Attribute],
  router : Bool,
  mount : String?,
) -> Unit raise AgentParseError {
  if router ||
    has_http_annotation(attrs, "static_file") ||
    has_http_annotation(attrs, "expose_files") {
    if mount is None {
      http_fail("M01/M02: HTTP router and file declarations require a mount")
    }
    let mounts = attrs.filter(a => a.raw.has_prefix("#derive.mount(")).length()
    if mounts != 1 {
      http_fail("M01: exactly one HTTP mount is required")
    }
  }
  if router {
    let declarations = attrs
      .filter(a => a.raw == "#derive.http_router")
      .length()
    if declarations != 1 ||
      attrs.any(a => a.raw.has_prefix("#derive.http_router(")) {
      http_fail(
        "M01: use exactly one parameterless #derive.http_router annotation",
      )
    }
  }
}

///|
fn extract_file_mappings(
  attrs : @list.List[@attribute.Attribute],
  name : String,
) -> Array[HttpFileMapping] raise AgentParseError {
  let pairs : Array[(String, String)] = []
  for attr in attrs {
    if attr.raw == "#derive.\{name}" || attr.raw.has_prefix("#derive.\{name}(") {
      match attr.parsed {
        Some(Apply({ qual: Some("derive"), name: actual, }, props)) if actual ==
          name => {
          let values : Array[String] = []
          for prop in props {
            match prop {
              Expr(String(s)) => values.push(s)
              _ =>
                http_fail("M03: #derive.\{name} requires two string literals")
            }
          }
          if values.length() != 2 {
            http_fail("M03: #derive.\{name} requires source and target")
          }
          pairs.push((values[0], values[1]))
        }
        _ => http_fail("M03: invalid #derive.\{name} declaration")
      }
    }
  }
  compile_http_file_mappings(pairs)
}

///|
fn validate_http_router(
  agent : AgentInfo,
  mount : HttpMountInfo,
) -> Unit raise AgentParseError {
  if mount.router {
    if agent.mode != Ephemeral ||
      agent.snapshotting != Disabled ||
      agent.has_snapshottable {
      http_fail(
        "M01: router must be ephemeral with snapshotting disabled: \{agent.name}",
      )
    }
    if agent.constructor_params.any(p => !is_config_type(p.1)) {
      http_fail(
        "M01: router constructor cannot have identity or principal inputs: \{agent.name}",
      )
    }
    if mount.phantom || !mount.filesystem_bindings.is_empty() {
      http_fail(
        "M02: router cannot expose live files or use a phantom mount: \{agent.name}",
      )
    }
    let _ = literal_http_path(mount.path)
    let mut handlers = 0
    for meth in agent.methods {
      if mount.openapi_provider_method == Some(meth.name) {
        if !meth.params.is_empty() || meth.http_endpoint is Some(_) {
          http_fail(
            "M01: OpenAPI provider must be parameterless without an endpoint: \{agent.name}.\{meth.name}",
          )
        }
        // Resolved output schemas, including aliases, are checked by extraction.
        if meth.return_type is None {
          http_fail("M01: OpenAPI provider must return a string")
        }
      } else {
        handlers += 1
        if !(meth.http_endpoint is Some({ http_method: Any, .. })) {
          http_fail(
            "M01: router methods must be annotated HTTP handlers or OpenAPI providers: \{agent.name}.\{meth.name}",
          )
        }
        let inputs = visible_params(meth.params)
        if inputs.length() != 1 ||
          inputs[0].0 != "request" ||
          meth.return_type is None {
          http_fail(
            "H01: handler requires request and a response: \{agent.name}.\{meth.name}",
          )
        }
      }
    }
    if handlers > 1 {
      http_fail("M01: at most one handler is allowed: \{agent.name}")
    }
  } else {
    if !mount.static_bindings.is_empty() ||
      mount.openapi_provider_method is Some(_) {
      http_fail(
        "M02: static files and OpenAPI providers require an HTTP router: \{agent.name}",
      )
    }
    if !mount.filesystem_bindings.is_empty() {
      if agent.mode != Durable || mount.phantom {
        http_fail(
          "M02: exposed files require a durable non-phantom agent: \{agent.name}",
        )
      }
      if !mount.path.has_prefix("/") {
        http_fail("M02: live file mount must be absolute")
      }
      let segments = parse_path_only(mount.path) catch {
        _ => {
          http_fail("M02: invalid live file mount")
          []
        }
      }
      let captures = collect_path_variable_names(segments)
      for param in agent.constructor_params {
        if is_config_type(param.1) {
          continue
        }
        if is_principal_type(param.1) ||
          captures.filter(c => c == param.0).length() != 1 {
          http_fail(
            "M02: file owner identity must be bound exactly once: \{agent.name}.\{param.0}",
          )
        }
      }
      for part in mount.path.split("/").drop(1) {
        if mount.path == "/" {
          continue
        }
        if part.has_prefix("{") && part.has_suffix("}") {
          continue
        }
        let _ = decode_http_segment(part.to_owned())
      }
    }
  }
}
