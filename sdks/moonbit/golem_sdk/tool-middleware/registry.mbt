///|
pub(all) struct MonomorphicToolMiddlewareScope {
  presented : @toolCommon.Tool
  expected : @toolCommon.Tool?
}

///|
pub(all) enum ToolMiddlewareScope {
  Monomorphic(MonomorphicToolMiddlewareScope)
  Universal
}

///|
pub(all) struct ToolMiddleware {
  name : String
  version : String
  aliases : Array[String]
  doc : @toolCommon.Doc
  scope : ToolMiddlewareScope
  parameter_schema : @model.SchemaGraph
}

///|
#doc(hidden)
pub type ToolMiddlewareInvoker = async (RawToolInvocation, UnderlyingTool) -> Result[
  RawInvocationResult,
  ToolInvokeError[RawTypedSchemaValue],
]

///|
priv struct ToolMiddlewareState {
  descriptors : Map[String, ToolMiddleware]
  invokers : Map[String, ToolMiddlewareInvoker]
  identities : Map[String, String]
}

///|
let tool_middleware_state : ToolMiddlewareState = {
  descriptors: {},
  invokers: {},
  identities: {},
}

///|
fn ToolMiddleware::to_wit(self : ToolMiddleware) -> @toolCommon.ToolMiddleware {
  {
    name: self.name,
    version: self.version,
    aliases: self.aliases.copy(),
    doc: self.doc,
    scope: match self.scope {
      Monomorphic(scope) =>
        @toolCommon.ToolMiddlewareScope::Monomorphic({
          presented: scope.presented,
          expected: scope.expected,
        })
      Universal => @toolCommon.ToolMiddlewareScope::Universal
    },
    parameter_schema: try! @model.schema_graph_to_wit(self.parameter_schema),
  }
}

///|
#doc(hidden)
pub fn empty_parameter_schema() -> @model.SchemaGraph {
  { defs: [], root: @model.schema_type(@model.SchemaTypeBody::Record([])), }
}

///|
fn validate_installation_parameters(
  declared : @model.SchemaGraph,
  supplied : @model.TypedSchemaValue,
) -> Bool {
  if @model.validate_graph(supplied.graph).length() > 0 ||
    supplied.graph != declared {
    return false
  }
  if declared == empty_parameter_schema() {
    return supplied.value is @model.SchemaValue::Record([])
  }
  true
}

///|
fn ToolMiddleware::registration_error(self : ToolMiddleware) -> String? {
  let seen : Map[String, Unit] = Map([])
  let identities = [self.name]
  identities.append(self.aliases)
  for identity in identities {
    if identity == "" {
      return Some("tool middleware names and aliases must not be empty")
    }
    if seen.contains(identity) ||
      tool_middleware_state.identities.contains(identity) {
      return Some(
        "duplicate tool middleware registration for middleware name or alias: " +
        identity,
      )
    }
    seen[identity] = ()
  }
  None
}

///|
fn register_tool_middleware_checked(
  middleware : ToolMiddleware,
  invoker : ToolMiddlewareInvoker,
) -> Result[Unit, String] {
  match middleware.registration_error() {
    Some(message) => return Err(message)
    None => ()
  }
  let canonical_name = middleware.name
  tool_middleware_state.identities[canonical_name] = canonical_name
  for alias_name in middleware.aliases {
    tool_middleware_state.identities[alias_name] = canonical_name
  }
  tool_middleware_state.descriptors[canonical_name] = middleware
  tool_middleware_state.invokers[canonical_name] = invoker
  Ok(())
}

///|
#doc(hidden)
pub fn register_tool_middleware(
  middleware : ToolMiddleware,
  invoker : ToolMiddlewareInvoker,
) -> Unit {
  match register_tool_middleware_checked(middleware, invoker) {
    Ok(_) => ()
    Err(message) => abort(message)
  }
}

///|
#doc(hidden)
pub fn discover_tool_middlewares() -> Array[@toolCommon.ToolMiddleware] {
  let descriptors = tool_middleware_state.descriptors
    .to_array()
    .map(entry => entry.1.to_wit())
  descriptors.sort_by((left, right) => right.name.compare(left.name))
  descriptors
}

///|
#doc(hidden)
pub fn get_registered_tool_middleware(
  name : String,
) -> Result[@toolCommon.ToolMiddleware, @types.ToolError] {
  match tool_middleware_state.descriptors.get(name) {
    Some(middleware) => Ok(middleware.to_wit())
    None => Err(@types.ToolError::InvalidToolName(name))
  }
}

