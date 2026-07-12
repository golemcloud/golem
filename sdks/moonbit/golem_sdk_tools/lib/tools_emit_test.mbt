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
test "pure dispatcher, subtree authoring, and stdout diagnostics" {
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
  let subtree_clients = @formatter.impls_to_string(
    @lib.generate_tool_client_code(@lib.parse_tools([("subtree.mbt", subtree)])),
  )
  assert_true(subtree_clients.contains("pub(all) struct RootChildClient"))
  assert_true(subtree_clients.contains("RootClient::child"))
  assert_true(subtree_clients.contains("RootChildClient::run"))
  let stdout =
    #|#derive.tool
    #|struct Out {}
    #|pub fn Out::out(stream : @streams.OutputStream) -> Unit { () }
  try
    @lib.generate_tool_client_code(@lib.parse_tools([("stdout.mbt", stdout)]))
  catch {
    @lib.ToolEmitError(message) => assert_true(message.contains("stdout"))
    error => fail("unexpected error: " + error.to_string())
  } noraise {
    _ => fail("expected stdout diagnostic")
  }
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
  assert_true(generated.contains("Broken(payload)"))
  assert_true(generated.contains("try_into_typed_schema_value(payload)"))
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
  assert_true(generated.contains("Broken(payload)"))
  assert_true(generated.contains("try_into_typed_schema_value(payload)"))
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
      #|pub fn Runner::run(stream : @streams.InputStream) -> Unit { () }
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
