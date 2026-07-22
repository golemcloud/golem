///|
test "tool emitters generate root, leaf, registration, and typed clients" {
  let source =
    #|/// Search files.
    #|/// Root details.
    #|#derive.tool("grep", version="1.2.3")
    #|struct Grep {}
    #|
    #|#derive.arg("verbose", scope="global", kind="flag")
    #|pub fn Grep::grep(verbose : Bool, pattern : String) -> String { "" }
    #|
    #|#derive.arg("verbose", scope="global", kind="flag")
    #|#derive.arg("files", scope="tail")
    #|pub fn Grep::count(verbose : Bool, files : Array[String]) -> Unit { () }
  let tools = @lib.parse_tools([("grep.mbt", source)])
  let registration = @formatter.impls_to_string(
    @lib.generate_tool_registration(tools),
  )
  assert_true(registration.contains("fn __golem_tool_def_Grep"))
  assert_true(registration.contains("@tool.register_tool_def_invoker"))
  assert_true(registration.contains("Grep::count"))
  assert_true(registration.contains("subcommands: [1]"))
  let clients = @formatter.impls_to_string(
    @lib.generate_tool_client_code(tools),
  )
  assert_true(clients.contains("pub(all) struct GrepClient"))
  assert_true(clients.contains("GrepClient::count"))
  assert_true(clients.contains("[\"count\"]"))
  assert_true(clients.contains("@tool.NoToolError"))
}

///|
test "pure dispatcher, subtree authoring, and stdout support" {
  let dispatcher =
    #|#derive.tool
    #|struct Dispatcher {}
    #|pub fn Dispatcher::run(value : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(
      @lib.parse_tools([("dispatcher.mbt", dispatcher)]),
    ),
  )
  assert_true(generated.contains("body: None"))
  let subtree =
    #|#derive.tool
    #|struct Root {}
    #|#derive.command(subtree="Child")
    #|pub fn Root::child() -> Unit { () }
    #|#derive.tool
    #|struct Child {}
    #|pub fn Child::run(value : String) -> Unit { () }
  let subtree_generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(
      @lib.parse_tools([("subtree.mbt", subtree)]),
    ),
  )
  assert_true(subtree_generated.contains("name: \"child\""))
  assert_true(subtree_generated.contains("fn __golem_tool_def_Child"))
  assert_true(subtree_generated.contains("@tool.register_subtool_def_invoker"))
  let subtree_clients = @formatter.impls_to_string(
    @lib.generate_tool_client_code(@lib.parse_tools([("subtree.mbt", subtree)])),
  )
  assert_true(subtree_clients.contains("pub(all) struct RootChildClient"))
  assert_true(subtree_clients.contains("RootClient::child"))
  assert_true(subtree_clients.contains("RootChildClient::run"))
  assert_false(subtree_clients.contains("pub fn ChildClient::new()"))
  let stdout =
    #|#derive.tool
    #|struct Out {}
    #|pub fn Out::out(input : @asyncCore.Stream[Byte], output : @asyncCore.Sink[Byte]) -> Unit { () }
  let stdout_tools = @lib.parse_tools([("stdout.mbt", stdout)])
  let stdout_registration = @formatter.impls_to_string(
    @lib.generate_tool_registration(stdout_tools),
  )
  assert_true(stdout_registration.contains("stdout: Some("))
  assert_true(stdout_registration.contains("match __golem_stdin"))
  assert_true(
    stdout_registration.contains(
      "let (__golem_stdout, output) = @tool.new_stdout()",
    ),
  )
  assert_true(
    stdout_registration.contains(
      "@tool.invocation_result_stdout(__golem_stdout)",
    ),
  )
  let stdout_client = @formatter.impls_to_string(
    @lib.generate_tool_client_code(stdout_tools),
  )
  assert_true(stdout_client.contains("@asyncCore.Stream[Byte]"))
  assert_false(stdout_client.contains("output : @asyncCore.Sink[Byte]"))
  assert_true(stdout_client.contains("@tool.decode_result_stdout_only"))
}

///|
test "labelled stdout is injected and returned by generated invoker" {
  let source =
    #|#derive.tool
    #|struct Out {}
    #|pub fn Out::out(output~ : @asyncCore.Sink[Byte]) -> Unit { ignore(output) }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("stdout.mbt", source)])),
  )
  assert_true(
    generated.contains("let (__golem_stdout, output) = @tool.new_stdout()"),
  )
  assert_true(generated.contains("Out::out(output~)"))
  assert_true(
    generated.contains("@tool.invocation_result_stdout(__golem_stdout)"),
  )
}

