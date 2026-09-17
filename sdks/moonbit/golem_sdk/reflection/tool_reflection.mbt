///|
/// A selected argument in a discovered tool command's canonical input.
pub(all) struct ToolArgument {
  kind : String
  name : String
  aliases : Array[String]
  short : Char?
  required : Bool
  default : @model.SchemaValue?
  schema : SchemaRef
  optional_carrier : Bool
} derive(Debug, Eq)

///|
/// A snapshot of one tool registration visible to the calling component.
pub(all) struct ToolType {
  lookup_name : String
  definition : @toolCommon.Tool
  implemented_by : @types.ComponentId
} derive(Debug, Eq)

///|
pub(all) struct ReflectedToolClient {
  tool : ToolType
} derive(Debug, Eq)

///|
pub(all) struct ToolCommand {
  tool : ToolType
  path : Array[String]
  body : @toolCommon.CommandBody
  arguments : Array[ToolArgument]
  input_schema : SchemaRef
  priv wire_input : @model.SchemaGraph
  result : SchemaRef?
} derive(Debug, Eq)

///|
pub(all) struct ReflectedToolCustomError {
  name : String
  payload : @model.TypedSchemaValue
} derive(Debug, Eq)

///|
pub struct ReflectedToolInvocation {
  priv command : ToolCommand
  priv invocation : @tool.ToolInvocation[ReflectedToolCustomError]
}

///|
pub(all) struct DynamicToolClient {
  name : String
} derive(Debug, Eq)

///|
pub fn get_all_tool_types() -> Array[ToolType] raise ReflectError {
  let result : Array[ToolType] = []
  for registered in @toolHost.get_all_tools() {
    result.push(ToolType::from_registered(registered))
  }
  result
}

///|
pub fn get_tool_type(name : String) -> ToolType raise ReflectError {
  match @toolHost.get_tool(name) {
    Some(registered) => ToolType::from_registered(registered)
    None => raise Discovery("tool '" + name + "' was not found")
  }
}

///|
pub fn ToolType::from_registered(
  registered : @toolHost.RegisteredTool,
) -> ToolType raise ReflectError {
  guard registered.definition.commands.nodes.length() > 0 else {
    raise Discovery("tool '" + registered.lookup_name + "' has no root command")
  }
  let _ = @model.schema_graph_from_wit(registered.definition.schema) catch {
    error => raise Schema("failed to decode tool schema: \{Repr(error)}")
  }
  {
    lookup_name: registered.lookup_name,
    definition: registered.definition,
    implemented_by: registered.implemented_by,
  }
}

///|
fn tool_schema_at(tool : ToolType, root : Int) -> SchemaRef raise ReflectError {
  let graph = @model.schema_graph_from_wit({ ..tool.definition.schema, root, }) catch {
    error =>
      raise Schema("failed to decode selected tool schema: \{Repr(error)}")
  }
  SchemaRef::new(graph)
}

///|
fn tool_option_argument(
  tool : ToolType,
  spec : @toolCommon.OptionSpec,
) -> ToolArgument raise ReflectError {
  let scalar_shape = match spec.shape {
    Scalar(_) | OptionalScalar(_) => true
    RepeatableList(_) | RepeatableMap(_) => false
  }
  let optional_carrier = !spec.required && spec.default is None && scalar_shape
  let schema = match spec.shape {
    Scalar(root) | OptionalScalar(root) => tool_schema_at(tool, root)
    RepeatableList(shape) => {
      let item = tool_schema_at(tool, shape.item_type)
      SchemaRef::new({
        defs: item.graph.defs,
        root: @model.schema_type(List(item.root)),
      })
    }
    RepeatableMap(shape) => tool_schema_at(tool, shape.map_type)
  }
  {
    kind: "option",
    name: spec.long,
    aliases: spec.aliases,
    short: spec.short,
    required: spec.required,
    default: match spec.default {
      Some(value) =>
        Some(
          @model_host.schema_value_from_wit(value) catch {
            error =>
              raise Schema("failed to decode tool default: \{Repr(error)}")
          },
        )
      None => None
    },
    schema,
    optional_carrier,
  }
}

///|
fn tool_flag_argument(
  tool : ToolType,
  spec : @toolCommon.FlagSpec,
) -> ToolArgument raise ReflectError {
  let root = match spec.shape {
    BoolFlag(_) => @model.schema_type(Bool)
    CountFlag(_) => @model.schema_type(U32(None))
  }
  let decoded = @model.schema_graph_from_wit(tool.definition.schema) catch {
    error => raise Schema("failed to decode tool schema: \{Repr(error)}")
  }
  {
    kind: "flag",
    name: spec.long,
    aliases: spec.aliases,
    short: spec.short,
    required: false,
    default: None,
    schema: SchemaRef::new({ defs: decoded.defs, root, }),
    optional_carrier: false,
  }
}

