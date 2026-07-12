///|
pub(all) struct CanonicalInputField {
  name : String
  aliases : Array[String]
  type_ : @model.SchemaGraph
} derive(Eq)

///|
pub(all) struct CanonicalInputValue {
  name : String
  aliases : Array[String]
  type_ : @model.SchemaGraph
  value : @model.SchemaValue
} derive(Eq)

///|
pub(all) struct CanonicalInputModel {
  fields : Array[CanonicalInputField]
  record_graph : @model.SchemaGraph
} derive(Eq)

///|
pub fn ToolDef::command_index_by_path(
  self : ToolDef,
  path : Array[String],
) -> Int? {
  guard self.commands.get(0) is Some(_) else { return None }
  let mut current = 0
  for part in path {
    let node = match self.commands.get(current) {
      Some(node) => node
      None => return None
    }
    let mut found = None
    for child_index in node.subcommands {
      if self.commands.get(child_index) is Some(child) &&
        (child.name == part || child.aliases.contains(part)) {
        found = Some(child_index)
        break
      }
    }
    match found {
      Some(index) => current = index
      None => return None
    }
  }
  Some(current)
}

///|
fn ToolDef::path_to_command(
  self : ToolDef,
  target : Int,
) -> Array[Int] raise ToolBuildError {
  guard self.commands.length() > 0 else { raise EmptyCommandTree }
  guard target >= 0 && target < self.commands.length() else {
    raise InvalidCommandTree("command index \{target} is out of bounds")
  }
  self.validate_command_subtree(0, {}, {})
  let path : Array[Int] = []
  let on_stack : Map[Int, Unit] = {}
  if self.visit_command(0, target, path, on_stack) {
    path
  } else {
    raise InvalidCommandTree("command index \{target} is unreachable from root")
  }
}

///|
fn ToolDef::validate_command_subtree(
  self : ToolDef,
  current : Int,
  visited : Map[Int, Unit],
  on_stack : Map[Int, Unit],
) -> Unit raise ToolBuildError {
  guard self.commands.get(current) is Some(node) else {
    raise InvalidCommandTree("subcommand index \{current} is out of bounds")
  }
  if on_stack.contains(current) {
    raise InvalidCommandTree(
      "command tree contains a cycle at index \{current}",
    )
  }
  if visited.contains(current) {
    return
  }
  visited[current] = ()
  on_stack[current] = ()
  for child in node.subcommands {
    if child < 0 || child >= self.commands.length() {
      raise InvalidCommandTree("subcommand index \{child} is out of bounds")
    }
    self.validate_command_subtree(child, visited, on_stack)
  }
  on_stack.remove(current)
}

///|
fn ToolDef::visit_command(
  self : ToolDef,
  current : Int,
  target : Int,
  path : Array[Int],
  on_stack : Map[Int, Unit],
) -> Bool raise ToolBuildError {
  guard self.commands.get(current) is Some(node) else {
    raise InvalidCommandTree("subcommand index \{current} is out of bounds")
  }
  if on_stack.contains(current) {
    raise InvalidCommandTree(
      "command tree contains a cycle at index \{current}",
    )
  }
  on_stack[current] = ()
  path.push(current)
  if current == target {
    on_stack.remove(current)
    return true
  }
  for child in node.subcommands {
    if child < 0 || child >= self.commands.length() {
      raise InvalidCommandTree("subcommand index \{child} is out of bounds")
    }
    if self.visit_command(child, target, path, on_stack) {
      on_stack.remove(current)
      return true
    }
  }
  ignore(path.pop())
  on_stack.remove(current)
  false
}

///|
pub fn ToolDef::canonical_input_fields(
  self : ToolDef,
  command_index : Int,
) -> Array[CanonicalInputField] raise ToolBuildError {
  let path = self.path_to_command(command_index)
  let command = match self.commands.get(command_index) {
    Some(command) => command
    None =>
      raise InvalidCommandTree("command index \{command_index} is invalid")
  }
  let body_names : Map[String, Unit] = {}
  if command.body is Some(body) {
    for positional in body.positionals.fixed {
      body_names[positional.name] = ()
    }
    if body.positionals.tail is Some(tail) {
      body_names[tail.name] = ()
    }
    for option in body.options {
      add_surface_names(body_names, option.long, option.aliases)
    }
    for flag in body.flags {
      add_surface_names(body_names, flag.long, flag.aliases)
    }
  }
  let fields : Array[CanonicalInputField] = []
  for index in path {
    let node = self.commands[index]
    for option in node.globals.options {
      if !surface_collides(body_names, option.long, option.aliases) {
        fields.push(option.canonical_field())
      }
    }
    for flag in node.globals.flags {
      if !surface_collides(body_names, flag.long, flag.aliases) {
        fields.push(flag.canonical_field())
      }
    }
  }
  if command.body is Some(body) {
    for positional in body.positionals.fixed {
      fields.push({
        name: positional.name,
        aliases: [],
        type_: positional.type_,
      })
    }
    if body.positionals.tail is Some(tail) {
      fields.push({
        name: tail.name,
        aliases: [],
        type_: list_graph(tail.item_type),
      })
    }
    for option in body.options {
      fields.push(option.canonical_field())
    }
    for flag in body.flags {
      fields.push(flag.canonical_field())
    }
  }
  fields
}