///|
test "stdout result does not reuse the stream handle owned by the tool method" {
  let source =
    #|#derive.tool
    #|struct Out {}
    #|pub fn Out::out(output : @asyncCore.Sink[Byte]) -> Unit {
    #|  output.drop()
    #|}
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("stdout.mbt", source)])),
  )
  assert_true(
    generated.contains("let (__golem_stdout, output) = @tool.new_stdout()"),
  )
  assert_true(
    generated.contains("@tool.invocation_result_stdout(__golem_stdout)"),
  )
}

///|
test "stdout parameter is not shadowed by a generated result binding" {
  let source =
    #|#derive.tool
    #|struct Out {}
    #|pub fn Out::out(value : @asyncCore.Sink[Byte]) -> String {
    #|  ignore(value)
    #|  "ok"
    #|}
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("stdout.mbt", source)])),
  )
  assert_false(
    generated.contains(
      "@tool.invocation_result_value_with_stdout(value, value)",
    ),
  )
}

///|
test "stdout parameter cannot collide with reserved result binding" {
  let source =
    #|#derive.tool
    #|struct Out {}
    #|pub fn Out::out(__golem_value : @asyncCore.Sink[Byte]) -> String {
    #|  ignore(__golem_value)
    #|  "ok"
    #|}
  expect_tool_emit_error(source, "reserved __golem_ generator prefix")
}

///|
test "tool generated internals use runtime facades instead of consumer aliases" {
  let source =
    #|#derive.tool
    #|struct Out {}
    #|pub fn Out::out(output : @asyncCore.Sink[Byte]) -> Unit { ignore(output) }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("stdout.mbt", source)])),
  )
  assert_true(generated.contains("@tool.Principal"))
  assert_true(generated.contains("@tool.new_stdout()"))
  assert_false(generated.contains("@common.Principal"))
  assert_false(generated.contains("@stdout.get_stdout()"))
}

///|
test "reexported tool Principal is injected as invocation context" {
  let source =
    #|#derive.tool
    #|struct Whoami {}
    #|pub fn Whoami::whoami(principal : @tool.Principal) -> Unit {
    #|  ignore(principal)
    #|}
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(
      @lib.parse_tools([("principal.mbt", source)]),
    ),
  )
  assert_true(generated.contains("Whoami::whoami(__golem_principal)"))
  assert_false(generated.contains("decode_canonical_field"))
}

///|
test "generated invoker passes diagnostic context to canonical decoders" {
  let source =
    #|#derive.tool
    #|struct Echo {}
    #|pub fn Echo::run(value : String) -> String { value }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("echo.mbt", source)])),
  )
  assert_true(generated.contains("@tool.decode_canonical_input("))
  assert_true(generated.contains("\"tool echo invocation\""))
  assert_true(generated.contains("@tool.decode_canonical_field("))
  assert_true(generated.contains("\"tool echo command run\""))
}

///|
test "value-is constraints are preserved in generated descriptors" {
  let source =
    #|#derive.tool
    #|struct Render {}
    #|#derive.arg("format", scope="option")
    #|#derive.constraint("requires_all", value_is="format=json")
    #|pub fn Render::render(format : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("render.mbt", source)])),
  )
  assert_true(generated.contains("@tool.RefDef::ValueIs"))
}

///|
test "each authored value-is constraint reference is emitted once" {
  let source =
    #|#derive.tool
    #|struct Render {}
    #|#derive.arg("format", scope="option")
    #|#derive.constraint("requires_all", value_is="format=json")
    #|pub fn Render::render(format : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("render.mbt", source)])),
  )
  assert_eq(generated.split("@tool.RefDef::ValueIs").count() - 1, 1)
}

///|
test "globals declared by a leaf command are emitted on that command node" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("token", scope="global")
    #|pub fn Runner::run(token : String) -> String { token }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_eq(generated.split("long: \"token\"").count() - 1, 1)
}

///|
test "options are optional by default" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("format", scope="option")
    #|pub fn Runner::run(format : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("required: false"))
}