///|
pub fn ToolType::command(
  self : ToolType,
  path : Array[String],
) -> ToolCommand raise ReflectError {
  let nodes = self.definition.commands.nodes
  let mut index = 0
  let indices : Array[Int] = [0]
  let canonical : Array[String] = []
  for segment in path {
    let next = nodes[index].subcommands
      .iter()
      .find_first(fn(child) {
        let node = nodes[child]
        node.name == segment || node.aliases.contains(segment)
      })
    index = match next {
      Some(next) => next
      None => raise Discovery("tool command '" + segment + "' was not found")
    }
    indices.push(index)
    canonical.push(nodes[index].name)
  }
  let body = match nodes[index].body {
    Some(body) => body
    None => raise Discovery("tool command has no body")
  }
  let local_names : Array[String] = []
  for item in body.positionals.fixed {
    local_names.push(item.name)
  }
  match body.positionals.tail {
    Some(item) => local_names.push(item.name)
    None => ()
  }
  for item in body.options {
    local_names.push(item.long)
    local_names.append(item.aliases)
  }
  for item in body.flags {
    local_names.push(item.long)
    local_names.append(item.aliases)
  }
  let arguments : Array[ToolArgument] = []
  for node_index in indices {
    let node = nodes[node_index]
    for item in node.globals.options {
      let argument = tool_option_argument(self, item)
      if !local_names.contains(argument.name) &&
        !argument.aliases.any(fn(alternative) {
          local_names.contains(alternative)
        }) {
        arguments.push(argument)
      }
    }
    for item in node.globals.flags {
      let argument = tool_flag_argument(self, item)
      if !local_names.contains(argument.name) &&
        !argument.aliases.any(fn(alternative) {
          local_names.contains(alternative)
        }) {
        arguments.push(argument)
      }
    }
  }
  for item in body.positionals.fixed {
    arguments.push({
      kind: "positional",
      name: item.name,
      aliases: [],
      short: None,
      required: item.required,
      default: match item.default {
        Some(value) =>
          Some(
            @model_host.schema_value_from_wit(value) catch {
              error =>
                raise Schema("failed to decode tool default: \{Repr(error)}")
            },
          )
        None => None
      },
      schema: tool_schema_at(self, item.type_),
      optional_carrier: !item.required && item.default is None,
    })
  }
  match body.positionals.tail {
    Some(item) => {
      let inner = tool_schema_at(self, item.item_type)
      arguments.push({
        kind: "tail",
        name: item.name,
        aliases: [],
        short: None,
        required: item.min > 0,
        default: None,
        schema: SchemaRef::new({
          defs: inner.graph.defs,
          root: @model.schema_type(List(inner.root)),
        }),
        optional_carrier: false,
      })
    }
    None => ()
  }
  for item in body.options {
    arguments.push(tool_option_argument(self, item))
  }
  for item in body.flags {
    arguments.push(tool_flag_argument(self, item))
  }
  let input_fields : Array[@model.NamedFieldType] = []
  for argument in arguments {
    input_fields.push({
      name: argument.name,
      body: if argument.optional_carrier {
        @model.schema_type(Option(argument.schema.root))
      } else {
        argument.schema.root
      },
      metadata: @model.empty_metadata(),
    })
  }
  let decoded = @model.schema_graph_from_wit(self.definition.schema) catch {
    error => raise Schema("failed to decode tool schema: \{Repr(error)}")
  }
  let result = match body.result {
    Some(spec) => Some(tool_schema_at(self, spec.type_))
    None => None
  }
  let input_root = @model.schema_type(Record(input_fields))
  {
    tool: self,
    path: canonical,
    body,
    arguments,
    input_schema: SchemaRef::new({ defs: decoded.defs, root: input_root, }),
    wire_input: { defs: decoded.defs, root: input_root, },
    result,
  }
}

///|
pub fn ToolType::client(self : ToolType) -> ReflectedToolClient {
  { tool: self, }
}

///|
pub fn ReflectedToolClient::command(
  self : ReflectedToolClient,
  path : Array[String],
) -> ToolCommand raise ReflectError {
  self.tool.command(path)
}

