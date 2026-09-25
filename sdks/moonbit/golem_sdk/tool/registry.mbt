///|
type ToolInvoker = async (
  Array[String],
  @types.TypedSchemaValue,
  @async-core.Stream[Byte]?,
  ProviderStdout?,
  @toolCore.Principal,
) -> Result[@toolCommon.InvocationResult, @types.ToolError]

///|
let invokers : Map[String, ToolInvoker] = Map([])

///|
#doc(hidden)
pub fn validate_tool_stdout(
  tool_name : String,
  command_path : Array[String],
  supplied : Bool,
) -> Result[Unit, @types.ToolError] {
  let tool = match get_registered_tool(tool_name) {
    Ok(tool) => tool
    Err(error) => return Err(error)
  }
  let index = match resolve_command_path(tool, command_path) {
    Ok(index) => index
    Err(_) => return Err(@types.ToolError::InvalidCommandPath(command_path))
  }
  let body = match tool.commands.nodes[index].body {
    Some(body) => body
    None => return Err(@types.ToolError::InvalidCommandPath(command_path))
  }
  match body.stdout {
    None if supplied =>
      Err(
        @types.ToolError::InvalidInput(
          "command does not accept a stdout stream",
        ),
      )
    Some(spec) if spec.required && !supplied =>
      Err(@types.ToolError::InvalidInput("required stdout stream is missing"))
    _ => Ok(())
  }
}

///|
pub fn register_tool_def_invoker(
  tool : @toolCore.ToolDef,
  invoker : ToolInvoker,
) -> Unit raise @toolCore.ToolBuildError {
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  @toolCore.register_tool_def(tool)
  invokers[name] = (path, input, stdin, stdout, principal) => {
    if tool.command_index_by_path(path) is None {
      @toolCore.reject_wire_invocation(
        input,
        stdin,
        @types.ToolError::InvalidCommandPath(path),
      )
    } else {
      invoker(path, input, stdin, stdout, principal)
    }
  }
}

///|
pub fn register_subtool_def_invoker(
  tool : @toolCore.ToolDef,
  invoker : ToolInvoker,
) -> Unit raise @toolCore.ToolBuildError {
  let name = match tool.tool_name() {
    Ok(name) => name
    Err(error) => raise error
  }
  @toolCore.register_subtool_def(tool)
  invokers[name] = invoker
}

///|
pub fn register_wire_tool_invoker(
  tool : @toolCommon.Tool,
  invoker : ToolInvoker,
) -> Unit raise @toolCore.ToolBuildError {
  let name = match @toolCore.tool_name(tool) {
    Ok(name) => name
    Err(error) => raise error
  }
  @toolCore.register_wire_tool(tool)
  invokers[name] = invoker
}

///|
pub fn register_wire_subtool_invoker(
  tool : @toolCommon.Tool,
  invoker : ToolInvoker,
) -> Unit raise @toolCore.ToolBuildError {
  let name = match @toolCore.tool_name(tool) {
    Ok(name) => name
    Err(error) => raise error
  }
  @toolCore.register_wire_subtool(tool)
  invokers[name] = invoker
}

///|
async fn invoke_invoker(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  stdout : ProviderStdout?,
  principal : @toolCore.Principal,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  match invokers.get(tool_name) {
    Some(invoker) => invoker(command_path, input, stdin, stdout, principal)
    None =>
      @toolCore.reject_wire_invocation(
        input,
        stdin,
        @types.ToolError::InvalidToolName(tool_name),
      )
  }
}

///|
pub async fn invoke_registered_tool(
  tool_name : String,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  stdout : ProviderStdout?,
  principal : @toolCore.Principal,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  if @toolCore.get_registered_tool(tool_name) is Err(_) {
    return @toolCore.reject_wire_invocation(
      input,
      stdin,
      @types.ToolError::InvalidToolName(tool_name),
    )
  }
  invoke_invoker(tool_name, command_path, input, stdin, stdout, principal)
}

///|
pub async fn invoke_registered_subtool(
  tool_name : String,
  command_path : Array[String],
  fields : Array[@toolCore.CanonicalInputValue],
  stdin : @async-core.Stream[Byte]?,
  stdout : ProviderStdout?,
  principal : @toolCore.Principal,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  let route = match @toolCore.registered_tool_def(tool_name) {
    None => Err(@types.ToolError::InvalidToolName(tool_name))
    Some(def) =>
      match def.command_index_by_path(command_path) {
        None => Err(@types.ToolError::InvalidCommandPath(command_path))
        Some(_) if !invokers.contains(tool_name) =>
          Err(@types.ToolError::InvalidToolName(tool_name))
        Some(index) => Ok((def, index))
      }
  }
  let (def, index) = match route {
    Err(error) =>
      return @toolCore.reject_decoded_invocation(fields, stdin, error)
    Ok(route) => route
  }
  let input = @toolCore.forward_canonical_input(fields, def, index) catch {
    error => return @toolCore.reject_decoded_invocation(fields, stdin, error)
  }
  @toolCore.release_registered_subtool_input(fields)
  invoke_invoker(tool_name, command_path, input, stdin, stdout, principal)
}
