///|
pub type Principal = @agentCommon.Principal

///|
priv struct ProviderStdoutState {
  writer : @toolHost.ToolStdoutWriter
  mut open : Bool
  mut busy : Bool
  mut dropped : Bool
}

///|
pub struct ProviderStdout {
  priv state : Ref[ProviderStdoutState]
  priv output : @async-core.Sink[Byte]
}

///|
pub fn provider_stdout(writer : @toolHost.ToolStdoutWriter) -> ProviderStdout {
  let state = @ref.new({ writer, open: true, busy: false, dropped: false })
  let output = @async-core.Sink::from_callbacks(
    async fn(bytes) {
      if !state.val.open || state.val.dropped {
        return 0
      }
      if state.val.busy {
        raise @toolHost.StreamWriteError::ConcurrentOperation
      }
      state.val.busy = true
      defer {
        state.val.busy = false
      }
      let owned = FixedArray::from_array(bytes)
      match state.val.writer.write(owned) {
        Ok(_) => bytes.length()
        Err(error) => {
          if error is @toolHost.StreamWriteError::Closed(_) {
            state.val.open = false
          }
          raise error
        }
      }
    },
    async fn() {
      if !state.val.open || state.val.dropped {
        return
      }
      if state.val.busy {
        raise @toolHost.StreamWriteError::ConcurrentOperation
      }
      state.val.busy = true
      defer {
        state.val.busy = false
      }
      match state.val.writer.finish() {
        Ok(_) => state.val.open = false
        Err(error) => {
          if error is @toolHost.StreamWriteError::Closed(_) {
            state.val.open = false
          }
          raise error
        }
      }
    },
    fn() { state.val.open && !state.val.dropped },
    None,
  )
  { state, output }
}

///|
pub fn ProviderStdout::output(self : ProviderStdout) -> @async-core.Sink[Byte] {
  self.output
}

///|
pub async fn ProviderStdout::finish(self : ProviderStdout) -> Unit {
  if !self.state.val.open || self.state.val.dropped {
    return
  }
  if self.state.val.busy {
    raise @toolHost.StreamWriteError::ConcurrentOperation
  }
  self.state.val.busy = true
  defer {
    self.state.val.busy = false
  }
  match self.state.val.writer.finish() {
    Ok(_) => self.state.val.open = false
    Err(@toolHost.StreamWriteError::Closed(_)) => self.state.val.open = false
    Err(error) => raise error
  }
}

///|
pub fn ProviderStdout::drop(self : ProviderStdout) -> Unit {
  if !self.state.val.dropped {
    self.state.val.dropped = true
    self.state.val.open = false
    self.state.val.writer.drop()
  }
}

///|
type ToolInvoker = async (
  Array[String],
  @types.TypedSchemaValue,
  @async-core.Stream[Byte]?,
  @async-core.Sink[Byte]?,
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
fn ensure_tool_name_available(name : String) -> Unit {
  if state.tools.contains(name) ||
    state.defs.contains(name) ||
    state.invokers.contains(name) {
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
pub fn register_subtool_def_invoker(
  tool : ToolDef,
  invoker : ToolInvoker,
) -> Unit raise ToolBuildError {
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  ensure_tool_name_available(name)
  state.defs[name] = tool
  state.invokers[name] = invoker
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
  ensure_tool_name_available(name)
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
pub async fn invoke_registered_tool(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  stdout : @async-core.Sink[Byte]?,
  principal : @agentCommon.Principal,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  invoke_registered_tool_with_resource_drops(
    tool_name,
    command_path,
    input,
    stdin,
    stdout,
    principal,
    @model_host.resource_drops(),
  )
}

///|
async fn invoke_registered_tool_with_resource_drops(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  stdout : @async-core.Sink[Byte]?,
  principal : @agentCommon.Principal,
  drops : @model.SchemaValueResourceDrops,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  if !state.tools.contains(tool_name) {
    return reject_wire_invocation_with_resource_drops(
      input,
      stdin,
      @toolCommon.ToolError::InvalidToolName(tool_name),
      drops,
    )
  }
  match state.defs.get(tool_name) {
    Some(def) if def.command_index_by_path(command_path) is None =>
      return reject_wire_invocation_with_resource_drops(
        input,
        stdin,
        @toolCommon.ToolError::InvalidCommandPath(command_path),
        drops,
      )
    _ => ()
  }
  invoke_registered_invoker(
    tool_name, command_path, input, stdin, stdout, principal, drops,
  )
}

///|
async fn invoke_registered_invoker(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  stdout : @async-core.Sink[Byte]?,
  principal : @agentCommon.Principal,
  drops : @model.SchemaValueResourceDrops,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  match state.invokers.get(tool_name) {
    Some(invoker) => invoker(command_path, input, stdin, stdout, principal)
    None =>
      reject_wire_invocation_with_resource_drops(
        input,
        stdin,
        @toolCommon.ToolError::InvalidToolName(tool_name),
        drops,
      )
  }
}

///|
pub async fn invoke_registered_subtool(
  tool_name : String,
  command_path : Array[String],
  fields : Array[CanonicalInputValue],
  stdin : @async-core.Stream[Byte]?,
  stdout : @async-core.Sink[Byte]?,
  principal : @agentCommon.Principal,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  invoke_registered_subtool_with_resource_drops(
    tool_name,
    command_path,
    fields,
    stdin,
    stdout,
    principal,
    @model_host.resource_drops(),
  )
}

///|
async fn invoke_registered_subtool_with_resource_drops(
  tool_name : String,
  command_path : Array[String],
  fields : Array[CanonicalInputValue],
  stdin : @async-core.Stream[Byte]?,
  stdout : @async-core.Sink[Byte]?,
  principal : @agentCommon.Principal,
  drops : @model.SchemaValueResourceDrops,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  let route = match state.defs.get(tool_name) {
    None => Err(@toolCommon.ToolError::InvalidToolName(tool_name))
    Some(def) =>
      match def.command_index_by_path(command_path) {
        None => Err(@toolCommon.ToolError::InvalidCommandPath(command_path))
        Some(index) if !state.invokers.contains(tool_name) =>
          Err(@toolCommon.ToolError::InvalidToolName(tool_name))
        Some(index) => Ok((def, index))
      }
  }
  let (def, index) = match route {
    Err(error) =>
      return reject_decoded_invocation_with_resource_drops(
        fields, stdin, error, drops,
      )
    Ok(route) => route
  }
  let input = forward_canonical_input(fields, def, index) catch {
    error =>
      return reject_decoded_invocation_with_resource_drops(
        fields, stdin, error, drops,
      )
  }
  release_canonical_input_values(fields, drops)
  invoke_registered_invoker(
    tool_name, command_path, input, stdin, stdout, principal, drops,
  )
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
