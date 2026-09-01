///|
pub(all) struct MonomorphicToolMiddlewareScope {
  presented : @toolCommon.Tool
  expected : @toolCommon.Tool
}

///|
pub(all) enum ToolMiddlewareScope {
  Monomorphic(MonomorphicToolMiddlewareScope)
  Universal
}

///|
pub(all) struct ToolMiddleware {
  name : String
  aliases : Array[String]
  doc : @toolCommon.Doc
  scope : ToolMiddlewareScope
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
    aliases: self.aliases.copy(),
    doc: self.doc,
    scope: match self.scope {
      Monomorphic(scope) =>
        @toolCommon.ToolMiddlewareScope::Monomorphic({
          presented: scope.presented,
          expected: Some(scope.expected),
        })
      Universal => @toolCommon.ToolMiddlewareScope::Universal
    },
  }
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
) -> Result[@toolCommon.ToolMiddleware, @toolCommon.ToolError] {
  match tool_middleware_state.descriptors.get(name) {
    Some(middleware) => Ok(middleware.to_wit())
    None => Err(@toolCommon.ToolError::InvalidToolName(name))
  }
}

///|
#doc(hidden)
pub async fn invoke_registered_tool_middleware(
  middleware_name : String,
  tool_name : String,
  tool_metadata : @toolCommon.Tool,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  principal : @agentCommon.Principal,
  underlying : UnderlyingTool,
) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {
  let context = underlying.ownership_context()
  let invocation = RawToolInvocation::from_wit(
    context, tool_name, tool_metadata, command_path, input, stdin, principal,
  )
  let invoker = match tool_middleware_state.invokers.get(middleware_name) {
    Some(invoker) => invoker
    None => return Err(@toolCommon.ToolError::InvalidToolName(middleware_name))
  }
  match invoker(invocation, underlying) {
    Ok(result) =>
      match result.take_for_final(context) {
        Ok(result) => Ok(result)
        Err(message) => Err(@toolCommon.ToolError::InvalidResult(message))
      }
    Err(error) => Err(encode_raw_tool_error(error, context))
  }
}

///|
#warnings("-unused_value")
fn clear_tool_middlewares_for_tests() -> Unit {
  tool_middleware_state.descriptors.clear()
  tool_middleware_state.invokers.clear()
  tool_middleware_state.identities.clear()
}