///|
fn add_surface_names(
  names : Map[String, Unit],
  name : String,
  aliases : Array[String],
) -> Unit {
  names[name] = ()
  for surface_alias in aliases {
    names[surface_alias] = ()
  }
}

///|
fn surface_collides(
  names : Map[String, Unit],
  name : String,
  aliases : Array[String],
) -> Bool {
  if names.contains(name) {
    return true
  }
  aliases.any(surface_alias => names.contains(surface_alias))
}

///|
fn OptionSpecDef::canonical_field(self : OptionSpecDef) -> CanonicalInputField {
  let type_ = match self.shape {
    Scalar(graph) | OptionalScalar(graph) => graph
    RepeatableList(shape) => list_graph(shape.item_type)
    RepeatableMap(shape) => shape.map_type
  }
  { name: self.long, aliases: self.aliases, type_ }
}

///|
fn @toolCommon.FlagSpec::canonical_field(
  self : @toolCommon.FlagSpec,
) -> CanonicalInputField {
  let root = match self.shape {
    @toolCommon.FlagShape::BoolFlag(_) =>
      @model.schema_type(@model.SchemaTypeBody::Bool)
    @toolCommon.FlagShape::CountFlag(_) =>
      @model.schema_type(@model.SchemaTypeBody::U32(None))
  }
  { name: self.long, aliases: self.aliases, type_: { defs: [], root } }
}

///|
fn list_graph(item : @model.SchemaGraph) -> @model.SchemaGraph {
  {
    defs: item.defs,
    root: @model.schema_type(@model.SchemaTypeBody::List(item.root)),
  }
}

///|
pub fn ToolDef::canonical_input_model(
  self : ToolDef,
  command_index : Int,
) -> CanonicalInputModel raise ToolBuildError {
  let fields = self.canonical_input_fields(command_index)
  let graphs = fields.map(field => field.type_)
  let defs = @model.merge_graph_defs(graphs) catch {
    error => raise SchemaModelError(error.to_string())
  }
  let record_fields = fields.map(field => {
    @model.field(field.name, field.type_.root)
  })
  let record_graph : @model.SchemaGraph = {
    defs,
    root: @model.schema_type(@model.SchemaTypeBody::Record(record_fields)),
  }
  if @model.validate_graph(record_graph) is [error, ..] {
    raise SchemaModelError(error.to_string())
  }
  { fields, record_graph }
}

///|
pub fn decode_canonical_input(
  def : ToolDef,
  command_index : Int,
  input : @types.TypedSchemaValue,
  context : String,
) -> Array[CanonicalInputValue] raise @toolCommon.ToolError {
  let model = def.canonical_input_model(command_index) catch {
    error =>
      raise @toolCommon.ToolError::InvalidInput(
        "\{context}: invalid canonical input model: \{to_repr(error)}",
      )
  }
  let typed = @model.typed_schema_value_from_wit(input) catch {
    error =>
      raise @toolCommon.ToolError::InvalidInput(
        "\{context}: invalid typed schema value input: \{error}",
      )
  }
  if typed.graph != model.record_graph {
    raise @toolCommon.ToolError::InvalidInput(
      "\{context}: input schema does not match the canonical input schema",
    )
  }
  if !value_matches_type(
      model.record_graph,
      model.record_graph.root,
      typed.value,
    ) {
    raise @toolCommon.ToolError::InvalidInput(
      "\{context}: input value does not match the canonical input schema",
    )
  }
  guard typed.value is @model.SchemaValue::Record(values) else {
    raise @toolCommon.ToolError::InvalidInput(
      "\{context}: canonical input must be a record",
    )
  }
  let result : Array[CanonicalInputValue] = []
  for i = 0; i < model.fields.length(); i = i + 1 {
    let field = model.fields[i]
    result.push({
      name: field.name,
      aliases: field.aliases,
      type_: field.type_,
      value: values[i],
    })
  }
  result
}

///|
pub fn[T : @schema.IntoSchema + @schema.FromSchema] decode_canonical_field(
  fields : Array[CanonicalInputValue],
  name : String,
  context : String,
) -> T raise @toolCommon.ToolError {
  for field in fields {
    if field.name == name || field.aliases.contains(name) {
      let tag : @schema.TypeTag[T] = @schema.type_tag()
      let expected = @schema.into_schema_graph(tag) catch {
        error =>
          raise @toolCommon.ToolError::InvalidInput(
            "\{context}: failed building the SDK schema for canonical field `\{name}`: \{error}",
          )
      }
      let value = canonical_value_for_sdk(expected, expected.root, field.value) catch {
        error =>
          raise @toolCommon.ToolError::InvalidInput(
            "\{context}: failed adapting canonical field `\{name}` to its SDK type: \{to_repr(error)}",
          )
      }
      return @schema.from_value_as(value) catch {
        error =>
          raise @toolCommon.ToolError::InvalidInput(
            "\{context}: failed decoding canonical field `\{name}`: \{error}",
          )
      }
    }
  }
  raise @toolCommon.ToolError::InvalidInput(
    "\{context}: canonical field `\{name}` was not found",
  )
}

