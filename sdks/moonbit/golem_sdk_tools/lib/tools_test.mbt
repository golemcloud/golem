///|
test "tool names, version, root, subcommand, and pure dispatcher" {
  let tools = @lib.parse_tools([
    (
      "tools.mbt",
      (
        #|/// HTTP API tool.
        #|#derive.tool
        #|struct HTTPAPI {}
        #|
        #|pub fn HTTPAPI::http_api() -> String { "root" }
        #|pub fn HTTPAPI::list_items(limit : Int) -> Array[String] { [] }
        #|
        #|#derive.tool("custom", version="2.0.0")
        #|struct CustomTool {}
        #|
        #|#derive.tool
        #|struct EmptyDispatcher {}
        #|
        #|fn EmptyDispatcher::helper() -> Unit {}
        #|
      ),
    ),
  ])
  debug_inspect(
    tools,
    content=(
      #|[
      #|  {
      #|    source_name: "HTTPAPI",
      #|    name: "httpapi",
      #|    version: None,
      #|    description: "HTTP API tool.",
      #|    examples: [],
      #|    commands: [
      #|      {
      #|        source_name: "http_api",
      #|        name: "http-api",
      #|        aliases: [],
      #|        subtree: None,
      #|        read_only: None,
      #|        destructive: None,
      #|        idempotent: None,
      #|        open_world: None,
      #|        params: [],
      #|        args: [],
      #|        success_type: Some(Simple("String")),
      #|        error_type: None,
      #|        error_via_result: false,
      #|        errors: [],
      #|        result: None,
      #|        constraints: [],
      #|        examples: [],
      #|        description: "",
      #|        is_root: false,
      #|      },
      #|      {
      #|        source_name: "list_items",
      #|        name: "list-items",
      #|        aliases: [],
      #|        subtree: None,
      #|        read_only: None,
      #|        destructive: None,
      #|        idempotent: None,
      #|        open_world: None,
      #|        params: [("limit", Public(Simple("Int")))],
      #|        args: [],
      #|        success_type: Some(List(Simple("String"))),
      #|        error_type: None,
      #|        error_via_result: false,
      #|        errors: [],
      #|        result: None,
      #|        constraints: [],
      #|        examples: [],
      #|        description: "",
      #|        is_root: false,
      #|      },
      #|    ],
      #|    root_command: None,
      #|  },
      #|  {
      #|    source_name: "CustomTool",
      #|    name: "custom",
      #|    version: Some("2.0.0"),
      #|    description: "",
      #|    examples: [],
      #|    commands: [],
      #|    root_command: None,
      #|  },
      #|  {
      #|    source_name: "EmptyDispatcher",
      #|    name: "empty-dispatcher",
      #|    version: None,
      #|    description: "",
      #|    examples: [],
      #|    commands: [],
      #|    root_command: None,
      #|  },
      #|]
    ),
  )
  inspect(
    tools[0].client_pascal_case(),
    content=(
      #|Httpapi
    ),
  )
}

///|
test "tool metadata, hidden types, result, and constraints" {
  let tools = @lib.parse_tools([
    (
      "tool.mbt",
      (
        #|#derive.example("tool --help", title="Help")
        #|#derive.tool("tool")
        #|struct Tool {}
        #|
        #|pub fn Tool::execute(path : String, principal : @common.Principal, input : @streams.InputStream, output : @streams.OutputStream) -> String { ... }
        #|
      ),
    ),
  ])
  guard tools is [tool] else {
    fail("expected one parsed tool, got \{tools.length()}")
  }
  guard tool.commands is [command] else { fail("expected one parsed command") }
  inspect(command.name, content="execute")
  debug_inspect(
    command.params,
    content=(
      #|[
      #|  ("path", Public(Simple("String"))),
      #|  ("principal", Hidden(Principal)),
      #|  ("input", Hidden(InputStream)),
      #|  ("output", Hidden(OutputStream)),
      #|]
    ),
  )
  inspect(command.success_type is Some(_), content="true")
}