///|
test "subtree invoker uses the child tool wire name" {
  let source =
    #|#derive.tool
    #|struct Parent {}
    #|#derive.command(name="child", subtree="Child")
    #|pub fn Parent::child() -> Unit { () }
    #|#derive.tool("wire-child")
    #|struct Child {}
    #|pub fn Child::run(value : String) -> String { value }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("subtree.mbt", source)])),
  )
  assert_true(generated.contains("@tool.invoke_registered_subtool"))
  assert_true(generated.contains("\"wire-child\""))
  assert_true(
    generated.contains(
      (
        #|      __golem_tool_def_raw_Child(),
        #|      "child",
        #|      { options: [], flags: [] },
        #|      Some("child"),
      ),
    ),
  )
  assert_true(
    generated.contains(
      "__golem_tool_def_raw_Child().normalize_inherited_globals()",
    ),
  )
}

///|
test "grafted child root aliases are accepted by the parent invoker" {
  let source =
    #|#derive.tool
    #|struct Parent {}
    #|#derive.command(subtree="Child")
    #|pub fn Parent::child() -> Unit { () }
    #|#derive.tool
    #|struct Child {}
    #|#derive.command(alias="kid")
    #|pub fn Child::child() -> String { "reached" }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("subtree.mbt", source)])),
  )
  assert_true(
    generated.contains(
      (
        #|["child", "kid"].contains(
        #|    __golem_command_path[0],
        #|  )
      ),
    ),
  )
}

///|
test "grafted child root aliases cannot shadow parent commands" {
  let source =
    #|#derive.tool
    #|struct Parent {}
    #|#derive.command(subtree="Child")
    #|pub fn Parent::child() -> Unit { () }
    #|pub fn Parent::run() -> String { "parent" }
    #|#derive.tool
    #|struct Child {}
    #|#derive.command(alias="run")
    #|pub fn Child::child() -> String { "child" }
  expect_tool_emit_error(source, "")
}

///|
test "typed subtree client omits a child global captured by short name" {
  let source =
    #|#derive.tool
    #|struct Parent {}
    #|#derive.command(subtree="Child")
    #|#derive.arg("parent_verbose", scope="global", short="v", kind="flag")
    #|pub fn Parent::child(parent_verbose : Bool) -> Unit { () }
    #|#derive.tool
    #|struct Child {}
    #|#derive.arg("child_verbose", scope="global", short="v", kind="flag")
    #|pub fn Child::child(child_verbose : Bool) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_client_code(@lib.parse_tools([("subtree.mbt", source)])),
  )
  assert_true(generated.contains("ParentClient::child"))
  assert_true(generated.contains("parent_verbose : Bool"))
  assert_false(generated.contains("child_verbose : Bool"))
}

///|
test "typed leaf client uses an inherited global captured by short name" {
  let source =
    #|#derive.tool
    #|struct Root {}
    #|#derive.arg("root_verbose", scope="global", short="v", kind="flag")
    #|pub fn Root::root(root_verbose : Bool) -> Unit { () }
    #|#derive.arg("leaf_verbose", scope="global", short="v", kind="flag")
    #|pub fn Root::run(leaf_verbose : Bool) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_client_code(@lib.parse_tools([("short.mbt", source)])),
  )
  assert_true(generated.contains("RootClient::run"))
  assert_true(generated.contains("root_verbose : Bool"))
  assert_true(
    generated.contains("(\"root-verbose\", @schema.to_value_as(root_verbose))"),
  )
  assert_false(generated.contains("leaf_verbose"))
}

///|
test "invoker decodes a reverse-alias global through the inherited canonical name" {
  let source =
    #|#derive.tool
    #|struct Root {}
    #|#derive.arg("format", scope="global")
    #|pub fn Root::root(format : String) -> Unit { () }
    #|#derive.arg("count", scope="global", alias="format")
    #|pub fn Root::run(count : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("alias.mbt", source)])),
  )
  assert_true(generated.contains("let count :String = try"))
  assert_true(
    generated.contains(
      (
        #|          "format",
        #|          "tool root command run",
      ),
    ),
  )
}