///|
pub fn build_canonical_input(
  model : CanonicalInputModel,
  params : Array[(String, @model.SchemaValue)],
) -> @model.TypedSchemaValue raise ToolBuildError {
  let values : Array[@model.SchemaValue] = []
  for field in model.fields {
    let mut found = None
    for i = params.length() - 1; i >= 0; i = i - 1 {
      if params[i].0 == field.name {
        found = Some(params[i].1)
        break
      }
    }
    match found {
      Some(value) =>
        values.push(canonical_value_for_wire(field.type_, field.type_.root, value))
      None => raise MissingCanonicalInputField(field.name)
    }
  }
  { graph: model.record_graph, value: @model.SchemaValue::Record(values) }
}

///|
fn canonical_value_for_wire(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
  value : @model.SchemaValue,
) -> @model.SchemaValue raise ToolBuildError {
  let type_ = resolve_literal_type(graph, type_)
  match (type_.body, value) {
    (@model.SchemaTypeBody::Text(_), @model.SchemaValue::String(value)) =>
      @model.SchemaValue::Text(value, None)
    (@model.SchemaTypeBody::List(item), @model.SchemaValue::List(values)) =>
      @model.SchemaValue::List(
        values.map(value => canonical_value_for_wire(graph, item, value)),
      )
    (@model.SchemaTypeBody::FixedList(item, _), @model.SchemaValue::FixedList(values)) =>
      @model.SchemaValue::FixedList(
        values.map(value => canonical_value_for_wire(graph, item, value)),
      )
    (@model.SchemaTypeBody::Option(item), @model.SchemaValue::Option(value)) =>
      @model.SchemaValue::Option(
        value.map(value => canonical_value_for_wire(graph, item, value)),
      )
    (_, value) => value
  }
}

///|
fn canonical_value_for_sdk(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
  value : @model.SchemaValue,
) -> @model.SchemaValue raise ToolBuildError {
  let type_ = resolve_literal_type(graph, type_)
  match (type_.body, value) {
    (@model.SchemaTypeBody::String, @model.SchemaValue::Text(value, _)) =>
      @model.SchemaValue::String(value)
    (@model.SchemaTypeBody::List(item), @model.SchemaValue::List(values)) =>
      @model.SchemaValue::List(
        values.map(value => canonical_value_for_sdk(graph, item, value)),
      )
    (@model.SchemaTypeBody::FixedList(item, _), @model.SchemaValue::FixedList(values)) =>
      @model.SchemaValue::FixedList(
        values.map(value => canonical_value_for_sdk(graph, item, value)),
      )
    (@model.SchemaTypeBody::Option(item), @model.SchemaValue::Option(value)) =>
      @model.SchemaValue::Option(
        value.map(value => canonical_value_for_sdk(graph, item, value)),
      )
    (_, value) => value
  }
}

///|
pub fn forward_canonical_input(
  fields : Array[CanonicalInputValue],
  def : ToolDef,
  command_index : Int,
) -> @types.TypedSchemaValue raise @toolCommon.ToolError {
  let model = def.canonical_input_model(command_index) catch {
    error =>
      raise @toolCommon.ToolError::InvalidInput(
        "invalid forwarded canonical input model: \{to_repr(error)}",
      )
  }
  let params : Array[(String, @model.SchemaValue)] = []
  for target in model.fields {
    let mut found : CanonicalInputValue? = None
    for candidate in fields {
      if candidate.name == target.name ||
        candidate.aliases.contains(target.name) ||
        target.aliases.any(surface_alias => {
          candidate.name == surface_alias ||
          candidate.aliases.contains(surface_alias)
        }) {
        found = Some(candidate)
        break
      }
    }
    guard found is Some(found) else {
      raise @toolCommon.ToolError::InvalidInput(
        "missing forwarded canonical input field `\{target.name}`",
      )
    }
    if found.type_ != target.type_ {
      raise @toolCommon.ToolError::InvalidInput(
        "canonical input field `\{found.name}` has an incompatible schema for forwarded field `\{target.name}`",
      )
    }
    params.push((target.name, found.value))
  }
  let input = build_canonical_input(model, params) catch {
    error =>
      raise @toolCommon.ToolError::InvalidInput(
        "failed to build forwarded canonical input: \{to_repr(error)}",
      )
  }
  @model.typed_schema_value_to_wit(input) catch {
    error =>
      raise @toolCommon.ToolError::InvalidInput(
        "failed to encode forwarded canonical input: \{error}",
      )
  }
}
