///|
pub suberror ToolMiddlewareEmitError {
  ToolMiddlewareEmitError(String)
} derive(Debug, Eq)

///|
fn tool_middleware_emit_fail(
  message : String,
) -> Unit raise ToolMiddlewareEmitError {
  raise ToolMiddlewareEmitError(message)
}

///|
fn universal_invoker_name(middleware : ToolMiddlewareInfo) -> String {
  "__golem_universal_tool_middleware_invoke_" + middleware.source_name
}

///|
fn emit_universal_tool_middleware_invoker(
  buf : StringBuilder,
  middleware : ToolMiddlewareInfo,
  universal : UniversalToolMiddlewareInfo,
) -> Unit {
  buf.write_string(
    "///|\nasync fn " +
    universal_invoker_name(middleware) +
    "(\n  invocation : @toolMiddleware.RawToolInvocation,\n  underlying : @toolMiddleware.UnderlyingTool,\n) -> Result[\n  @toolMiddleware.RawInvocationResult,\n  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],\n] {\n  " +
    universal.handler_name +
    "(invocation, underlying)\n}\n\n",
  )
}

///|
fn emit_universal_tool_middleware_registration(
  buf : StringBuilder,
  middleware : ToolMiddlewareInfo,
) -> Unit {
  buf.write_string(
    "    @toolMiddleware.register_tool_middleware(\n      {\n        name: \"" +
    escape_string_lit(middleware.name) +
    "\",\n        aliases: " +
    emit_strings(middleware.aliases) +
    ",\n        doc: " +
    emit_doc(middleware.description, []) +
    ",\n        scope: @toolMiddleware.ToolMiddlewareScope::Universal,\n      },\n      " +
    universal_invoker_name(middleware) +
    ",\n    )\n",
  )
}

///|
fn resolve_middleware_subtree_tool(
  tools : Array[ToolInfo],
  target : String,
) -> ToolInfo raise ToolMiddlewareEmitError {
  resolve_subtree_tool(tools, target) catch {
    ToolEmitError(message) => {
      tool_middleware_emit_fail(message)
      abort("")
    }
  }
}

///|
fn mark_middleware_descriptor_tool(
  tools : Array[ToolInfo],
  tool : ToolInfo,
  selected : Map[String, Unit],
) -> Unit raise ToolMiddlewareEmitError {
  if selected.contains(tool.source_name) {
    return
  }
  selected[tool.source_name] = ()
  for command in tool.commands {
    if command.subtree is Some(target) {
      mark_middleware_descriptor_tool(
        tools,
        resolve_middleware_subtree_tool(tools, target),
        selected,
      )
    }
  }
}

///|
fn middleware_descriptor_tools(
  middlewares : Array[ToolMiddlewareInfo],
  tools : Array[ToolInfo],
) -> Array[ToolInfo] raise ToolMiddlewareEmitError {
  let selected : Map[String, Unit] = {}
  for middleware in middlewares {
    match middleware.scope {
      Monomorphic(monomorphic) => {
        mark_middleware_descriptor_tool(tools, monomorphic.presented, selected)
        mark_middleware_descriptor_tool(tools, monomorphic.expected, selected)
      }
      Universal(_) => ()
    }
  }
  tools.filter(tool => selected.contains(tool.source_name))
}

///|
fn middleware_value_type(surface : ToolMiddlewareCommandInfo) -> ParamType? {
  if surface.has_stdout {
    match surface.success_type {
      Tuple([value, _]) => Some(value)
      _ => None
    }
  } else if surface.success_type == Simple("Unit") {
    None
  } else {
    Some(surface.success_type)
  }
}

///|
fn middleware_is_infallible(surface : ToolMiddlewareCommandInfo) -> Bool {
  surface.error_type == Simple("@tool.NoToolError")
}

