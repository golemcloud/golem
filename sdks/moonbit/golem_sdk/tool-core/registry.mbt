///|
pub type Principal = @agentCommon.Principal

///|
pub fn new_stdout() -> (@async-core.Stream[Byte], @async-core.Sink[Byte]) {
  @async-core.Stream::new()
}

///|
priv struct ToolState {
  tools : Map[String, @toolCommon.Tool]
  subtools : Map[String, @toolCommon.Tool]
  defs : Map[String, ToolDef]
}

///|
let state : ToolState = { tools: {}, subtools: {}, defs: {}, }

///|
fn ensure_tool_name_available(name : String) -> Unit {
  if state.tools.contains(name) ||
    state.subtools.contains(name) ||
    state.defs.contains(name) {
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
pub fn register_wire_subtool(
  tool : @toolCommon.Tool,
) -> Unit raise ToolBuildError {
  let name = match tool_name(tool) {
    Ok(name) => name
    Err(error) => raise error
  }
  ensure_tool_name_available(name)
  state.subtools[name] = tool
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
) -> Result[@toolCommon.Tool, @types.ToolError] {
  match state.tools.get(name) {
    Some(tool) => Ok(tool)
    None => Err(@types.ToolError::InvalidToolName(name))
  }
}

///|
pub fn registered_tool_def(name : String) -> ToolDef? {
  match state.defs.get(name) {
    Some(def) => Some(def)
    None => {
      let wire = match state.tools.get(name) {
        Some(tool) => Some(tool)
        None => state.subtools.get(name)
      }
      wire.map(tool => {
        ToolDef::from_wire(tool) catch {
          error => abort(repr(error))
        }
      })
    }
  }
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
  error : @types.ToolError,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
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
  error : @types.ToolError,
  drops : @model.SchemaValueResourceDrops,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  release_wire_invocation_input(input, drops)
  drop_optional_stdin(stdin)
  Err(error)
}

///|
fn release_wire_invocation_input(
  input : @types.TypedSchemaValue,
  drops : @model.SchemaValueResourceDrops,
) -> Unit {
  @schema.WireReader::new(input.value, drop_resource=node => {
    match node {
      @types.SecretValue(raw) => (drops.drop_secret)(raw)
      @types.QuotaTokenHandle(raw) => (drops.drop_quota_token)(raw)
      @types.StreamValue(raw) => (drops.drop_stream)(raw)
      @types.PermissionCardHandle(raw) => raw.drop()
      _ => ()
    }
  }).abort()
}

///|
pub fn reject_undecoded_invocation(
  stdin : @async-core.Stream[Byte]?,
  error : @types.ToolError,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  drop_optional_stdin(stdin)
  Err(error)
}

///|
pub fn reject_decoded_invocation(
  fields : Array[CanonicalInputValue],
  stdin : @async-core.Stream[Byte]?,
  error : @types.ToolError,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
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
  error : @types.ToolError,
  drops : @model.SchemaValueResourceDrops,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
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
