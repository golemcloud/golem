///|
fn universal_middleware_source(
  name : String,
  aliases : String,
  handler : String,
) -> String {
  "/// Audits every tool.\n/// Without rebuilding carriers.\n#derive.universal_tool_middleware(\"" +
  name +
  "\"" +
  aliases +
  ")\npub async fn " +
  handler +
  "(\n  invocation : @toolMiddleware.RawToolInvocation,\n  underlying : @toolMiddleware.UnderlyingTool,\n) -> Result[\n  @toolMiddleware.RawInvocationResult,\n  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],\n] {\n  underlying.invoke_raw(invocation.command_path(), invocation.input(), invocation.stdin())\n}\n"
}

///|
test "universal middleware emitter registers metadata and a raw async adapter" {
  let source = universal_middleware_source(
    "audit", ", alias=\"trace-tools\", alias=\"logged-tools\"", "audit_all",
  )
  let middlewares = @lib.parse_tool_middlewares([("audit.mbt", source)])
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_middleware_registration(
      middlewares,
      [],
      @lib.ComponentRole::ToolMiddleware,
    ),
  )
  assert_true(
    generated.contains(
      "async fn __golem_universal_tool_middleware_invoke_audit_all",
    ),
  )
  assert_true(generated.contains("audit_all(invocation, underlying)"))
  assert_true(generated.contains("name: \"audit\""))
  assert_true(
    generated.contains("aliases: [\"trace-tools\", \"logged-tools\"]"),
  )
  assert_true(generated.contains("summary: \"Audits every tool.\""))
  assert_true(
    generated.contains("description: \"Without rebuilding carriers.\""),
  )
  assert_true(
    generated.contains("@toolMiddleware.ToolMiddlewareScope::Universal"),
  )
  assert_true(generated.contains("@toolMiddleware.register_tool_middleware"))
  assert_true(!generated.contains("@model"))
  assert_true(!generated.contains("@schema"))
  assert_true(!generated.contains("@tool."))
}

///|
test "universal middleware emitter handles multiple declarations and empty input" {
  let source = universal_middleware_source("zeta", "", "zeta") +
    universal_middleware_source("alpha", "", "alpha")
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_middleware_registration(
      @lib.parse_tool_middlewares([("middlewares.mbt", source)]),
      [],
      @lib.ComponentRole::ToolMiddleware,
    ),
  )
  assert_true(
    generated.contains("__golem_universal_tool_middleware_invoke_zeta"),
  )
  assert_true(
    generated.contains("__golem_universal_tool_middleware_invoke_alpha"),
  )
  assert_eq(
    @formatter.impls_to_string(
      @lib.generate_tool_middleware_registration(
        [],
        [],
        @lib.ComponentRole::ToolMiddleware,
      ),
    ),
    "",
  )
}

///|
test "monomorphic emitter generates namespaced descriptors wrappers dispatch and metadata" {
  let source =
    #|#derive.tool
    #|struct Echo {}
    #|pub fn Echo::echo(value : String) -> String { value }
    #|
    #|#derive.tool_middleware("echo-policy", presented="Echo")
    #|struct EchoPolicy {}
    #|pub async fn EchoPolicy::echo(
    #|  underlying : EchoUnderlying,
    #|  value : String,
    #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
    #|  underlying.echo(value)
    #|}
  let middlewares = @lib.parse_tool_middlewares([("policy.mbt", source)])
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_middleware_registration(
      middlewares,
      @lib.parse_tools([("policy.mbt", source)]),
      @lib.ComponentRole::ToolMiddleware,
    ),
  )
  assert_true(generated.contains("fn __golem_middleware_tool_def_Echo"))
  assert_false(generated.contains("fn __golem_tool_def_Echo"))
  assert_false(generated.contains("fn __golem_tool_invoke_Echo"))
  assert_true(generated.contains("pub struct EchoUnderlying"))
  assert_true(
    generated.contains("priv underlying : @toolMiddleware.UnderlyingTool"),
  )
  assert_true(generated.contains("pub async fn EchoUnderlying::echo"))
  assert_true(generated.contains("invoke_with("))
  assert_false(generated.contains("ToolClient"))
  assert_true(
    generated.contains(
      "async fn __golem_monomorphic_tool_middleware_invoke_EchoPolicy",
    ),
  )
  assert_true(generated.contains("EchoPolicy::echo"))
  assert_true(
    generated.contains(
      "scope: @toolMiddleware.ToolMiddlewareScope::Monomorphic",
    ),
  )
  assert_true(generated.contains("presented: __golem_middleware_tool_def_Echo"))
  assert_true(generated.contains("expected: __golem_middleware_tool_def_Echo"))
}

///|
test "checked-in authoring fixture generates middleware source" {
  let path = "../golem_sdk/tool-middleware-authoring-test/authoring.mbt"
  let source = @fs.read_file_to_string(path)
  let tools = @lib.parse_tools([(path, source)])
  let middlewares = @lib.parse_tool_middlewares([(path, source)])
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_middleware_registration(
      middlewares,
      tools,
      @lib.ComponentRole::ToolMiddleware,
    ),
  )
  assert_true(generated.contains("pub struct PresentedUnderlying"))
  assert_true(generated.contains("pub struct StorageUnderlying"))
  assert_true(generated.contains("PresentedUnderlying::admin__run"))
  assert_true(generated.contains("StorageUnderlying::fetch"))
  assert_true(generated.contains("[\"admin\", \"run\"]"))
  assert_true(generated.contains("RunError::from_error_payload_value"))
  assert_true(generated.contains("RunError::to_error_payload_value"))
  assert_true(generated.contains("decode_result_empty"))
  assert_true(generated.contains("decode_result_value"))
  assert_true(generated.contains("decode_result_stdout_only"))
  assert_true(generated.contains("decode_result_with_stdout"))
  assert_true(generated.contains("typed_result_empty"))
  assert_true(generated.contains("typed_result_value("))
  assert_true(generated.contains("typed_result_stdout("))
  assert_true(generated.contains("typed_result_value_with_stdout"))
  assert_true(generated.contains("__golem_invocation.principal()"))
  assert_true(generated.contains("__golem_invocation.stdin()"))
  assert_true(generated.contains("fn __golem_middleware_tool_def_Presented"))
  assert_true(generated.contains("fn __golem_middleware_tool_def_Nested"))
  assert_true(generated.contains("fn __golem_middleware_tool_def_Storage"))
  assert_false(generated.contains("fn __golem_tool_def_Presented"))
  assert_false(generated.contains("fn __golem_tool_invoke_Presented"))
  assert_false(generated.contains("ToolClient"))
  assert_true(generated.contains("pub impl @tool.ToolErrorSchema for RunError"))
  let combined = @formatter.impls_to_string(
    @lib.generate_tool_middleware_registration(
      middlewares,
      tools,
      @lib.ComponentRole::Combined,
    ),
  )
  assert_false(combined.contains("pub impl @tool.ToolErrorSchema for RunError"))
  let error = try {
    ignore(
      @lib.generate_tool_middleware_registration(
        middlewares,
        tools,
        @lib.ComponentRole::Ordinary,
      ),
    )
    "NO ERROR"
  } catch {
    @lib.ToolMiddlewareEmitError(message) => message
  } noraise {
    value => value
  }
  inspect(
    error,
    content="component role 'ordinary' does not export tool middleware",
  )
}
