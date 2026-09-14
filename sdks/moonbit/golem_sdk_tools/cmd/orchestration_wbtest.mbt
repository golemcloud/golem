///|
fn remove_fixture_tree(path : String) -> Unit raise {
  if !@fs.path_exists(path) {
    return
  }
  if @fs.is_dir(path) {
    for entry in @fs.read_dir(path) {
      remove_fixture_tree(path + "/" + entry)
    }
    @fs.remove_dir(path)
  } else {
    @fs.remove_file(path)
  }
}

///|
fn create_component_fixture(root : String, source : String) -> String raise {
  if !@fs.path_exists("_build") {
    @fs.create_dir("_build")
  }
  remove_fixture_tree(root)
  @fs.create_dir(root)
  let component = root + "/component"
  @fs.create_dir(component)
  @fs.write_string_to_file(
    component + "/moon.pkg",
    "options(\n  \"is-main\": true,\n)\n",
  )
  @fs.write_string_to_file(component + "/source.mbt", source)
  component
}

///|
fn generated_snapshot(
  component : String,
  files : Array[String],
) -> String raise {
  let parts : Array[String] = []
  for file in files {
    parts.push(file)
    parts.push(@fs.read_file_to_string(component + "/" + file))
  }
  parts.join("\n---\n")
}

///|
fn append_package_snapshot(
  root : String,
  relative_dir : String,
  parts : Array[String],
) -> Unit raise {
  let dir = if relative_dir == "" { root } else { root + "/" + relative_dir }
  let entries = @fs.read_dir(dir)
  entries.sort_by((left, right) => left.compare(right))
  for entry in entries {
    let relative_path = if relative_dir == "" {
      entry
    } else {
      relative_dir + "/" + entry
    }
    let path = root + "/" + relative_path
    if @fs.is_dir(path) {
      append_package_snapshot(root, relative_path, parts)
    } else {
      parts.push(relative_path)
      parts.push(@fs.read_file_to_string(path))
    }
  }
}

///|
fn package_snapshot(component : String) -> String raise {
  let parts : Array[String] = []
  append_package_snapshot(component, "", parts)
  parts.join("\n---\n")
}

///|
fn agents_role_error(
  project_root : String,
  component_dir : String,
  role : @lib.ComponentRole,
) -> String {
  try run_agents(project_root, component_dir, role) catch {
    @lib.ComponentRoleError(message) => message
    error => "Unexpected error: \{error}"
  } noraise {
    _ => "NO ERROR"
  }
}

///|
fn reexports_role_error(
  sdk_path : String,
  target_dir : String,
  role : @lib.ComponentRole,
) -> String {
  try run_reexports(sdk_path, target_dir, role) catch {
    @lib.ComponentRoleError(message) => message
    error => "Unexpected error: \{error}"
  } noraise {
    _ => "NO ERROR"
  }
}