///|
test "typed raise and suberror variants" {
  let tools = @lib.parse_tools([
    (
      "tool.mbt",
      (
        #|#derive.tool
        #|struct Runner {}
        #|pub fn Runner::run(value : Int) -> String raise RunError { ... }
        #|suberror RunError {
        #|  #derive.error(kind="runtime-error", exit_code="7")
        #|  Broken(String)
        #|}
        #|
      ),
    ),
  ])
  debug_inspect(
    tools[0].commands[0].error_type,
    content="Some(Simple(\"RunError\"))",
  )
  debug_inspect(
    tools[0].commands[0].errors[0].payload,
    content="Some(Simple(\"String\"))",
  )
  debug_inspect(tools[0].commands[0].errors[0].payload_label, content="None")
}

///|
test "labelled tool error payload" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::run() -> Result[String, RunError] { Err(Broken(message="x")) }
    #|enum RunError {
    #|  #derive.error(kind="runtime-error")
    #|  Broken(message~ : String)
    #|}
  let tools = @lib.parse_tools([("tool.mbt", source)])
  debug_inspect(
    tools[0].commands[0].errors[0].payload_label,
    content="Some(\"message\")",
  )
}

///|
fn parse_error(source : String) -> String {
  try @lib.parse_tools([("bad.mbt", source)]) catch {
    @lib.ToolParseError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
}

///|
test "strict tool parser diagnostics" {
  inspect(
    parse_error("#derive.tool\nstruct Bad { value : Int }\n"),
    content="#derive.tool struct 'Bad' must be empty",
  )
  inspect(
    parse_error(
      "#derive.tool\nstruct Bad {}\npub fn Bad::run(self : Self) -> Unit {}\n",
    ),
    content="Tool command 'Bad::run' must not have a self receiver",
  )
  inspect(
    parse_error(
      "#derive.tool\nstruct Bad {}\npub async fn Bad::run() -> Unit { ... }\n",
    ),
    content="Async tool command 'Bad::run' is unsupported: the MoonBit WIT export invoker is synchronous and no block_on is available",
  )
  inspect(
    parse_error(
      "#derive.tool\nstruct Bad {}\n#derive.arg(\"missing\", kind=\"flag\")\npub fn Bad::run(ok : Bool) -> Unit {}\n",
    ),
    content="#derive.arg references missing parameter 'missing' on Bad::run",
  )
  inspect(
    parse_error(
      "#derive.tool\nstruct Bad {}\npub fn Bad::run(p : Principal) -> Unit {}\n",
    ),
    content="Hidden SDK type 'Principal' must use its exact qualified name",
  )
}

///|
test "subtree commands accept an implicit Unit return" {
  let source =
    #|#derive.tool
    #|struct Root {}
    #|#derive.command(subtree="Child")
    #|pub fn Root::child() { () }
    #|#derive.tool
    #|struct Child {}
    #|pub fn Child::run() -> Unit { () }
  let tools = @lib.parse_tools([("subtree.mbt", source)])
  assert_eq(tools.length(), 2)
}

///|
test "a public method named new gets an explicit reserved-name diagnostic" {
  let source =
    #|#derive.tool
    #|struct Project {}
    #|pub fn Project::new(name : String) -> String { name }
  let message = parse_error(source)
  assert_true(message.contains("Project::new"))
  assert_true(message.contains("reserved"))
}

///|
test "custom error exit codes must fit the declared Byte field" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|pub fn Runner::run() -> Result[String, RunError] { Err(Broken("boom")) }
    #|#derive.golem_schema
    #|enum RunError {
    #|  #derive.error(kind="runtime-error", exit_code="256")
    #|  Broken(String)
    #|}
  let message = parse_error(source)
  assert_false(message == "NO ERROR")
  assert_true(message.contains("exit_code"))
}

///|
test "a parameter cannot have duplicate derive.arg declarations" {
  let source =
    #|#derive.tool
    #|struct Runner {}
    #|#derive.arg("value", scope="option", name="first")
    #|#derive.arg("value", scope="option", name="second")
    #|pub fn Runner::run(value : String) -> Unit { () }
  let message = parse_error(source)
  assert_false(message == "NO ERROR")
  assert_true(message.contains("duplicate"))
  assert_true(message.contains("value"))
}