///|
fn emit_middleware_protocol_helpers(buf : StringBuilder) -> Unit {
  buf.write_string(
    (
      #|///|
      #|fn[T : @schema.IntoSchema + @schema.FromSchema, E] __golem_middleware_decode_canonical_field(
      #|  fields : Array[@tool.CanonicalInputValue],
      #|  name : String,
      #|  context : String,
      #|) -> Result[T, @toolMiddleware.ToolInvokeError[E]] {
      #|  try @tool.decode_canonical_field(fields, name, context) catch {
      #|    @toolCommon.ToolError::InvalidToolName(name) => Err(@toolMiddleware.ToolInvokeError::InvalidToolName(name))
      #|    @toolCommon.ToolError::InvalidCommandPath(path) => Err(@toolMiddleware.ToolInvokeError::InvalidCommandPath(path))
      #|    @toolCommon.ToolError::InvalidInput(message) => Err(@toolMiddleware.ToolInvokeError::InvalidInput(message))
      #|    @toolCommon.ToolError::ConstraintViolation(message) => Err(@toolMiddleware.ToolInvokeError::ConstraintViolation(message))
      #|    @toolCommon.ToolError::InvalidResult(message) => Err(@toolMiddleware.ToolInvokeError::InvalidResult(message))
      #|    @toolCommon.ToolError::CustomError(value) => {
      #|      @model.drop_wit_capabilities(value.value)
      #|      Err(@toolMiddleware.ToolInvokeError::InvalidResult("canonical field decoding returned a custom tool error"))
      #|    }
      #|  } noraise {
      #|    value => Ok(value)
      #|  }
      #|}
      #|
      #|
    ),
  )
}

///|
fn emit_expected_underlying_method(
  buf : StringBuilder,
  expected : ToolInfo,
  surface : ToolMiddlewareCommandInfo,
) -> Unit {
  let wrapper = expected.source_name + "Underlying"
  let params = surface.params
    .map(pair => pair.0 + " : " + emit_type(pair.1))
    .join(", ")
  buf.write_string(
    "///|\npub async fn " +
    wrapper +
    "::" +
    surface.handler_name +
    "(self : " +
    wrapper +
    (if params.is_empty() { "" } else { ", " + params }) +
    ") -> Result[" +
    emit_type(surface.success_type) +
    ", @toolMiddleware.ToolInvokeError[" +
    emit_type(surface.error_type) +
    "]] {\n  let __golem_command_path = " +
    emit_strings(surface.command_path) +
    "\n  let __golem_def = " +
    descriptor_name(expected, "middleware") +
    "()\n  let __golem_index = match __golem_def.command_index_by_path(__golem_command_path) {\n    Some(__golem_index) => __golem_index\n    None => return Err(@toolMiddleware.ToolInvokeError::InvalidCommandPath(__golem_command_path))\n  }\n  let __golem_model = __golem_def.canonical_input_model(__golem_index) catch { __golem_error => return Err(@toolMiddleware.ToolInvokeError::InvalidInput(repr(__golem_error))) }\n  let __golem_values : Array[(String, @model.SchemaValue)] = []\n",
  )
  let mut stdin = "None"
  for index, source in surface.param_sources {
    match source {
      CanonicalInput(canonical) =>
        buf.write_string(
          "  __golem_values.push((\"" +
          escape_string_lit(canonical) +
          "\", @schema.to_value_as(" +
          surface.params[index].0 +
          ")))\n",
        )
      Stdin => stdin = "Some(" + surface.params[index].0 + ")"
      Principal => abort("expected middleware surface contains principal")
    }
  }
  let decode_custom_error = if middleware_is_infallible(surface) {
    "fn(_) { Err(\"underlying returned a custom error for an infallible command\") }"
  } else {
    "fn(__golem_error) { " +
    emit_type(surface.error_type) +
    "::from_error_payload_value(__golem_error) }"
  }
  let decode_result = if surface.has_stdout {
    if middleware_value_type(surface) is Some(_) {
      "decode_result_with_stdout"
    } else {
      "decode_result_stdout_only"
    }
  } else if middleware_value_type(surface) is Some(_) {
    "decode_result_value"
  } else {
    "decode_result_empty"
  }
  buf.write_string(
    "  let __golem_input = @tool.build_canonical_input(__golem_model, __golem_values) catch { __golem_error => return Err(@toolMiddleware.ToolInvokeError::InvalidInput(repr(__golem_error))) }\n  let __golem_result = match self.underlying.invoke_with(\n    __golem_command_path,\n    __golem_input,\n    " +
    stdin +
    ",\n    " +
    decode_custom_error +
    ",\n  ) {\n    Ok(__golem_result) => __golem_result\n    Err(__golem_error) => return Err(__golem_error)\n  }\n  @toolMiddleware." +
    decode_result +
    "(__golem_result)\n}\n\n",
  )
}

///|
fn emit_expected_underlying(
  buf : StringBuilder,
  expected : ToolInfo,
  surfaces : Array[ToolMiddlewareCommandInfo],
) -> Unit {
  let wrapper = expected.source_name + "Underlying"
  buf.write_string(
    "///|\npub struct " +
    wrapper +
    " { priv underlying : @toolMiddleware.UnderlyingTool }\n\n",
  )
  for surface in surfaces {
    emit_expected_underlying_method(buf, expected, surface)
  }
}