///|
test "pure middleware generation is targeted, idempotent, and cleans stale output" {
  let root = "_build/gol34-phase8-pure"
  let component = create_component_fixture(
    root,
    (
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
      #|
      #|#derive.tool_middleware("policy", presented="Echo")
      #|struct Policy {}
      #|
      #|pub async fn Policy::echo(
      #|  underlying : EchoUnderlying,
      #|  value : String,
      #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
      #|  underlying.echo(value)
      #|}
    ),
  )
  run_agents(root, "component", @lib.ComponentRole::ToolMiddleware)
  assert_true(@fs.path_exists(component + "/golem_tool_middlewares.mbt"))
  assert_false(@fs.path_exists(component + "/golem_tools.mbt"))
  assert_false(@fs.path_exists(component + "/golem_tool_clients.mbt"))
  assert_false(@fs.path_exists(component + "/golem_agents.mbt"))
  let pkg = @fs.read_file_to_string(component + "/moon.pkg")
  assert_true(pkg.contains("\"golemcloud/golem_sdk/tool-core\" @tool,"))
  assert_false(pkg.contains("\"golemcloud/golem_sdk/tool\" @tool,"))
  assert_false(pkg.contains("golem:tool/host"))
  let generated = @fs.read_file_to_string(
    component + "/golem_tool_middlewares.mbt",
  )
  assert_eq(
    generated.split("@toolMiddleware.register_tool_middleware(").count() - 1,
    1,
  )
  assert_false(generated.contains("register_tool_def_invoker"))
  assert_false(generated.contains("ToolClient"))
  let before = generated_snapshot(component, [
    "moon.pkg", ".golem-sdk-role", "golem_tool_middlewares.mbt",
  ])
  run_agents(root, "component", @lib.ComponentRole::ToolMiddleware)
  let after = generated_snapshot(component, [
    "moon.pkg", ".golem-sdk-role", "golem_tool_middlewares.mbt",
  ])
  assert_eq(after, before)
  @fs.write_string_to_file(
    component + "/source.mbt",
    (
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
    ),
  )
  run_agents(root, "component", @lib.ComponentRole::ToolMiddleware)
  assert_false(@fs.path_exists(component + "/golem_tool_middlewares.mbt"))
  let cleaned_pkg = @fs.read_file_to_string(component + "/moon.pkg")
  assert_false(cleaned_pkg.contains("golemcloud/golem_sdk/"))
  remove_fixture_tree(root)
}

///|
test "universal pure generation adds only its runtime import" {
  let root = "_build/gol34-phase8-universal"
  let component = create_component_fixture(
    root,
    (
      #|#derive.universal_tool_middleware("audit")
      #|pub async fn audit(
      #|  invocation : @toolMiddleware.RawToolInvocation,
      #|  underlying : @toolMiddleware.UnderlyingTool,
      #|) -> Result[
      #|  @toolMiddleware.RawInvocationResult,
      #|  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],
      #|] {
      #|  underlying.invoke_raw(
      #|    invocation.command_path(),
      #|    invocation.input(),
      #|    invocation.stdin(),
      #|  )
      #|}
    ),
  )
  run_agents(root, "component", @lib.ComponentRole::ToolMiddleware)
  let pkg = @fs.read_file_to_string(component + "/moon.pkg")
  assert_true(
    pkg.contains("\"golemcloud/golem_sdk/tool-middleware\" @toolMiddleware,"),
  )
  assert_false(pkg.contains("\"golemcloud/golem_sdk/tool-core\""))
  assert_false(pkg.contains("\"golemcloud/golem_sdk/tool\""))
  assert_false(pkg.contains("golemcloud/golem_sdk/schema"))
  assert_false(pkg.contains("golemcloud/golem_sdk/interface/golem/tool"))
  remove_fixture_tree(root)
}