///|
test "bug finder short-deprojected child root global does not shadow leaf input" {
  let source =
    #|#derive.tool
    #|struct Parent {}
    #|#derive.command(subtree="Child")
    #|#derive.arg("count", scope="global", short="f")
    #|pub fn Parent::child(count : String) -> Unit { () }
    #|#derive.tool
    #|struct Child {}
    #|#derive.arg("format", scope="global", short="f")
    #|pub fn Child::child(format : String) -> Unit { () }
    #|#derive.arg("format", scope="option")
    #|pub fn Child::run(format : String) -> Unit { () }
  let tools = @lib.parse_tools([("subtree.mbt", source)])
  let registration = @formatter.impls_to_string(
    @lib.generate_tool_registration(tools),
  ) catch {
    @lib.ToolEmitError(message) =>
      fail("valid grafted tool was rejected: " + message)
  }
  let clients = @formatter.impls_to_string(
    @lib.generate_tool_client_code(tools),
  ) catch {
    @lib.ToolEmitError(message) =>
      fail("valid grafted client was rejected: " + message)
  }
  assert_true(registration.contains("@tool.register_subtool_def_invoker"))
  assert_true(clients.contains("ParentClient::child"))
  assert_true(clients.contains("count : String"))
  assert_true(clients.contains("ParentChildClient::run"))
  assert_true(clients.contains("format : String"))
}

///|
test "distinct subtree paths do not emit duplicate client wrapper types" {
  let source =
    #|#derive.tool
    #|struct Root {}
    #|#derive.command(name="foo-bar", subtree="Leaf")
    #|pub fn Root::direct() -> Unit { () }
    #|#derive.command(name="foo", subtree="Middle")
    #|pub fn Root::middle() -> Unit { () }
    #|#derive.tool
    #|struct Middle {}
    #|#derive.command(name="bar", subtree="Leaf")
    #|pub fn Middle::nested() -> Unit { () }
    #|#derive.tool
    #|struct Leaf {}
    #|pub fn Leaf::run() -> Unit { () }
  try
    @lib.generate_tool_client_code(@lib.parse_tools([("subtree.mbt", source)]))
  catch {
    @lib.ToolEmitError(message) => {
      assert_true(message.contains("RootFooBarClient"))
      assert_true(message.contains("root/foo-bar"))
      assert_true(message.contains("root/foo/bar"))
      assert_true(message.contains("name="))
    }
    _ => fail("expected colliding generated client type error")
  } noraise {
    _ => fail("expected colliding generated client type error")
  }
}

///|
test "top-level and subtree-only tools cannot share a wire name" {
  let source =
    #|#derive.tool("same")
    #|struct Child {}
    #|pub fn Child::leaf() -> Unit { () }
    #|
    #|#derive.tool("root")
    #|struct Root {}
    #|#derive.command(subtree="Child")
    #|pub fn Root::child() -> Unit { () }
    #|
    #|#derive.tool("same")
    #|struct Standalone {}
    #|pub fn Standalone::run() -> Unit { () }
  expect_tool_emit_error(source, "same")
}

///|
test "command aliases are accepted and preserved" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.command(alias="r")
    #|pub fn Runner::run() -> String { "ok" }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("aliases: [\"r\"]"))
}

///|
test "custom tool errors encode the declared case payload" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::run() -> Result[String, RunError] { Err(Broken("boom")) }
    #|#derive.golem_schema
    #|enum RunError {
    #|  #derive.error(kind="runtime-error")
    #|  Broken(String)
    #|}
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("impl @tool.ToolErrorSchema for RunError"))
  assert_true(generated.contains("with to_error_payload_value"))
  assert_true(
    generated.contains("try_into_typed_schema_value(__golem_payload)"),
  )
  assert_true(generated.contains("RunError::to_error_payload_value(error)"))
}

///|
test "raised custom tool errors encode the declared case payload" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::run() -> String raise RunError { raise Broken("boom") }
    #|suberror RunError {
    #|  #derive.error(kind="runtime-error")
    #|  Broken(String)
    #|}
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("Broken(payload) => Err(Broken(payload))"))
  assert_true(
    generated.contains("try_into_typed_schema_value(__golem_payload)"),
  )
  assert_true(generated.contains("RunError::to_error_payload_value(error)"))
}