///|
fn emit_presented_parameter_bindings(
  buf : StringBuilder,
  surface : ToolMiddlewareCommandInfo,
  context : String,
) -> Unit {
  let has_stdin = surface.param_sources.any(source => source is Stdin)
  if has_stdin {
    buf.write_string("    let __golem_stdin = __golem_invocation.stdin()\n")
  } else {
    buf.write_string(
      "    if __golem_invocation.stdin() is Some(_) {\n      return Err(@toolMiddleware.ToolInvokeError::InvalidInput(\"tool invocation contained an unexpected stdin stream\"))\n    }\n",
    )
  }
  for index, source in surface.param_sources {
    let (name, type_) = surface.params[index]
    match source {
      CanonicalInput(canonical) =>
        buf.write_string(
          "    let " +
          name +
          " : " +
          emit_type(type_) +
          " = match __golem_middleware_decode_canonical_field(__golem_decoded, \"" +
          escape_string_lit(canonical) +
          "\", \"" +
          escape_string_lit(context) +
          "\") {\n      Ok(__golem_value) => __golem_value\n      Err(__golem_error) => return Err(__golem_error)\n    }\n",
        )
      Stdin =>
        buf.write_string(
          "    let " +
          name +
          " = match __golem_stdin {\n      Some(__golem_stdin) => __golem_stdin\n      None => return Err(@toolMiddleware.ToolInvokeError::InvalidInput(\"tool invocation did not contain declared stdin stream\"))\n    }\n",
        )
      Principal =>
        buf.write_string(
          "    let " + name + " = __golem_invocation.principal()\n",
        )
    }
  }
}

///|
fn emit_presented_success(
  surface : ToolMiddlewareCommandInfo,
  value : String,
) -> String {
  if surface.has_stdout {
    match middleware_value_type(surface) {
      Some(_) =>
        "let (__golem_value, __golem_stdout) = " +
        value +
        "\n        return __golem_invocation.typed_result_value_with_stdout(__golem_value, __golem_stdout)"
      None => "return __golem_invocation.typed_result_stdout(" + value + ")"
    }
  } else {
    match middleware_value_type(surface) {
      Some(_) => "return __golem_invocation.typed_result_value(" + value + ")"
      None =>
        "ignore(" +
        value +
        ")\n        return __golem_invocation.typed_result_empty()"
    }
  }
}

///|
fn emit_presented_custom_error_encoder(
  surface : ToolMiddlewareCommandInfo,
) -> String {
  if middleware_is_infallible(surface) {
    "fn(_) { Err(\"infallible middleware handler returned a custom tool error\") }"
  } else {
    "fn(__golem_error) { " +
    emit_type(surface.error_type) +
    "::to_error_payload_value(__golem_error) }"
  }
}

///|
fn monomorphic_invoker_name(middleware : ToolMiddlewareInfo) -> String {
  "__golem_monomorphic_tool_middleware_invoke_" + middleware.source_name
}

///|
fn emit_monomorphic_tool_middleware_invoker(
  buf : StringBuilder,
  middleware : ToolMiddlewareInfo,
  monomorphic : MonomorphicToolMiddlewareInfo,
) -> Unit {
  buf.write_string(
    "///|\nasync fn " +
    monomorphic_invoker_name(middleware) +
    "(\n  __golem_invocation : @toolMiddleware.RawToolInvocation,\n  __golem_underlying : @toolMiddleware.UnderlyingTool,\n) -> Result[\n  @toolMiddleware.RawInvocationResult,\n  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],\n] {\n  let __golem_command_path = __golem_invocation.command_path()\n  let __golem_def = " +
    descriptor_name(monomorphic.presented, "middleware") +
    "()\n  let __golem_index = match __golem_def.command_index_by_path(__golem_command_path) {\n    Some(__golem_index) => __golem_index\n    None => return Err(@toolMiddleware.ToolInvokeError::InvalidCommandPath(__golem_command_path))\n  }\n  let __golem_decoded = match __golem_invocation.decode_input(__golem_def, __golem_index, \"tool middleware " +
    escape_string_lit(middleware.name) +
    " invocation\") {\n    Ok(__golem_decoded) => __golem_decoded\n    Err(__golem_error) => return Err(__golem_error)\n  }\n",
  )
  for surface in monomorphic.handlers {
    let context = "tool middleware " +
      middleware.name +
      " command " +
      surface.command_path.join("/")
    buf.write_string(
      "  if __golem_def.command_index_by_path(" +
      emit_strings(surface.command_path) +
      ") == Some(__golem_index) {\n",
    )
    emit_presented_parameter_bindings(buf, surface, context)
    let args = surface.params.map(pair => pair.0)
    let call_args = if args.is_empty() {
      "__golem_expected"
    } else {
      "__golem_expected, " + args.join(", ")
    }
    buf.write_string(
      "    let __golem_expected = " +
      monomorphic.expected.source_name +
      "Underlying::{ underlying: __golem_underlying }\n    match " +
      middleware.source_name +
      "::" +
      surface.handler_name +
      "(" +
      call_args +
      ") {\n      Ok(__golem_result) => {\n        " +
      emit_presented_success(surface, "__golem_result") +
      "\n      }\n      Err(__golem_error) => {\n        return Err(__golem_invocation.encode_handler_error(\n          __golem_error,\n          " +
      emit_presented_custom_error_encoder(surface) +
      ",\n        ))\n      }\n    }\n  }\n",
    )
  }
  buf.write_string(
    "  Err(@toolMiddleware.ToolInvokeError::InvalidCommandPath(__golem_command_path))\n}\n\n",
  )
}