///|
test "ordinary and combined roles generate exactly their selected surfaces" {
  let ordinary_root = "_build/gol34-phase8-ordinary"
  let ordinary = create_component_fixture(
    ordinary_root,
    (
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
    ),
  )
  run_agents(ordinary_root, "component", @lib.ComponentRole::Ordinary)
  assert_true(@fs.path_exists(ordinary + "/golem_tools.mbt"))
  assert_true(@fs.path_exists(ordinary + "/golem_tool_clients.mbt"))
  assert_false(@fs.path_exists(ordinary + "/golem_tool_middlewares.mbt"))
  assert_false(
    @fs.read_file_to_string(ordinary + "/moon.pkg").contains(
      "golem_sdk/tool-middleware",
    ),
  )
  let combined_root = "_build/gol34-phase8-combined"
  let combined = create_component_fixture(
    combined_root,
    (
      #|#derive.agent
      #|struct CombinedAgent {}
      #|
      #|fn CombinedAgent::new() -> CombinedAgent {
      #|  {}
      #|}
      #|
      #|pub fn CombinedAgent::status(self : Self) -> String {
      #|  ignore(self)
      #|  "ready"
      #|}
      #|
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
      #|
      #|#derive.tool_middleware("policy", presented="Echo")
      #|struct Policy {}
      #|
      #|pub async fn Policy::echo(
      #|  underlying : EchoUnderlying,
      #|  value : String,
      #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
      #|  underlying.echo(value)
      #|}
    ),
  )
  run_agents(combined_root, "component", @lib.ComponentRole::Combined)
  let combined_files = [
    "moon.pkg", ".golem-sdk-role", "golem_agents.mbt", "golem_clients.mbt", "golem_tools.mbt",
    "golem_tool_clients.mbt", "golem_tool_middlewares.mbt",
  ]
  for file in combined_files {
    assert_true(@fs.path_exists(combined + "/" + file))
  }
  let agents = @fs.read_file_to_string(combined + "/golem_agents.mbt")
  let tools = @fs.read_file_to_string(combined + "/golem_tools.mbt")
  let middlewares = @fs.read_file_to_string(
    combined + "/golem_tool_middlewares.mbt",
  )
  assert_eq(agents.split("fn init {").count() - 1, 1)
  assert_eq(tools.split("fn init {").count() - 1, 1)
  assert_eq(middlewares.split("fn init {").count() - 1, 1)
  assert_eq(
    middlewares.split("@toolMiddleware.register_tool_middleware(").count() - 1,
    1,
  )
  let before = generated_snapshot(combined, combined_files)
  run_agents(combined_root, "component", @lib.ComponentRole::Combined)
  assert_eq(generated_snapshot(combined, combined_files), before)
  remove_fixture_tree(ordinary_root)
  remove_fixture_tree(combined_root)
}

///|
test "middleware generation never overwrites or removes a user-owned managed path" {
  let overwrite_root = "_build/gol34-phase8-owned-overwrite"
  let overwrite_component = create_component_fixture(
    overwrite_root,
    (
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
      #|
      #|#derive.tool_middleware("policy", presented="Echo")
      #|struct Policy {}
      #|
      #|pub async fn Policy::echo(
      #|  underlying : EchoUnderlying,
      #|  value : String,
      #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
      #|  underlying.echo(value)
      #|}
    ),
  )
  let user_source = "// user-owned despite the reserved filename\npub fn keep() -> Unit {}\n"
  let overwrite_path = overwrite_component + "/golem_tool_middlewares.mbt"
  @fs.write_string_to_file(overwrite_path, user_source)
  assert_true(
    agents_role_error(
      overwrite_root,
      "component",
      @lib.ComponentRole::ToolMiddleware,
    ).contains("Refusing to overwrite non-generated file"),
  )
  assert_eq(@fs.read_file_to_string(overwrite_path), user_source)
  let removal_root = "_build/gol34-phase8-owned-removal"
  let removal_component = create_component_fixture(
    removal_root,
    (
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
    ),
  )
  let removal_path = removal_component + "/golem_tool_middlewares.mbt"
  @fs.write_string_to_file(removal_path, user_source)
  assert_true(
    agents_role_error(
      removal_root,
      "component",
      @lib.ComponentRole::ToolMiddleware,
    ).contains("Refusing to remove non-generated file"),
  )
  assert_eq(@fs.read_file_to_string(removal_path), user_source)
  remove_fixture_tree(overwrite_root)
  remove_fixture_tree(removal_root)
}

