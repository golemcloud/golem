///|
type ToolInvoker = (
  Array[String],
  @types.TypedSchemaValue,
  @streams.InputStream?,
  @agentCommon.Principal,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError]

///|
priv struct ToolState {
  tools : Map[String, @toolCommon.Tool]
  defs : Map[String, ToolDef]
  invokers : Map[String, ToolInvoker]
}

///|
let state : ToolState = { tools: {}, defs: {}, invokers: {} }

///|
pub fn register_tool_def(tool : ToolDef) -> Unit raise ToolBuildError {
  let wire = tool.to_tool()
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  register_wire_tool_inner(wire, None)
  state.defs[name] = tool
}

///|
pub fn register_tool_def_invoker(
  tool : ToolDef,
  invoker : ToolInvoker,
) -> Unit raise ToolBuildError {
  let wire = tool.to_tool()
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  register_wire_tool_inner(wire, Some(invoker))
  state.defs[name] = tool
}

///|
pub fn register_wire_tool(tool : @toolCommon.Tool) -> Unit raise ToolBuildError {
  register_wire_tool_inner(tool, None)
}

///|
pub fn register_wire_tool_invoker(
  tool : @toolCommon.Tool,
  invoker : ToolInvoker,
) -> Unit raise ToolBuildError {
  register_wire_tool_inner(tool, Some(invoker))
}

///|
fn register_wire_tool_inner(
  tool : @toolCommon.Tool,
  invoker : ToolInvoker?,
) -> Unit raise ToolBuildError {
  let name = match tool_name(tool) {
    Ok(name) => name
    Err(e) => raise e
  }
  if state.tools.get(name) is Some(_) {
    abort("duplicate tool registration for tool name: " + name)
  }
  state.tools[name] = tool
  if invoker is Some(invoker) {
    state.invokers[name] = invoker
  }
}

///|
pub fn discover_tools() -> Array[@toolCommon.Tool] {
  state.tools.to_array().map(fn(kv) { kv.1 })
}

///|
pub fn get_registered_tool(
  name : String,
) -> Result[@toolCommon.Tool, @toolCommon.ToolError] {
  match state.tools.get(name) {
    Some(tool) => Ok(tool)
    None => Err(@toolCommon.ToolError::InvalidToolName(name))
  }
}

///|
pub fn invoke_registered_tool(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @streams.InputStream?,
  principal : @agentCommon.Principal,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  match state.invokers.get(tool_name) {
    Some(invoker) => invoker(command_path, input, stdin, principal)
    None => Err(@toolCommon.ToolError::InvalidToolName(tool_name))
  }
}

///|
pub fn invoke_registered_subtool(
  tool_name : String,
  command_path : Array[String],
  fields : Array[CanonicalInputValue],
  stdin : @streams.InputStream?,
  principal : @agentCommon.Principal,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  let def = match state.defs.get(tool_name) {
    Some(def) => def
    None => return Err(@toolCommon.ToolError::InvalidToolName(tool_name))
  }
  let index = match def.command_index_by_path(command_path) {
    Some(index) => index
    None => return Err(@toolCommon.ToolError::InvalidCommandPath(command_path))
  }
  let input = forward_canonical_input(fields, def, index) catch {
    error => return Err(error)
  }
  invoke_registered_tool(tool_name, command_path, input, stdin, principal)
}