///|
pub fn ToolCommand::pack_json(
  self : ToolCommand,
  input : Json,
) -> Result[@model.SchemaValue, @tool.ToolError[ReflectedToolCustomError]] {
  let value = self.input_schema.pack_json(input) catch {
    error => return Err(@tool.ToolError::InvalidInput("\{Repr(error)}"))
  }
  match self.check_constraints(value) {
    Ok(_) => Ok(value)
    Err(error) => Err(error)
  }
}

///|
fn ToolCommand::checked_input(
  self : ToolCommand,
  input : @model.SchemaValue,
) -> Result[@model.TypedSchemaValue, @tool.ToolError[ReflectedToolCustomError]] {
  self.input_schema.validate_value(input) catch {
    error => return Err(@tool.ToolError::InvalidInput("\{Repr(error)}"))
  }
  match self.check_constraints(input) {
    Ok(_) => Ok({ graph: self.wire_input, value: input, })
    Err(error) => Err(error)
  }
}

///|
fn ToolCommand::check_constraints(
  self : ToolCommand,
  value : @model.SchemaValue,
) -> Result[Unit, @tool.ToolError[ReflectedToolCustomError]] {
  let fields = match value {
    Record(fields) => fields
    _ =>
      return Err(@tool.ToolError::InvalidInput("tool input must be a record"))
  }
  if fields.length() != self.arguments.length() {
    return Err(
      @tool.ToolError::InvalidInput(
        "tool input field count differs from declaration",
      ),
    )
  }
  for constraint in self.body.constraints {
    let okay = match constraint {
      RequiresAll(refs) => self.constraint_refs_match(refs, fields, true)
      RequiresAny(refs) => self.constraint_refs_match(refs, fields, false)
      AllOrNone(refs) => {
        let count = refs
          .iter()
          .filter(fn(reference) {
            self.constraint_ref_matches(reference, fields)
          })
          .count()
        count == 0 || count == refs.length()
      }
      MutexGroups(groups) => {
        let count = groups
          .iter()
          .filter(fn(group) {
            self.constraint_refs_match(group.refs, fields, true)
          })
          .count()
        count <= 1
      }
      Implies(item) =>
        !self.constraint_refs_match(item.lhs, fields, item.lhs_quant == ALL) ||
        self.constraint_refs_match(item.rhs, fields, item.rhs_quant == ALL)
      Forbids(item) =>
        !self.constraint_refs_match(item.lhs, fields, item.lhs_quant == ALL) ||
        !self.constraint_refs_match(item.rhs, fields, false)
    }
    if !okay {
      return Err(
        @tool.ToolError::InvalidInput("tool command constraint failed"),
      )
    }
  }
  Ok(())
}

///|
fn ToolCommand::constraint_refs_match(
  self : ToolCommand,
  refs : Array[@toolCommon.Ref],
  values : Array[@model.SchemaValue],
  all : Bool,
) -> Bool {
  if all {
    refs.all(fn(reference) { self.constraint_ref_matches(reference, values) })
  } else {
    refs.any(fn(reference) { self.constraint_ref_matches(reference, values) })
  }
}

///|
fn ToolCommand::constraint_ref_matches(
  self : ToolCommand,
  reference : @toolCommon.Ref,
  values : Array[@model.SchemaValue],
) -> Bool {
  let name = match reference {
    Present(name) => name
    ValueIs(item) => item.name
  }
  let mut index = -1
  for i = 0; i < self.arguments.length(); i = i + 1 {
    let argument = self.arguments[i]
    if argument.name == name || argument.aliases.contains(name) {
      index = i
      break
    }
  }
  if index < 0 {
    return false
  }
  let actual = values[index]
  match reference {
    Present(_) =>
      match actual {
        Option(None) => false
        _ =>
          match self.arguments[index].default {
            Some(default) => actual != default
            None => true
          }
      }
    ValueIs(item) => {
      let expected = @model_host.schema_value_from_wit(item.value) catch {
        _ => return false
      }
      actual == expected
    }
  }
}