///|
test "middleware generation is independent of source-file creation order" {
  let first_root = "_build/gol34-phase8-order-first"
  let second_root = "_build/gol34-phase8-order-second"
  let first = create_component_fixture(first_root, "")
  let second = create_component_fixture(second_root, "")
  let alpha =
    #|#derive.tool("alpha")
    #|struct Alpha {}
    #|
    #|pub fn Alpha::run(value : String) -> String {
    #|  value
    #|}
    #|
    #|#derive.tool_middleware("alpha-policy", presented="Alpha")
    #|struct AlphaPolicy {}
    #|
    #|pub async fn AlphaPolicy::run(
    #|  underlying : AlphaUnderlying,
    #|  value : String,
    #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
    #|  underlying.run(value)
    #|}
  let beta =
    #|#derive.tool("beta")
    #|struct Beta {}
    #|
    #|pub fn Beta::run(value : String) -> String {
    #|  value
    #|}
    #|
    #|#derive.tool_middleware("beta-policy", presented="Beta")
    #|struct BetaPolicy {}
    #|
    #|pub async fn BetaPolicy::run(
    #|  underlying : BetaUnderlying,
    #|  value : String,
    #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
    #|  underlying.run(value)
    #|}
  @fs.write_string_to_file(first + "/alpha.mbt", alpha)
  @fs.write_string_to_file(first + "/beta.mbt", beta)
  @fs.write_string_to_file(second + "/beta.mbt", beta)
  @fs.write_string_to_file(second + "/alpha.mbt", alpha)
  run_agents(first_root, "component", @lib.ComponentRole::ToolMiddleware)
  run_agents(second_root, "component", @lib.ComponentRole::ToolMiddleware)
  assert_eq(
    generated_snapshot(first, ["moon.pkg", "golem_tool_middlewares.mbt"]),
    generated_snapshot(second, ["moon.pkg", "golem_tool_middlewares.mbt"]),
  )
  remove_fixture_tree(first_root)
  remove_fixture_tree(second_root)
}

///|
test "pure reexports rejects ambient user imports before changing generated state" {
  let root = "_build/gol34-phase8-reexports-ambient"
  let component = create_component_fixture(root, "")
  let pkg =
    #|import {
    #|  "golemcloud/golem_sdk/tool" @customTool,
    #|}
    #|
    #|options(
    #|  "is-main": true,
    #|)
  let reexports = generated_file_banner + "\n\npub fn existing() -> Unit {}\n"
  @fs.write_string_to_file(component + "/moon.pkg", pkg)
  @fs.write_string_to_file(component + "/golem_reexports.mbt", reexports)
  let error = reexports_role_error(
    root + "/missing-sdk",
    component,
    @lib.ComponentRole::ToolMiddleware,
  )
  assert_true(error.contains("must not import ambient package"))
  assert_eq(@fs.read_file_to_string(component + "/moon.pkg"), pkg)
  assert_eq(
    @fs.read_file_to_string(component + "/golem_reexports.mbt"),
    reexports,
  )
  assert_false(@fs.path_exists(component + "/.golem-sdk-role"))
  remove_fixture_tree(root)
}

///|
test "agents ambient-import failure leaves the whole package unchanged" {
  let root = "_build/gol34-phase11-agents-ambient"
  let component = create_component_fixture(
    root,
    (
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
      #|
      #|#derive.tool_middleware("policy", presented="Echo")
      #|struct Policy {}
      #|
      #|pub async fn Policy::echo(
      #|  underlying : EchoUnderlying,
      #|  value : String,
      #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
      #|  underlying.echo(value)
      #|}
    ),
  )
  @fs.write_string_to_file(
    component + "/moon.pkg",
    (
      #|import {
      #|  "golemcloud/golem_sdk/tool" @customTool,
      #|}
      #|
      #|options(
      #|  "is-main": true,
      #|)
    ),
  )
  @fs.write_string_to_file(
    component + "/golem_tool_middlewares.mbt",
    generated_file_banner + "\n\npub fn previous_middleware() -> Unit {}\n",
  )
  @fs.write_string_to_file(
    component + "/golem_tools.mbt",
    generated_file_banner + "\n\npub fn stale_tool() -> Unit {}\n",
  )
  let before = package_snapshot(component)
  let error = agents_role_error(
    root,
    "component",
    @lib.ComponentRole::ToolMiddleware,
  )
  assert_true(error.contains("must not import ambient package"))
  assert_eq(package_snapshot(component), before)
  assert_false(@fs.path_exists(component + "/.golem-sdk-role"))
  remove_fixture_tree(root)
}