///|
test "tool error schemas are reusable across descriptors and typed clients" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::first() -> Result[String, RunError] { Err(Broken("boom")) }
    #|pub fn Runner::second() -> Result[String, RunError] { Err(NotFound) }
    #|enum RunError {
    #|  #derive.error(kind="runtime-error")
    #|  Broken(String)
    #|  #derive.error(kind="usage-error", exit_code="4")
    #|  NotFound
    #|}
  let tools = @lib.parse_tools([("runner.mbt", source)])
  let registration = @formatter.impls_to_string(
    @lib.generate_tool_registration(tools),
  )
  assert_eq(
    registration.split("impl @tool.ToolErrorSchema for RunError").count() - 1,
    3,
  )
  assert_true(registration.contains("with error_cases"))
  assert_true(registration.contains("payload: None"))
  assert_true(registration.contains("exit_code: 4"))
  assert_true(registration.contains("try_into_typed_schema_value(())"))
  assert_true(registration.contains("with from_error_payload_value"))
  let clients = @formatter.impls_to_string(
    @lib.generate_tool_client_code(tools),
  )
  assert_true(clients.contains("invoke_and_await_tool_error"))
  assert_false(clients.contains("__golem_tool_decode_error"))
}

///|
test "tool error schemas preserve labelled payload constructors" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::run() -> Result[String, RunError] { Err(Broken(message="boom")) }
    #|pub fn Runner::raised() -> String raise RunError { raise Broken(message="boom") }
    #|enum RunError {
    #|  #derive.error(kind="runtime-error")
    #|  Broken(message~ : String)
    #|}
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("Broken(message~)"))
  assert_true(generated.contains("try_into_typed_schema_value(message)"))
  assert_true(generated.contains("Broken(message=__golem_payload)"))
  assert_true(generated.contains("Broken(message~) => Err(Broken(message~))"))
}

///|
test "generated clients handle raising canonical input APIs" {
  let source =
    #|#derive.tool
    #|struct Echo {}
    #|pub fn Echo::run(value : String) -> String { value }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_client_code(@lib.parse_tools([("echo.mbt", source)])),
  )
  assert_false(
    generated.contains("match def.canonical_input_model(index) {\n    Ok("),
  )
  assert_false(
    generated.contains(
      "match @tool.build_canonical_input(model, values) {\n    Ok(",
    ),
  )
}

///|
test "inout path direction uses the declared MoonBit constructor" {
  let source =
    #|#derive.tool
    #|struct Files {}
    #|#derive.arg("path", kind="file", direction="inout")
    #|pub fn Files::open(path : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("files.mbt", source)])),
  )
  assert_true(generated.contains("@types.PathDirection::IN_OUT"))
  assert_false(generated.contains("@types.PathDirection::INOUT"))
}

///|
fn expect_tool_emit_error(source : String, expected : String) -> Unit raise {
  try
    @lib.generate_tool_registration(
      @lib.parse_tools([("invalid_tool.mbt", source)]),
    )
  catch {
    @lib.ToolEmitError(message) => assert_true(message.contains(expected))
    @lib.ToolParseError(message) => assert_true(message.contains(expected))
    error => fail("unexpected error: " + error.to_string())
  } noraise {
    _ => fail("expected invalid tool definition to be rejected")
  }
}

///|
test "duplicate sibling command names are rejected" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.command(name="same")
    #|pub fn Runner::first() -> String { "first" }
    #|#derive.command(name="same")
    #|pub fn Runner::second() -> String { "second" }
  expect_tool_emit_error(source, "same")
}

///|
test "root subcommand indices follow emitted node order" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::first() -> String { "first" }
    #|pub fn Runner::runner() -> String { "root" }
    #|pub fn Runner::second() -> String { "second" }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("subcommands: [1, 2]"))
}

///|
test "duplicate argument short forms are rejected" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("left", scope="option", short="x")
    #|#derive.arg("right", scope="option", short="x")
    #|pub fn Runner::run(left : String, right : String) -> Unit { () }
  expect_tool_emit_error(source, "x")
}

///|
test "constraint references must resolve to a declared argument" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.constraint("requires_all", "missing")
    #|pub fn Runner::run(value : String) -> Unit { () }
  expect_tool_emit_error(source, "missing")
}

///|
test "at most one argument can be the tail positional" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("left", scope="tail")
    #|#derive.arg("right", scope="tail")
    #|pub fn Runner::run(left : Array[String], right : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "tail")
}

///|
test "a tail positional cannot precede a fixed positional" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="tail")
    #|pub fn Runner::run(items : Array[String], target : String) -> Unit { () }
  expect_tool_emit_error(source, "tail")
}