///|
fn emit_monomorphic_tool_middleware_registration(
  buf : StringBuilder,
  middleware : ToolMiddlewareInfo,
  monomorphic : MonomorphicToolMiddlewareInfo,
) -> Unit {
  buf.write_string(
    "    @toolMiddleware.register_tool_middleware(\n      {\n        name: \"" +
    escape_string_lit(middleware.name) +
    "\",\n        aliases: " +
    emit_strings(middleware.aliases) +
    ",\n        doc: " +
    emit_doc(middleware.description, []) +
    ",\n        scope: @toolMiddleware.ToolMiddlewareScope::Monomorphic({\n          presented: " +
    descriptor_name(monomorphic.presented, "middleware") +
    "().to_tool(),\n          expected: " +
    descriptor_name(monomorphic.expected, "middleware") +
    "().to_tool(),\n        }),\n      },\n      " +
    monomorphic_invoker_name(middleware) +
    ",\n    )\n",
  )
}

///|
pub fn generate_tool_middleware_registration(
  middlewares : Array[ToolMiddlewareInfo],
  tools : Array[ToolInfo],
  role : ComponentRole,
) -> @list.List[@syntax.Impl] raise ToolMiddlewareEmitError {
  if !role.allows_tool_middleware_registration() {
    tool_middleware_emit_fail(
      "component role '" + role.name() + "' does not export tool middleware",
    )
  }
  let buf = StringBuilder::new()
  let descriptor_tools = middleware_descriptor_tools(middlewares, tools)
  emit_tool_descriptor_support(
    buf,
    descriptor_tools,
    "middleware",
    false,
    role is ToolMiddleware,
  ) catch {
    ToolEmitError(message) => tool_middleware_emit_fail(message)
  }
  if middlewares.any(middleware => middleware.scope is Monomorphic(_)) {
    emit_middleware_protocol_helpers(buf)
    let emitted_wrappers : Map[String, Unit] = {}
    for middleware in middlewares {
      if middleware.scope is Monomorphic(monomorphic) &&
        !emitted_wrappers.contains(monomorphic.expected.source_name) {
        emitted_wrappers[monomorphic.expected.source_name] = ()
        emit_expected_underlying(
          buf,
          monomorphic.expected,
          monomorphic.underlying_methods,
        )
      }
    }
  }
  for middleware in middlewares {
    match middleware.scope {
      Universal(universal) =>
        emit_universal_tool_middleware_invoker(buf, middleware, universal)
      Monomorphic(monomorphic) =>
        emit_monomorphic_tool_middleware_invoker(buf, middleware, monomorphic)
    }
  }
  if middlewares.length() > 0 {
    buf.write_string(
      "///|\nfn __golem_register_tool_middlewares() -> Unit raise {\n",
    )
    for middleware in middlewares {
      match middleware.scope {
        Universal(_) =>
          emit_universal_tool_middleware_registration(buf, middleware)
        Monomorphic(monomorphic) =>
          emit_monomorphic_tool_middleware_registration(
            buf, middleware, monomorphic,
          )
      }
    }
    buf.write_string(
      "}\n\n///|\nfn init {\n  try __golem_register_tool_middlewares() catch { error => abort(\"failed to register generated tool middleware: \" + repr(error)) }\n}\n",
    )
  }
  parse_generated_impls(buf.to_string(), "tool middleware registration")
}
