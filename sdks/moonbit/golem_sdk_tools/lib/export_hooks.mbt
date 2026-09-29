///|
/// Connect only the runtimes used by this component to the single guest world.
/// Absent categories keep the SDK's empty/error handlers and do not root their
/// registries, dispatchers, or snapshot implementation in whole-program DCE.
pub fn generate_export_hooks(
  has_agents : Bool,
  has_tools : Bool,
  has_middlewares : Bool,
) -> (@list.List[@syntax.Impl], Array[(String, String)]) {
  let imports : Array[(String, String)] = []
  let body = StringBuilder()
  if has_agents {
    imports.push(("golemcloud/golem_sdk/agent-exports", "@agentExports"))
    imports.push(
      ("golemcloud/golem_sdk/gen/interface/golem/agent/guest", "@agentGuest"),
    )
    imports.push(
      (
        "golemcloud/golem_sdk/gen/interface/golem/api/save-snapshot", "@saveSnapshot",
      ),
    )
    imports.push(
      (
        "golemcloud/golem_sdk/gen/interface/golem/api/load-snapshot", "@loadSnapshot",
      ),
    )
    body.write_string(
      (
        #|@agentGuest.handlers.val = Some({
        #|  initialize: @agentExports.initialize,
        #|  invoke: @agentExports.invoke,
        #|  get_definition: @agentExports.get_definition,
        #|  discover_agent_types: @agentExports.discover_agent_types,
        #|})
        #|@saveSnapshot.handler.val = Some(@agentExports.save)
        #|@loadSnapshot.handler.val = Some(@agentExports.load)
        #|
      ),
    )
  }
  if has_tools {
    imports.push(("golemcloud/golem_sdk/tool-exports", "@toolExports"))
    imports.push(
      ("golemcloud/golem_sdk/gen/interface/golem/tool/guest", "@toolGuest"),
    )
    body.write_string(
      (
        #|@toolGuest.handlers.val = Some({
        #|  discover_tools: @toolExports.discover_tools,
        #|  get_tool: @toolExports.get_tool,
        #|  invoke: @toolExports.invoke,
        #|})
        #|
      ),
    )
  }
  if has_middlewares {
    imports.push(
      ("golemcloud/golem_sdk/tool-middleware-exports", "@middlewareExports"),
    )
    imports.push(
      (
        "golemcloud/golem_sdk/gen/interface/golem/tool/tool-middleware-guest", "@middlewareGuest",
      ),
    )
    body.write_string(
      (
        #|@middlewareGuest.handlers.val = Some({
        #|  discover_tool_middlewares: @middlewareExports.discover_tool_middlewares,
        #|  get_tool_middleware: @middlewareExports.get_tool_middleware,
        #|  invoke_tool_middleware: @middlewareExports.invoke_tool_middleware,
        #|})
        #|
      ),
    )
  }
  let impls = if imports.is_empty() {
    to_list([])
  } else {
    parse_generated_impls(
      "fn init {\n" + body.to_string() + "}\n",
      "export hooks",
    )
  }
  (impls, imports)
}