///|
fn ToolCommand::decode_result(
  self : ToolCommand,
  outcome : @tool.InvocationResult,
) -> Result[@model.SchemaValue?, @tool.ToolError[ReflectedToolCustomError]] {
  match outcome.stdout {
    Some(stdout) => {
      stdout.drop_sync()
      return Err(
        @tool.ToolError::MalformedRemoteOutput(
          "remote tool returned an embedded stdout stream",
        ),
      )
    }
    None => ()
  }
  match (self.result, outcome.result) {
    (None, None) => Ok(None)
    (Some(schema), Some(payload)) => {
      if payload.graph.root != schema.root {
        payload.value.release_resources(@model_host.resource_drops())
        return Err(
          @tool.ToolError::MalformedRemoteOutput(
            "remote tool result schema does not match its declaration",
          ),
        )
      }
      schema.validate_value(payload.value) catch {
        error => {
          payload.value.release_resources(@model_host.resource_drops())
          return Err(@tool.ToolError::MalformedRemoteOutput("\{Repr(error)}"))
        }
      }
      Ok(Some(payload.value))
    }
    (_, Some(payload)) => {
      payload.value.release_resources(@model_host.resource_drops())
      Err(
        @tool.ToolError::MalformedRemoteOutput(
          "remote tool returned an unexpected value",
        ),
      )
    }
    _ =>
      Err(
        @tool.ToolError::MalformedRemoteOutput(
          "remote tool omitted a declared value",
        ),
      )
  }
}

///|
fn ToolCommand::decode_error(
  self : ToolCommand,
  name : String,
  payload : @model.TypedSchemaValue,
) -> Result[ReflectedToolCustomError, String]? {
  let declaration = self.body.errors
    .iter()
    .find_first(fn(item) { item.name == name })
  match declaration {
    None => None
    Some(declaration) =>
      match declaration.payload {
        Some(root) => {
          let schema = tool_schema_at(self.tool, root) catch {
            error => return Some(Err("\{Repr(error)}"))
          }
          if payload.graph.root != schema.root {
            return Some(Err("custom tool error has the wrong schema"))
          }
          schema.validate_value(payload.value) catch {
            error => return Some(Err("\{Repr(error)}"))
          }
          Some(Ok({ name, payload, }))
        }
        None =>
          match payload.value {
            Tuple(elements) if elements.is_empty() =>
              Some(Ok({ name, payload, }))
            _ => Some(Err("custom tool error has an unexpected payload"))
          }
      }
  }
}

///|
pub fn ToolCommand::start_value(
  self : ToolCommand,
  input : @model.SchemaValue,
  stdin? : @async-core.Stream[Byte]? = None,
) -> Result[ReflectedToolInvocation, @tool.ToolError[ReflectedToolCustomError]] {
  if self.body.stdin is Some(spec) && spec.required && stdin is None {
    return Err(@tool.ToolError::InvalidInput("command requires stdin"))
  }
  let input = match self.checked_input(input) {
    Ok(input) => input
    Err(error) => return Err(error)
  }
  let client = match @tool.ToolClient::try_new(self.tool.lookup_name) {
    Ok(client) => client
    Err(error) => return Err(error)
  }
  let started = client.start(
    self.path,
    input,
    stdin,
    self.body.stdout is Some(_),
    fn(name, payload) { self.decode_error(name, payload) },
  )
  client.drop()
  match started {
    Ok(invocation) => Ok({ command: self, invocation, })
    Err(error) => Err(error)
  }
}

///|
pub async fn ToolCommand::invoke_value(
  self : ToolCommand,
  input : @model.SchemaValue,
  stdin? : @async-core.Stream[Byte]? = None,
) -> Result[@model.SchemaValue?, @tool.ToolError[ReflectedToolCustomError]] {
  if self.body.stdout is Some(spec) && spec.required {
    return Err(
      @tool.ToolError::InvalidInput("command requires caller-readable stdout"),
    )
  }
  let started = match self.start_value(input, stdin~) {
    Ok(started) => started
    Err(error) => return Err(error)
  }
  if self.body.stdout is Some(_) {
    match started.collect() {
      Ok((result, _)) => Ok(result)
      Err(error) => Err(error)
    }
  } else {
    started.get()
  }
}

///|
pub async fn ToolCommand::invoke_json(
  self : ToolCommand,
  input : Json,
  stdin? : @async-core.Stream[Byte]? = None,
) -> Result[Json?, @tool.ToolError[ReflectedToolCustomError]] {
  let packed = match self.pack_json(input) {
    Ok(value) => value
    Err(error) => return Err(error)
  }
  let result = match self.invoke_value(packed, stdin~) {
    Ok(result) => result
    Err(error) => return Err(error)
  }
  match (self.result, result) {
    (Some(schema), Some(value)) =>
      try schema.unpack_json(value) catch {
        error => Err(@tool.ToolError::MalformedRemoteOutput("\{Repr(error)}"))
      } noraise {
        json => Ok(Some(json))
      }
    _ => Ok(None)
  }
}