///|
test "tail min and max only constrain occurrence count" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="tail", min="1", max="3")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("min: 1U"))
  assert_true(generated.contains("max: Some(3U)"))
  assert_false(generated.contains("@tool.refine_numeric"))
}

///|
test "generated invokers preserve labelled parameter calls" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::run(value~ : String) -> String { value }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("Runner::run(value~)"))
}

///|
test "generated invokers preserve question-optional parameter calls" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::run(value? : String) -> String { value.unwrap_or("") }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("Runner::run(value?)"))
}

///|
test "implies defaults both quantifiers to all" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("a", kind="flag")
    #|#derive.arg("b", kind="flag")
    #|#derive.arg("c", kind="flag")
    #|#derive.constraint("implies", lhs="a", rhs="b", rhs="c")
    #|pub fn Runner::run(a : Bool, b : Bool, c : Bool) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("rhs_quant: @toolCommon.Quantifier::ALL"))
}

///|
test "result formatting is rejected for a unit success type" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.result("json", default="json")
    #|pub fn Runner::run() -> Unit { () }
  expect_tool_emit_error(source, "unit")
}

///|
test "delimited repeatable options require an explicit delimiter" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="option", repeatable="delimited")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "delim")
}

///|
test "delimiter metadata requires a delimiter-aware repetition mode" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="option", delim=",")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "delim")
}

///|
test "repeatable option delimiters must be one character" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="option", repeatable="delimited", delim="::")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "character")
}

///|
test "repeatable delimiter metadata is rejected on scalar parameters" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("item", scope="option", repeatable="delimited", delim=",")
    #|pub fn Runner::run(item : String) -> Unit { () }
  expect_tool_emit_error(source, "repeatable")
}

///|
test "a one-character backslash delimiter is accepted" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="option", repeatable="delimited", delim="\\")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  ignore(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  ) catch {
    @lib.ToolEmitError(message) | @lib.ToolParseError(message) => fail(message)
    error => fail("unexpected error: " + error.to_string())
  }
}

///|
test "a one-character form-feed escape delimiter is accepted" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="option", repeatable="delimited", delim="\f")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  ignore(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  ) catch {
    @lib.ToolEmitError(message) | @lib.ToolParseError(message) => fail(message)
    error => fail("unexpected error: " + error.to_string())
  }
}

///|
test "an out-of-range unicode escape delimiter is rejected as metadata" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="option", repeatable="delimited", delim="\u{110000}")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "character")
}

///|
test "a surrogate unicode escape delimiter is rejected as metadata" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="option", repeatable="delimited", delim="\u{d800}")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "character")
}

///|
test "escaped short forms are decoded and emitted canonically" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("item", scope="option", short="\x76")
    #|pub fn Runner::run(item : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("short: Some('\\u{76}')"))
}

///|
test "out-of-range unicode escape short forms are rejected as metadata" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("item", scope="option", short="\u{110000}")
    #|pub fn Runner::run(item : String) -> Unit { () }
  expect_tool_emit_error(source, "character")
}

///|
test "value refinements are rejected on flags" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("verbose", kind="flag", regex=".+")
    #|pub fn Runner::run(verbose : Bool) -> Unit { () }
  expect_tool_emit_error(source, "flag")
}

///|
test "value refinements are rejected on map options" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("labels", scope="option", min="1")
    #|pub fn Runner::run(labels : Map[String, String]) -> Unit { () }
  expect_tool_emit_error(source, "map option")
}

///|
test "max remains valid as a count-flag occurrence cap" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("verbose", kind="count-flag", max="3")
    #|pub fn Runner::run(verbose : UInt) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("CountFlag(Some(3U))"))
}

///|
test "count-flag max must be a u32 literal" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("verbose", kind="count-flag", max="limit")
    #|pub fn Runner::run(verbose : UInt) -> Unit { () }
  expect_tool_emit_error(source, "max")
}

///|
test "constraint quantifiers reject unknown values" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("a", kind="flag")
    #|#derive.arg("b", kind="flag")
    #|#derive.constraint("implies", lhs="a", rhs="b", rhs_quant="some")
    #|pub fn Runner::run(a : Bool, b : Bool) -> Unit { () }
  expect_tool_emit_error(source, "quant")
}