///|
#doc(hidden)
pub async fn invoke_registered_tool_middleware_wire(
  middleware_name : String,
  tool_name : String,
  tool_metadata : @toolCommon.Tool,
  parameters : @types.TypedSchemaValue,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[
    Result[FixedArray[Byte], @toolStreams.ByteStreamFailure],
  ]?,
  stdout : @toolStreams.ToolOutputWriter?,
  stderr : @toolStreams.ToolOutputWriter?,
  principal : @agentCommon.Principal,
  underlying : UnderlyingTool,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  let context = underlying.ownership_context()
  let decoded_parameters = @model_host.typed_schema_value_from_wit(parameters) catch {
    error =>
      return Err(
        @types.ToolError::InvalidInput(
          "failed to decode tool middleware parameters: \{Repr(error)}",
        ),
      )
  }
  context.register_cleanup(fn() {
    @model.drop_owned_capabilities(decoded_parameters.value)
  })
  let invocation = RawToolInvocation::from_wit(
    context, tool_name, tool_metadata, decoded_parameters, command_path, input, stdin,
    stdout, stderr, principal,
  )
  let parameter_schema = match
    tool_middleware_state.descriptors.get(middleware_name) {
    Some(middleware) => middleware.parameter_schema
    None => return Err(@types.ToolError::InvalidToolName(middleware_name))
  }
  if !validate_installation_parameters(parameter_schema, decoded_parameters) {
    return Err(
      @types.ToolError::InvalidInput(
        "tool middleware parameters do not match the declared parameter schema",
      ),
    )
  }
  let invoker = match tool_middleware_state.invokers.get(middleware_name) {
    Some(invoker) => invoker
    None => return Err(@types.ToolError::InvalidToolName(middleware_name))
  }
  let host_stdout = invocation.take_host_stdout()
  let host_stderr = invocation.take_host_stderr()
  let authored = invoker(invocation, underlying)
  let final_result = match authored {
    Ok(result) =>
      result
      .take_for_final(context)
      .map_err(message => @types.ToolError::InvalidResult(message))
    Err(error) => Err(encode_raw_tool_error(error, context))
  }
  match final_result {
    Err(error) => {
      match host_stdout {
        Some(writer) => writer.drop()
        None => ()
      }
      match host_stderr {
        Some(writer) => writer.drop()
        None => ()
      }
      context.revoke()
      Err(error)
    }
    Ok(result) => {
      @async-core.with_task_group(async fn(group) {
        let stdout_pump = host_stdout.map(writer => {
          group.spawn(() => pump_final_output(result.stdout, writer))
        })
        let stderr_pump = host_stderr.map(writer => {
          group.spawn(() => pump_final_output(result.stderr, writer))
        })
        match stdout_pump {
          Some(pump) => ignore(pump.wait())
          None => ()
        }
        match stderr_pump {
          Some(pump) => ignore(pump.wait())
          None => ()
        }
      })
      context.revoke()
      Ok({
        result: result.result,
        stdout: if host_stdout is Some(_) {
          None
        } else {
          result.stdout
        },
        stderr: if host_stderr is Some(_) {
          None
        } else {
          result.stderr
        },
      })
    }
  }
}

///|
async fn pump_final_output(
  source : @async-core.Stream[Byte]?,
  writer : @toolStreams.ToolOutputWriter,
) -> Unit {
  defer writer.drop()
  match source {
    None => ignore(writer.finish())
    Some(stream) => {
      defer stream.drop()
      for ;; {
        let chunk = stream.read(65536) catch {
          @toolCore.ToolStreamError::Cancelled => {
            ignore(writer.fail(@toolStreams.ByteStreamFailure::Cancelled))
            return
          }
          @toolCore.ToolStreamError::Abandoned => {
            ignore(writer.fail(@toolStreams.ByteStreamFailure::Abandoned))
            return
          }
          @toolCore.ToolStreamError::ResourceExhausted => {
            ignore(
              writer.fail(@toolStreams.ByteStreamFailure::ResourceExhausted),
            )
            return
          }
          error => {
            ignore(
              writer.fail(
                @toolStreams.ByteStreamFailure::Failed(error.to_string()),
              ),
            )
            return
          }
        }
        match chunk {
          Some(bytes) =>
            if writer.write(FixedArray::from_array(bytes)) is Err(_) {
              return
            }
          None => {
            ignore(writer.finish())
            return
          }
        }
      }
    }
  }
}

///|
async fn invoke_registered_tool_middleware(
  middleware_name : String,
  tool_name : String,
  tool_metadata : @toolCommon.Tool,
  parameters : @types.TypedSchemaValue,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  principal : @agentCommon.Principal,
  underlying : UnderlyingTool,
) -> Result[@toolCommon.InvocationResult, @types.ToolError] {
  invoke_registered_tool_middleware_wire(
    middleware_name,
    tool_name,
    tool_metadata,
    parameters,
    command_path,
    input,
    stdin.map(byte_stream_items),
    None,
    None,
    principal,
    underlying,
  )
}

///|
#warnings("-unused_value")
fn clear_tool_middlewares_for_tests() -> Unit {
  tool_middleware_state.descriptors.clear()
  tool_middleware_state.invokers.clear()
  tool_middleware_state.identities.clear()
}