///|
pub fn ToolCommand::trigger_value(
  self : ToolCommand,
  input : @model.SchemaValue,
  stdin? : @async-core.Stream[Byte]? = None,
) -> Result[Unit, @tool.ToolError[ReflectedToolCustomError]] {
  if self.body.stdout is Some(spec) && spec.required {
    return Err(
      @tool.ToolError::InvalidInput("command requires caller-readable stdout"),
    )
  }
  if self.body.stdin is Some(spec) && spec.required && stdin is None {
    return Err(@tool.ToolError::InvalidInput("command requires stdin"))
  }
  let input = match self.checked_input(input) {
    Ok(input) => input
    Err(error) => return Err(error)
  }
  let client = match @tool.ToolClient::try_new(self.tool.lookup_name) {
    Ok(client) => client
    Err(error) => return Err(error)
  }
  let outcome = client.trigger(self.path, input, stdin, fn(name, payload) {
    self.decode_error(name, payload)
  })
  client.drop()
  outcome
}

///|
pub fn ToolCommand::trigger_json(
  self : ToolCommand,
  input : Json,
  stdin? : @async-core.Stream[Byte]? = None,
) -> Result[Unit, @tool.ToolError[ReflectedToolCustomError]] {
  match self.pack_json(input) {
    Ok(value) => self.trigger_value(value, stdin~)
    Err(error) => Err(error)
  }
}

///|
pub fn ReflectedToolInvocation::cancel(self : ReflectedToolInvocation) -> Unit {
  self.invocation.cancel()
}

///|
pub fn ReflectedToolInvocation::drop(self : ReflectedToolInvocation) -> Unit {
  self.invocation.drop()
}

///|
pub fn ReflectedToolInvocation::stdout(
  self : ReflectedToolInvocation,
) -> @async-core.Stream[Byte]? {
  self.invocation.stdout_stream()
}

///|
pub async fn ReflectedToolInvocation::get(
  self : ReflectedToolInvocation,
) -> Result[@model.SchemaValue?, @tool.ToolError[ReflectedToolCustomError]] {
  match self.invocation.get() {
    Ok(outcome) => self.command.decode_result(outcome)
    Err(error) => Err(error)
  }
}

///|
pub async fn ReflectedToolInvocation::collect(
  self : ReflectedToolInvocation,
) -> Result[
  (@model.SchemaValue?, Bytes?),
  @tool.ToolError[ReflectedToolCustomError],
] {
  match self.invocation.collect() {
    Ok((outcome, bytes)) =>
      match self.command.decode_result(outcome) {
        Ok(result) => Ok((result, bytes))
        Err(error) => Err(error)
      }
    Err(error) => Err(error)
  }
}

///|
pub fn DynamicToolClient::new(name : String) -> DynamicToolClient {
  { name, }
}

///|
pub async fn DynamicToolClient::invoke_value(
  self : DynamicToolClient,
  path : Array[String],
  input : @model.TypedSchemaValue,
  stdin? : @async-core.Stream[Byte]? = None,
) -> Result[@tool.InvocationResult, @tool.ToolError[ReflectedToolCustomError]] {
  let client = match @tool.ToolClient::try_new(self.name) {
    Ok(client) => client
    Err(error) => return Err(error)
  }
  let outcome = client.invoke_and_await(path, input, stdin, fn(name, payload) {
    Some(Ok({ name, payload, }))
  })
  client.drop()
  outcome
}

///|
pub fn DynamicToolClient::start_value(
  self : DynamicToolClient,
  path : Array[String],
  input : @model.TypedSchemaValue,
  stdin? : @async-core.Stream[Byte]? = None,
  stdout? : Bool = false,
) -> Result[
  @tool.ToolInvocation[ReflectedToolCustomError],
  @tool.ToolError[ReflectedToolCustomError],
] {
  let client = match @tool.ToolClient::try_new(self.name) {
    Ok(client) => client
    Err(error) => return Err(error)
  }
  let started = client.start(path, input, stdin, stdout, fn(name, payload) {
    Some(Ok({ name, payload, }))
  })
  client.drop()
  started
}

///|
pub fn DynamicToolClient::trigger_value(
  self : DynamicToolClient,
  path : Array[String],
  input : @model.TypedSchemaValue,
  stdin? : @async-core.Stream[Byte]? = None,
) -> Result[Unit, @tool.ToolError[ReflectedToolCustomError]] {
  let client = match @tool.ToolClient::try_new(self.name) {
    Ok(client) => client
    Err(error) => return Err(error)
  }
  let outcome = client.trigger(path, input, stdin, fn(name, payload) {
    Some(Ok({ name, payload, }))
  })
  client.drop()
  outcome
}