///|
test "agents stale ownership failure leaves the whole package unchanged" {
  let root = "_build/gol34-phase11-agents-stale"
  let component = create_component_fixture(
    root,
    (
      #|#derive.tool("echo")
      #|struct Echo {}
      #|
      #|pub fn Echo::echo(value : String) -> String {
      #|  value
      #|}
      #|
      #|#derive.tool_middleware("policy", presented="Echo")
      #|struct Policy {}
      #|
      #|pub async fn Policy::echo(
      #|  underlying : EchoUnderlying,
      #|  value : String,
      #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
      #|  underlying.echo(value)
      #|}
    ),
  )
  @fs.write_string_to_file(
    component + "/golem_tool_middlewares.mbt",
    generated_file_banner + "\n\npub fn previous_middleware() -> Unit {}\n",
  )
  @fs.write_string_to_file(
    component + "/golem_clients.mbt",
    "// user-owned despite the reserved filename\npub fn keep() -> Unit {}\n",
  )
  let before = package_snapshot(component)
  let error = agents_role_error(
    root,
    "component",
    @lib.ComponentRole::ToolMiddleware,
  )
  assert_true(error.contains("Refusing to remove non-generated file"))
  assert_eq(package_snapshot(component), before)
  assert_false(@fs.path_exists(component + "/.golem-sdk-role"))
  remove_fixture_tree(root)
}

///|
/// `moon` is mtime-driven, so rewriting a generated file with identical bytes
/// would rebuild the package and invalidate every later build step. These tests
/// pin the write decision, since MoonBit has no mtime API to assert on.
test "write_generated_file skips a byte-identical rewrite" {
  let root = "_build/content-stable-write"
  let component = create_component_fixture(root, "")
  let path = component + "/golem_reexports.mbt"
  let content = generated_file_banner + "\n\npub fn a() -> Unit {}\n"
  assert_true(write_generated_file(path, content))
  assert_false(write_generated_file(path, content))
  assert_true(write_generated_file(path, content + "\npub fn b() -> Unit {}\n"))
  assert_false(
    write_generated_file(path, content + "\npub fn b() -> Unit {}\n"),
  )
  remove_fixture_tree(root)
}

///|
test "a generated file plan rewrites only what changed, and still removes stale files" {
  let root = "_build/content-stable-plan"
  let component = create_component_fixture(root, "")
  let agents_path = component + "/golem_agents.mbt"
  let stale_path = component + "/golem_tools.mbt"
  let content = generated_file_banner + "\n\npub fn init_agents() -> Unit {}\n"

  // First application writes the file.
  let first = GeneratedFilePlan::new()
  first.stage_write(agents_path, content, "Generated \{agents_path}")
  first.preflight()
  assert_eq(first.apply(), 1)

  // Re-applying identical content writes nothing.
  let second = GeneratedFilePlan::new()
  second.stage_write(agents_path, content, "Generated \{agents_path}")
  second.preflight()
  assert_eq(second.apply(), 0)

  // Changed content is written again.
  let changed = content + "\npub fn extra() -> Unit {}\n"
  let third = GeneratedFilePlan::new()
  third.stage_write(agents_path, changed, "Generated \{agents_path}")
  third.preflight()
  assert_eq(third.apply(), 1)
  assert_eq(@fs.read_file_to_string(agents_path), changed)

  // Skipping an unchanged write must not suppress a staged removal.
  @fs.write_string_to_file(
    stale_path,
    generated_file_banner + "\n\npub fn stale() -> Unit {}\n",
  )
  let fourth = GeneratedFilePlan::new()
  fourth.stage_write(agents_path, changed, "Generated \{agents_path}")
  fourth.stage_removal(stale_path)
  fourth.preflight()
  assert_eq(fourth.apply(), 0)
  assert_false(@fs.path_exists(stale_path))
  assert_true(@fs.path_exists(agents_path))
  remove_fixture_tree(root)
}
