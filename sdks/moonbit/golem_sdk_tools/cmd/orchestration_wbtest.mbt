///|
fn remove_default_world_fixture(path : String) -> Unit raise {
  if !@fs.path_exists(path) {
    return
  }
  if @fs.is_dir(path) {
    for entry in @fs.read_dir(path) {
      remove_default_world_fixture(path + "/" + entry)
    }
    @fs.remove_dir(path)
  } else {
    @fs.remove_file(path)
  }
}

///|
test "default generation supports agents tools and middleware together" {
  let root = "_build/default-world-generation"
  remove_default_world_fixture(root)
  @fs.create_dir(root)
  @fs.create_dir(root + "/component")
  @fs.write_string_to_file(root + "/moon.mod", "name = \"test/generation\"\n")
  let packages = @lib.discover_packages(root + "/component")
  assert_eq(packages.length(), 0)
  @fs.write_string_to_file(
    root + "/component/moon.pkg",
    "pkgtype(kind: \"foreign_library\")\noptions(\n)\n",
  )
  let packages = @lib.discover_packages(root + "/component")
  assert_eq(packages.length(), 1)
  assert_eq(packages[0].rel_path, "component")
  @fs.write_string_to_file(
    root + "/component/source.mbt",
    (
      #|#derive.agent
      #|struct DefaultAgent {}
      #|fn DefaultAgent::new() -> DefaultAgent { {} }
      #|pub fn DefaultAgent::status(self : Self) -> String { "ready" }
      #|
      #|#derive.tool("echo")
      #|struct Echo {}
      #|pub fn Echo::echo(value : String) -> String { value }
      #|
      #|#derive.tool_middleware("policy", presented="Echo")
      #|struct Policy {}
      #|pub async fn Policy::echo(
      #|  underlying : EchoUnderlying,
      #|  value : String,
      #|) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
      #|  underlying.echo(value)
      #|}
    ),
  )
  run_agents(root, "component")
  for
    file in [
      "golem_agents.mbt", "golem_tools.mbt", "golem_tool_clients.mbt", "golem_tool_middlewares.mbt",
    ] {
    assert_true(@fs.path_exists(root + "/component/" + file))
  }
  let pkg = @fs.read_file_to_string(root + "/component/moon.pkg")
  assert_true(pkg.contains("golemcloud/golem_sdk/tool"))
  assert_true(pkg.contains("golemcloud/golem_sdk/tool-middleware"))
  remove_default_world_fixture(root)
}
