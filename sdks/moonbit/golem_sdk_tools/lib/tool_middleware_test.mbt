///|
fn middleware_parse_error(source : String) -> String {
  try @lib.parse_tool_middlewares([("middleware.mbt", source)]) catch {
    @lib.ToolMiddlewareParseError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
}

///|
fn monomorphic_scope(
  middleware : @lib.ToolMiddlewareInfo,
) -> @lib.MonomorphicToolMiddlewareInfo {
  match middleware.scope {
    @lib.ToolMiddlewareScopeInfo::Monomorphic(scope) => scope
    _ => abort("expected monomorphic middleware")
  }
}

///|
fn middleware_handler(
  handlers : Array[@lib.ToolMiddlewareCommandInfo],
  name : String,
) -> @lib.ToolMiddlewareCommandInfo {
  for handler in handlers {
    if handler.handler_name == name {
      return handler
    }
  }
  abort("missing middleware handler " + name)
}

///|
test "monomorphic authoring projects direct parameters, streams, principal, and errors" {
  let source =
    #|#derive.tool("presented")
    #|struct Presented {}
    #|pub fn Presented::unit() -> Unit { () }
    #|pub fn Presented::value(value : String) -> Int { value.length() }
    #|pub fn Presented::stdout(output : @asyncCore.Sink[Byte]) -> Unit { ignore(output) }
    #|pub fn Presented::value_stdout(value : String, output : @asyncCore.Sink[Byte]) -> Int { ignore(output); value.length() }
    #|pub fn Presented::io(input : @asyncCore.Stream[Byte], output : @asyncCore.Sink[Byte], principal : @common.Principal) -> Result[String, RunError] { ... }
    #|
    #|#derive.golem_schema
    #|enum RunError {
    #|  #derive.error(kind="runtime-error")
    #|  Broken(String)
    #|}
    #|
    #|/// Enforces the complete presented surface.
    #|#derive.tool_middleware("policy", presented="Presented", alias="safe", alias="audited")
    #|struct Policy {}
    #|pub async fn Policy::unit(underlying : PresentedUnderlying) -> Result[Unit, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Policy::value(underlying : PresentedUnderlying, value : String) -> Result[Int, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Policy::stdout(underlying : PresentedUnderlying) -> Result[@asyncCore.Stream[Byte], @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Policy::value_stdout(underlying : PresentedUnderlying, value : String) -> Result[(Int, @asyncCore.Stream[Byte]), @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Policy::io(underlying : PresentedUnderlying, input : @toolMiddleware.MiddlewareStdin, principal : @tool.Principal) -> Result[(String, @asyncCore.Stream[Byte]), @toolMiddleware.ToolInvokeError[RunError]] { ... }
  let middlewares = @lib.parse_tool_middlewares([("middleware.mbt", source)])
  guard middlewares is [middleware] else { fail("expected one middleware") }
  assert_eq(middleware.name, "policy")
  debug_inspect(middleware.aliases, content="[\"safe\", \"audited\"]")
  assert_eq(middleware.description, "Enforces the complete presented surface.")
  let scope = monomorphic_scope(middleware)
  assert_eq(scope.presented.source_name, "Presented")
  assert_eq(scope.expected.source_name, "Presented")
  assert_eq(scope.handlers.length(), 5)
  assert_true(
    middleware_handler(scope.handlers, "unit").success_type ==
    @lib.ParamType::Simple("Unit"),
  )
  assert_true(
    middleware_handler(scope.handlers, "stdout").success_type ==
    @lib.ParamType::Parameterized("@asyncCore.Stream", [
      @lib.ParamType::Simple("Byte"),
    ]),
  )
  debug_inspect(
    middleware_handler(scope.handlers, "io").params,
    content=(
      #|[
      #|  ("input", Simple("@toolMiddleware.MiddlewareStdin")),
      #|  ("principal", Simple("@tool.Principal")),
      #|]
    ),
  )
  debug_inspect(
    middleware_handler(scope.handlers, "io").param_sources,
    content="[Stdin, Principal]",
  )
  assert_true(
    middleware_handler(scope.handlers, "io").error_type ==
    @lib.ParamType::Simple("RunError"),
  )
  debug_inspect(
    middleware_handler(scope.underlying_methods, "io").params,
    content="[(\"input\", Simple(\"@toolMiddleware.MiddlewareStdin\"))]",
  )
}

///|
test "adapter authoring separates presented handlers from expected underlying methods" {
  let source =
    #|#derive.tool("public-files")
    #|struct PublicFiles {}
    #|pub fn PublicFiles::read(path : String) -> String { path }
    #|#derive.tool("storage")
    #|struct Storage {}
    #|pub fn Storage::fetch(key : String) -> String { key }
    #|#derive.tool_middleware("adapter", presented="PublicFiles", expected="Storage")
    #|struct Adapter {}
    #|pub async fn Adapter::read(underlying : StorageUnderlying, path : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
  let middlewares = @lib.parse_tool_middlewares([("adapter.mbt", source)])
  let scope = monomorphic_scope(middlewares[0])
  debug_inspect(
    scope.handlers.map(handler => handler.handler_name),
    content="[\"read\"]",
  )
  debug_inspect(
    scope.underlying_methods.map(handler => handler.handler_name),
    content="[\"fetch\"]",
  )
}

///|
test "nested command paths flatten source segments and share inherited identities" {
  let source =
    #|#derive.tool("root")
    #|struct Root {}
    #|#derive.command(subtree="Child")
    #|#derive.arg("verbose", scope="global", alias="v")
    #|pub fn Root::admin(verbose : Bool) -> Unit { () }
    #|#derive.tool("child")
    #|struct Child {}
    #|#derive.arg("verbosity", scope="global", name="verbosity", alias="verbose")
    #|pub fn Child::child(verbosity : Bool) -> Unit { () }
    #|pub fn Child::run(name : String) -> String { name }
    #|#derive.tool_middleware("nested", presented="Root")
    #|struct Nested {}
    #|pub async fn Nested::admin(underlying : RootUnderlying, verbose : Bool) -> Result[Unit, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Nested::admin__run(underlying : RootUnderlying, verbose : Bool, name : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
  let scope = monomorphic_scope(
    @lib.parse_tool_middlewares([("nested.mbt", source)])[0],
  )
  let root = middleware_handler(scope.handlers, "admin")
  debug_inspect(root.command_path, content="[\"admin\"]")
  debug_inspect(root.params, content="[(\"verbose\", Simple(\"Bool\"))]")
  let leaf = middleware_handler(scope.handlers, "admin__run")
  debug_inspect(leaf.command_path, content="[\"admin\", \"run\"]")
  debug_inspect(
    leaf.params,
    content=(
      #|[("verbose", Simple("Bool")), ("name", Simple("String"))]
    ),
  )
  debug_inspect(
    leaf.param_sources,
    content=(
      #|[CanonicalInput("verbose"), CanonicalInput("name")]
    ),
  )
}

///|
test "root globals flow to sibling and nested middleware surfaces" {
  let source =
    #|#derive.tool("root")
    #|struct Root {}
    #|#derive.arg("profile", scope="global")
    #|pub fn Root::root(profile : String, local : String) -> Unit { () }
    #|pub fn Root::run(name : String) -> String { name }
    #|#derive.command(subtree="Child")
    #|#derive.arg("verbose", scope="global", alias="v")
    #|pub fn Root::admin(verbose : Bool) -> Unit { () }
    #|#derive.tool("child")
    #|struct Child {}
    #|#derive.arg("verbosity", scope="global", alias="verbose")
    #|#derive.arg("region", scope="global")
    #|pub fn Child::child(verbosity : Bool, region : String) -> Unit { () }
    #|pub fn Child::run(name : String) -> String { name }
    #|#derive.tool_middleware("nested", presented="Root")
    #|struct Nested {}
    #|pub async fn Nested::root(underlying : RootUnderlying, profile : String, local : String) -> Result[Unit, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Nested::run(underlying : RootUnderlying, profile : String, name : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Nested::admin(underlying : RootUnderlying, profile : String, verbose : Bool, region : String) -> Result[Unit, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
    #|pub async fn Nested::admin__run(underlying : RootUnderlying, profile : String, verbose : Bool, region : String, name : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
  let scope = monomorphic_scope(
    @lib.parse_tool_middlewares([("root-globals.mbt", source)])[0],
  )
  let expected : Array[(String, Array[(String, @lib.ParamType)])] = [
    (
      "root",
      [
        ("profile", @lib.ParamType::Simple("String")),
        ("local", @lib.ParamType::Simple("String")),
      ],
    ),
    (
      "run",
      [
        ("profile", @lib.ParamType::Simple("String")),
        ("name", @lib.ParamType::Simple("String")),
      ],
    ),
    (
      "admin",
      [
        ("profile", @lib.ParamType::Simple("String")),
        ("verbose", @lib.ParamType::Simple("Bool")),
        ("region", @lib.ParamType::Simple("String")),
      ],
    ),
    (
      "admin__run",
      [
        ("profile", @lib.ParamType::Simple("String")),
        ("verbose", @lib.ParamType::Simple("Bool")),
        ("region", @lib.ParamType::Simple("String")),
        ("name", @lib.ParamType::Simple("String")),
      ],
    ),
  ]
  for pair in expected {
    assert_true(middleware_handler(scope.handlers, pair.0).params == pair.1)
    assert_true(
      middleware_handler(scope.underlying_methods, pair.0).params == pair.1,
    )
  }
}

///|
test "universal authoring locks one grouped raw async signature" {
  let source =
    #|/// Audits an invocation without reinterpreting its carriers.
    #|#derive.universal_tool_middleware("audit", alias="trace")
    #|pub async fn audit(invocation : @toolMiddleware.RawToolInvocation, underlying : @toolMiddleware.UnderlyingTool) -> Result[@toolMiddleware.RawInvocationResult, @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue]] { ... }
  let middlewares = @lib.parse_tool_middlewares([("universal.mbt", source)])
  guard middlewares is [middleware] else {
    fail("expected one universal middleware")
  }
  assert_eq(middleware.name, "audit")
  debug_inspect(middleware.aliases, content="[\"trace\"]")
  assert_eq(
    middleware.description,
    "Audits an invocation without reinterpreting its carriers.",
  )
  match middleware.scope {
    @lib.ToolMiddlewareScopeInfo::Universal(info) =>
      assert_eq(info.handler_name, "audit")
    _ => fail("expected universal middleware")
  }
}

///|
test "checked-in authoring sources parse to the locked middleware contract" {
  let fixtures : Array[(String, String)] = [
    ("adapter.mbt", "file-adapter"),
    ("combined.mbt", "echo-policy"),
    ("retry.mbt", "retry-once"),
    ("short_circuit.mbt", "deny-policy"),
    ("transparent.mbt", "path-policy"),
    ("universal.mbt", "audit"),
  ]
  for fixture in fixtures {
    let path = "test-data/tool_middleware_authoring/" + fixture.0
    let middlewares = @lib.parse_tool_middlewares([
      (path, @fs.read_file_to_string(path)),
    ]) catch {
      @lib.ToolMiddlewareParseError(message) => {
        fail("failed to parse " + path + ": " + message)
        []
      }
      _ => {
        fail("failed to parse " + path + " with an unexpected error")
        []
      }
    }
    guard middlewares is [middleware] else {
      fail("expected one middleware in " + path)
    }
    assert_eq(middleware.name, fixture.1)
  }

  let authoring_path = "../golem_sdk/tool-middleware-authoring-test/authoring.mbt"
  let middlewares = @lib.parse_tool_middlewares([
    (authoring_path, @fs.read_file_to_string(authoring_path)),
  ]) catch {
    @lib.ToolMiddlewareParseError(message) => {
      fail("failed to parse " + authoring_path + ": " + message)
      []
    }
    _ => {
      fail("failed to parse " + authoring_path + " with an unexpected error")
      []
    }
  }
  assert_eq(middlewares.length(), 3)
  let policy = middlewares
    .iter()
    .find_first(middleware => middleware.name == "policy")
    .unwrap()
  let policy_scope = monomorphic_scope(policy)
  debug_inspect(
    policy_scope.handlers.map(handler => handler.handler_name),
    content=(
      #|[
      #|  "unit",
      #|  "value",
      #|  "retry",
      #|  "stdout",
      #|  "abandon_stdout",
      #|  "value_stdout",
      #|  "io",
      #|  "admin__run",
      #|]
    ),
  )
  debug_inspect(
    policy_scope.underlying_methods.map(handler => handler.handler_name),
    content=(
      #|[
      #|  "unit",
      #|  "value",
      #|  "retry",
      #|  "stdout",
      #|  "abandon_stdout",
      #|  "value_stdout",
      #|  "io",
      #|  "admin__run",
      #|]
    ),
  )
  debug_inspect(
    middleware_handler(policy_scope.handlers, "io").params,
    content=(
      #|[
      #|  ("input", Simple("@toolMiddleware.MiddlewareStdin")),
      #|  ("principal", Simple("@tool.Principal")),
      #|]
    ),
  )
  debug_inspect(
    middleware_handler(policy_scope.underlying_methods, "io").params,
    content="[(\"input\", Simple(\"@toolMiddleware.MiddlewareStdin\"))]",
  )
  let adapter = middlewares
    .iter()
    .find_first(middleware => middleware.name == "adapter")
    .unwrap()
  let adapter_scope = monomorphic_scope(adapter)
  assert_eq(adapter_scope.handlers.length(), 8)
  debug_inspect(
    adapter_scope.underlying_methods.map(handler => handler.handler_name),
    content="[\"fetch\"]",
  )
  let audit = middlewares
    .iter()
    .find_first(middleware => middleware.name == "audit")
    .unwrap()
  assert_true(audit.scope is @lib.ToolMiddlewareScopeInfo::Universal(_))
}

///|
fn simple_middleware(handler : String) -> String {
  (
    #|#derive.tool("runner")
    #|struct Runner {}
    #|pub fn Runner::run(value : String) -> String { value }
    #|#derive.tool_middleware("policy", presented="Runner")
    #|struct Policy {}
    #|
  ) +
  handler
}

///|
test "monomorphic declarations reject invalid metadata, placement, and coverage" {
  let cases : Array[(String, String)] = [
    (
      "#derive.tool_middleware(\"bad\", presented=\"Runner\")\nstruct Bad { value : Int }\n#derive.tool\nstruct Runner {}\n",
      "must be empty",
    ),
    ("#derive.tool_middleware(\"bad\")\nstruct Bad {}\n", "requires presented"),
    (
      "#derive.tool_middleware(\"bad\", presented=\"Missing\")\nstruct Bad {}\n",
      "same-package tool shapes only",
    ),
    (
      "#derive.tool_middleware(\"bad\", presented=\"@other.Tool\")\nstruct Bad {}\n",
      "same-package tool shapes only",
    ),
    (simple_middleware(""), "is missing async handler"),
    (
      simple_middleware(
        "pub async fn Policy::run(underlying : RunnerUnderlying, value : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }\npub fn Policy::extra() -> Unit { () }\n",
      ),
      "unexpected public handler",
    ),
    (
      simple_middleware(
        "pub fn Policy::run(underlying : RunnerUnderlying, value : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }\n",
      ),
      "must be async",
    ),
    (
      simple_middleware(
        "async fn Policy::run(underlying : RunnerUnderlying, value : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }\n",
      ),
      "must be public",
    ),
    (
      simple_middleware(
        "pub async fn Policy::run(self : Self, underlying : RunnerUnderlying, value : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }\n",
      ),
      "must be static",
    ),
    (
      simple_middleware(
        "pub async fn Policy::run(underlying : WrongUnderlying, value : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }\n",
      ),
      "parameter 1",
    ),
    (
      simple_middleware(
        "pub async fn Policy::run(underlying : RunnerUnderlying, value : Int) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }\n",
      ),
      "parameter 2",
    ),
    (
      simple_middleware(
        "pub async fn Policy::run(underlying : RunnerUnderlying, value : String) -> Result[Int, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }\n",
      ),
      "middleware result projection",
    ),
    (
      "#derive.tool_middleware(\"bad\", presented=\"Runner\")\n#derive.tool_middleware(\"other\", presented=\"Runner\")\nstruct Bad {}\n#derive.tool\nstruct Runner {}\n",
      "more than one middleware annotation",
    ),
  ]
  for case in cases {
    let message = middleware_parse_error(case.0)
    if !message.contains(case.1) {
      fail("expected '\{message}' to contain '\{case.1}'")
    }
  }
}

///|
test "monomorphic declarations reject qualified subtree and handler owners" {
  let qualified_subtree =
    #|#derive.tool("root")
    #|struct Root {}
    #|#derive.command(subtree="@other.Child")
    #|pub fn Root::child() -> Unit { () }
    #|#derive.tool_middleware("policy", presented="Root")
    #|struct Policy {}
  let qualified_subtree_error = middleware_parse_error(qualified_subtree)
  if !qualified_subtree_error.contains("same-package tool shapes only") {
    fail("unexpected qualified subtree error: " + qualified_subtree_error)
  }
  let qualified_subtree_with_namesake = qualified_subtree +
    "\n" +
    (
      #|#derive.tool("child")
      #|struct Child {}
      #|pub fn Child::run() -> Unit { () }
    )
  let namesake_error = middleware_parse_error(qualified_subtree_with_namesake)
  if !namesake_error.contains("same-package tool shapes only") {
    fail("unexpected qualified subtree namesake error: " + namesake_error)
  }
  let qualified_handler =
    #|#derive.tool("runner")
    #|struct Runner {}
    #|pub fn Runner::run(value : String) -> String { value }
    #|#derive.tool_middleware("policy", presented="Runner")
    #|struct Policy {}
    #|pub async fn @other.Policy::run(underlying : RunnerUnderlying, value : String) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] { ... }
  let qualified_handler_error = middleware_parse_error(qualified_handler)
  if !qualified_handler_error.contains("same-package middleware handlers only") {
    fail("unexpected qualified handler error: " + qualified_handler_error)
  }
}

///|
test "flattened handler collisions and subtree cycles are rejected" {
  let collision =
    #|#derive.tool("root")
    #|struct Root {}
    #|pub fn Root::foo__bar() -> Unit { () }
    #|#derive.command(name="foo", subtree="Child")
    #|pub fn Root::foo() -> Unit { () }
    #|#derive.tool("child")
    #|struct Child {}
    #|pub fn Child::bar() -> Unit { () }
    #|#derive.tool_middleware("collision", presented="Root")
    #|struct Collision {}
  assert_true(middleware_parse_error(collision).contains("collides"))
  let cycle =
    #|#derive.tool("a")
    #|struct A {}
    #|#derive.command(subtree="B")
    #|pub fn A::b() -> Unit { () }
    #|#derive.tool("b")
    #|struct B {}
    #|#derive.command(subtree="A")
    #|pub fn B::a() -> Unit { () }
    #|#derive.tool_middleware("cycle", presented="A")
    #|struct Cycle {}
  assert_true(middleware_parse_error(cycle).contains("cycle"))
}

///|
test "universal middleware rejects invalid placement and signatures" {
  let valid_return = "Result[@toolMiddleware.RawInvocationResult, @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue]]"
  let valid_params = "invocation : @toolMiddleware.RawToolInvocation, underlying : @toolMiddleware.UnderlyingTool"
  let cases : Array[(String, String)] = [
    (
      "#derive.universal_tool_middleware(\"audit\")\nstruct Audit {}\n", "async free function",
    ),
    (
      "#derive.universal_tool_middleware(\"audit\")\npub async fn Audit::run(\{valid_params}) -> \{valid_return} { ... }\n",
      "async free function",
    ),
    (
      "#derive.universal_tool_middleware(\"audit\")\npub fn audit(\{valid_params}) -> \{valid_return} { ... }\n",
      "must be async",
    ),
    (
      "#derive.universal_tool_middleware(\"audit\")\nasync fn audit(\{valid_params}) -> \{valid_return} { ... }\n",
      "must be public",
    ),
    (
      "#derive.universal_tool_middleware(\"audit\")\npub async fn audit(underlying : @toolMiddleware.UnderlyingTool) -> \{valid_return} { ... }\n",
      "accept exactly",
    ),
    (
      "#derive.universal_tool_middleware(\"audit\")\npub async fn audit(\{valid_params}) -> Result[Unit, Unit] { ... }\n",
      "must return",
    ),
    (
      "#derive.universal_tool_middleware(\"audit\")\n#derive.universal_tool_middleware(\"other\")\npub async fn audit(\{valid_params}) -> \{valid_return} { ... }\n",
      "more than one middleware annotation",
    ),
  ]
  for case in cases {
    let message = middleware_parse_error(case.0)
    if !message.contains(case.1) {
      fail("expected '\{message}' to contain '\{case.1}'")
    }
  }
}

///|
test "universal middleware rejects a qualified function owner" {
  let source =
    #|#derive.universal_tool_middleware("audit")
    #|pub async fn @other.Audit::run(
    #|  invocation : @toolMiddleware.RawToolInvocation,
    #|  underlying : @toolMiddleware.UnderlyingTool,
    #|) -> Result[@toolMiddleware.RawInvocationResult, @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue]] { ... }
  let message = middleware_parse_error(source)
  if !message.contains("async free function") {
    fail("unexpected qualified universal handler result: " + message)
  }
}

///|
test "universal middleware rejects annotation on a value declaration" {
  let source =
    #|#derive.universal_tool_middleware("audit")
    #|let audit = 1
  let message = middleware_parse_error(source)
  if !message.contains("async free function") {
    fail("unexpected universal value-declaration result: " + message)
  }
}

///|
test "canonical names and aliases form one collision domain" {
  let signature = "(invocation : @toolMiddleware.RawToolInvocation, underlying : @toolMiddleware.UnderlyingTool) -> Result[@toolMiddleware.RawInvocationResult, @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue]]"
  let source = "#derive.universal_tool_middleware(\"first\", alias=\"shared\")\npub async fn first\{signature} { ... }\n" +
    "#derive.universal_tool_middleware(\"shared\")\npub async fn second\{signature} { ... }\n"
  assert_true(middleware_parse_error(source).contains("collides"))
  let duplicate_alias = "#derive.universal_tool_middleware(\"first\", alias=\"same\", alias=\"same\")\npub async fn first\{signature} { ... }\n"
  assert_true(middleware_parse_error(duplicate_alias).contains("Duplicate"))
}