///|
test "forbids rejects unsupported rhs quantifier metadata" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("a", kind="flag")
    #|#derive.arg("b", kind="flag")
    #|#derive.constraint("forbids", lhs="a", rhs="b", rhs_quant="any")
    #|pub fn Runner::run(a : Bool, b : Bool) -> Unit { () }
  expect_tool_emit_error(source, "rhs_quant")
}

///|
test "verbatim tail requires a separator" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("args", scope="tail", verbatim=true)
    #|pub fn Runner::run(args : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "separator")
}

///|
test "implicit root command preserves authored aliases" {
  let source =
    #|#derive.tool("runner")
    #|struct Runner {}
    #|#derive.command(alias="r")
    #|pub fn Runner::runner() -> String { "ok" }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("aliases: [\"r\"]"))
}

///|
test "typed client preserves a parameter named values" {
  let source =
    #|#derive.tool
    #|struct Echo {}
    #|pub fn Echo::run(values : String) -> String { values }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_client_code(@lib.parse_tools([("echo.mbt", source)])),
  )
  assert_false(
    generated.contains(
      "let values = []\n  values.push((\"values\", @schema.to_value_as(values)))",
    ),
  )
  assert_true(
    generated.contains(
      "__golem_values.push((\"values\", @schema.to_value_as(values)))",
    ),
  )
}

///|
test "typed client lifecycle method does not collide with a drop command" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::drop() -> Unit { () }
  expect_tool_emit_error(source, "reserved")
}

///|
test "argument aliases are accepted and preserved" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("value", scope="option", alias="v")
    #|pub fn Runner::run(value : String) -> Unit { () }
  let generated = @formatter.impls_to_string(
    @lib.generate_tool_registration(@lib.parse_tools([("runner.mbt", source)])),
  )
  assert_true(generated.contains("aliases: [\"v\"]"))
}

///|
test "tail bounds must be UInt literals" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("items", scope="tail", max="4294967296")
    #|pub fn Runner::run(items : Array[String]) -> Unit { () }
  expect_tool_emit_error(source, "max")
}

///|
test "accepts_stdio is rejected on options" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("input", scope="option", accepts_stdio=true)
    #|pub fn Runner::run(input : String) -> Unit { () }
  expect_tool_emit_error(source, "accepts_stdio")
}

///|
test "invalid tool identifiers are rejected before registration" {
  let source =
    #|#derive.tool("Bad_Name")
    #|struct Bad {}
  expect_tool_emit_error(source, "identifier")
}

///|
test "structural metadata is closed over each inferred argument surface" {
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("verbose", required=true)
      #|pub fn Runner::run(verbose : Bool) -> Unit { () }
    ),
    "required",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("args", scope="tail", default="value")
      #|pub fn Runner::run(args : Array[String]) -> Unit { () }
    ),
    "default",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("value", alias="v")
      #|pub fn Runner::run(value : String) -> Unit { () }
    ),
    "alias",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("verbose", value_name="LEVEL")
      #|pub fn Runner::run(verbose : Bool) -> Unit { () }
    ),
    "value_name",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("stream", required=true)
      #|pub fn Runner::run(stream : @asyncCore.Stream[Byte]) -> Unit { () }
    ),
    "required",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("principal", doc="caller")
      #|pub fn Runner::run(principal : @agentCommon.Principal) -> Unit { () }
    ),
    "Principal",
  )
}

///|
test "boolean flag defaults are validated" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("verbose", default="yes")
    #|pub fn Runner::run(verbose : Bool) -> Unit { () }
  expect_tool_emit_error(source, "true or false")
}

///|
test "text UInt bounds and their ordering are validated" {
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("value", min_length="many")
      #|pub fn Runner::run(value : String) -> Unit { () }
    ),
    "min_length",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("value", min_length="5", max_length="2")
      #|pub fn Runner::run(value : String) -> Unit { () }
    ),
    "cannot exceed",
  )
}

///|
test "all emitted contract identifiers use lower-kebab-case" {
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.command(name="BadCommand")
      #|pub fn Runner::run() -> Unit { () }
    ),
    "command identifier",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.arg("value", scope="option", alias="BadAlias")
      #|pub fn Runner::run(value : String) -> Unit { () }
    ),
    "argument alias identifier",
  )
  expect_tool_emit_error(
    (
      #|#derive.tool
      #|struct Runner {}
      #|#derive.result(formatter="Pretty_JSON")
      #|pub fn Runner::run() -> String { "" }
    ),
    "result formatter identifier",
  )
}

