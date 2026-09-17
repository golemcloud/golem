///|
pub type Principal = @agentCommon.Principal

///|
pub fn new_stdout() -> (@async-core.Stream[Byte], @async-core.Sink[Byte]) {
  @async-core.Stream::new()
}

///|
priv struct ToolState {
  tools : Map[String, @toolCommon.Tool]
  defs : Map[String, ToolDef]
}

///|
let state : ToolState = { tools: {}, defs: {}, }

///|
fn ensure_tool_name_available(name : String) -> Unit {
  if state.tools.contains(name) || state.defs.contains(name) {
    abort("duplicate tool registration for tool name: " + name)
  }
}

///|
pub fn register_tool_def(tool : ToolDef) -> Unit raise ToolBuildError {
  let wire = tool.to_tool()
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  register_wire_tool_inner(wire)
  state.defs[name] = tool
}

///|
pub fn register_subtool_def(tool : ToolDef) -> Unit raise ToolBuildError {
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  ensure_tool_name_available(name)
  state.defs[name] = tool
}

///|
pub fn register_wire_tool(tool : @toolCommon.Tool) -> Unit raise ToolBuildError {
  register_wire_tool_inner(tool)
}

///|
fn register_wire_tool_inner(
  tool : @toolCommon.Tool,
) -> Unit raise ToolBuildError {
  let name = match tool_name(tool) {
    Ok(name) => name
    Err(e) => raise e
  }
  ensure_tool_name_available(name)
  state.tools[name] = tool
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
pub fn registered_tool_def(name : String) -> ToolDef? {
  state.defs.get(name)
}

///|
pub fn release_registered_subtool_input(
  fields : Array[CanonicalInputValue],
) -> Unit {
  release_canonical_input_values(fields, @model_host.resource_drops())
}

///|
pub fn reject_wire_invocation(
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  error : @toolCommon.ToolError,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  reject_wire_invocation_with_resource_drops(
    input,
    stdin,
    error,
    @model_host.resource_drops(),
  )
}

///|
fn reject_wire_invocation_with_resource_drops(
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  error : @toolCommon.ToolError,
  drops : @model.SchemaValueResourceDrops,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  release_wire_invocation_input(input, drops)
  drop_optional_stdin(stdin)
  Err(error)
}

///|
fn release_wire_invocation_input(
  input : @types.TypedSchemaValue,
  drops : @model.SchemaValueResourceDrops,
) -> Unit {
  try @model.typed_schema_value_from_wit(input, drops) catch {
    _ => ()
  } noraise {
    decoded => decoded.value.release_resources(drops)
  }
}

///|
pub fn reject_undecoded_invocation(
  stdin : @async-core.Stream[Byte]?,
  error : @toolCommon.ToolError,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  drop_optional_stdin(stdin)
  Err(error)
}

///|
pub fn reject_decoded_invocation(
  fields : Array[CanonicalInputValue],
  stdin : @async-core.Stream[Byte]?,
  error : @toolCommon.ToolError,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  reject_decoded_invocation_with_resource_drops(
    fields,
    stdin,
    error,
    @model_host.resource_drops(),
  )
}

///|
fn reject_decoded_invocation_with_resource_drops(
  fields : Array[CanonicalInputValue],
  stdin : @async-core.Stream[Byte]?,
  error : @toolCommon.ToolError,
  drops : @model.SchemaValueResourceDrops,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  release_canonical_input_values(fields, drops)
  drop_optional_stdin(stdin)
  Err(error)
}

///|
fn drop_optional_stdin(stdin : @async-core.Stream[Byte]?) -> Unit {
  match stdin {
    Some(stdin) => stdin.drop_sync()
    None => ()
  }
}