///|
test "child argument surfaces cannot shadow inherited globals" {
  let source =
    #|#derive.tool("runner")
    #|struct Runner {}
    #|#derive.arg("global_value", scope="global", name="token")
    #|pub fn Runner::runner(global_value : String) -> Unit { () }
    #|#derive.arg("local_value", scope="option", name="token")
    #|pub fn Runner::run(local_value : String) -> Unit { () }
  expect_tool_emit_error(source, "token")
}

///|
fn canonical_tool(tools : Array[@lib.ToolInfo], name : String) -> @lib.ToolInfo {
  for tool in tools {
    if tool.name == name {
      return tool
    }
  }
  abort("missing canonical tool: " + name)
}

///|
fn canonical_command(
  tool : @lib.ToolInfo,
  name : String,
) -> @lib.ToolCommandInfo {
  for command in tool.commands {
    if command.name == name {
      return command
    }
  }
  abort("missing canonical command: " + tool.name + " " + name)
}

///|
test "canonical MoonBit tools preserve the shared Rust and Scala contract" {
  let package = @lib.PackageInfo::{
    rel_path: "golem_moonbit_examples",
    dir: "../golem_sdk_example1/golem_moonbit_examples",
  }
  let tools = @lib.parse_tools(@lib.scan_package(package).files)
  assert_eq(tools.length(), 5)
  let grep = canonical_tool(tools, "canonical-grep")
  assert_eq(grep.version, Some("2.0.0"))
  guard grep.root_command is Some(grep_root_index) else {
    fail("canonical grep must have a root command")
  }
  let grep_root = grep.commands[grep_root_index]
  assert_true(
    grep_root.params.any(param => param.1 is @lib.Hidden(InputStream)),
  )
  assert_true(
    grep_root.params.any(param => param.1 is @lib.Hidden(OutputStream)),
  )
  inspect(
    grep_root.errors.map(error => "\{error.name}:\{error.exit_code}"),
    content="[InvalidPattern:Some(2), NoMatch:Some(1)]",
  )
  let git = canonical_tool(tools, "canonical-git")
  assert_true(git.root_command is None)
  let commit = canonical_command(git, "commit")
  assert_true(commit.aliases.contains("ci"))
  assert_eq(commit.destructive, Some(true))
  guard commit.result is Some(result) else {
    fail("canonical commit must define result formatters")
  }
  inspect(result.formatters, content="[human, porcelain, json]")
  inspect(result.defaults, content="[human]")
  let log = canonical_command(git, "log")
  assert_eq(log.read_only, Some(true))
  assert_eq(log.idempotent, Some(true))
  assert_eq(canonical_command(git, "remote").subtree, Some("CanonicalRemote"))
  assert_eq(canonical_command(git, "stash").subtree, Some("CanonicalStash"))
  let remote = canonical_tool(tools, "remote")
  assert_true(canonical_command(remote, "remove").aliases.contains("rm"))
  let set_url = canonical_command(remote, "set-url")
  assert_true(
    set_url.constraints.any(constraint => constraint.kind == "mutex_groups"),
  )
  let stash = canonical_tool(tools, "stash")
  assert_true(stash.root_command is Some(_))
  assert_true(stash.commands.any(command => command.name == "pop"))
  let big = canonical_tool(tools, "canonical-big-bound")
  let big_root = big.commands[big.root_command.unwrap()]
  guard big_root.args[0].max is Some(max) else {
    fail("canonical big bound must have a maximum")
  }
  assert_eq(max, "18446744073709551615")
  let registration = @formatter.impls_to_string(
    @lib.generate_tool_registration(tools),
  )
  assert_eq(registration.split("@tool.register_tool_def_invoker").length(), 4)
  assert_eq(
    registration.split("@tool.register_subtool_def_invoker").length(),
    3,
  )
  let clients = @formatter.impls_to_string(
    @lib.generate_tool_client_code(tools),
  )
  assert_true(clients.contains("pub(all) struct CanonicalGitRemoteClient"))
  assert_true(clients.contains("pub(all) struct CanonicalGitStashClient"))
  assert_false(clients.contains("pub fn RemoteClient::new()"))
  assert_false(clients.contains("pub fn StashClient::new()"))
}
