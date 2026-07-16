///|
pub suberror ToolBuildError {
  EmptyCommandTree
  CommandNotFound(String)
  InvalidCommandTree(String)
  SubtreeRootNameMismatch(String, String)
  InheritedGlobalConflict(String, String)
  MissingCanonicalInputField(String)
  SchemaModelError(String)
  LiteralTypeMismatch(String)
  InvalidLiteral(String)
  RefinementTypeMismatch(String, String)
  InvalidNumericBound(String)
  ValueIsTypeMismatch(String)
  UnresolvedValueIsLiteral(String)
} derive(Eq, Debug)

///|
pub(all) struct ToolDef {
  version : String
  commands : Array[CommandNodeDef]
}

///|
pub(all) struct CommandNodeDef {
  name : String
  aliases : Array[String]
  doc : @toolCommon.Doc
  globals : GlobalsDef
  subcommands : Array[Int]
  body : CommandBodyDef?
}

///|
pub(all) struct GlobalsDef {
  options : Array[OptionSpecDef]
  flags : Array[@toolCommon.FlagSpec]
}

///|
pub(all) struct CommandBodyDef {
  positionals : PositionalsDef
  options : Array[OptionSpecDef]
  flags : Array[@toolCommon.FlagSpec]
  constraints : Array[ConstraintDef]
  stdin : @toolCommon.StreamSpec?
  stdout : @toolCommon.StreamSpec?
  result : ResultSpecDef?
  errors : Array[ErrorCaseDef]
  annotations : @toolCommon.CommandAnnotations?
}

///|
pub(all) struct PositionalsDef {
  fixed : Array[PositionalDef]
  tail : TailPositionalDef?
}

///|
pub(all) struct PositionalDef {
  name : String
  doc : @toolCommon.Doc
  value_name : String?
  type_ : @model.SchemaGraph
  default : @model.SchemaValue?
  required : Bool
  accepts_stdio : Bool
}

///|
pub(all) struct TailPositionalDef {
  name : String
  doc : @toolCommon.Doc
  value_name : String?
  item_type : @model.SchemaGraph
  min : UInt
  max : UInt?
  separator : String?
  verbatim : Bool
  accepts_stdio : Bool
}

///|
pub(all) struct OptionSpecDef {
  long : String
  short : Char?
  aliases : Array[String]
  doc : @toolCommon.Doc
  value_name : String?
  shape : OptionShapeDef
  default : @model.SchemaValue?
  required : Bool
  env_var : String?
}

///|
pub(all) enum OptionShapeDef {
  Scalar(@model.SchemaGraph)
  OptionalScalar(@model.SchemaGraph)
  RepeatableList(RepeatableListShapeDef)
  RepeatableMap(RepeatableMapShapeDef)
}

///|
pub(all) struct RepeatableListShapeDef {
  repetition : @toolCommon.Repetition
  item_type : @model.SchemaGraph
}

///|
pub(all) struct RepeatableMapShapeDef {
  repetition : @toolCommon.Repetition
  map_type : @model.SchemaGraph
  duplicate_key_policy : @toolCommon.DuplicateKeyPolicy
}

///|
pub(all) enum RefDef {
  Present(String)
  ValueIs(ValueIsRefDef)
}

///|
pub(all) struct ValueIsRefDef {
  name : String
  value : ValueIsLiteralDef
}

///|
pub(all) enum ValueIsLiteralDef {
  Resolved(@model.SchemaValue)
  Deferred(ToolLiteral)
}

///|
pub(all) struct RefGroupDef {
  refs : Array[RefDef]
}

///|
pub(all) struct ImpliesDef {
  lhs_quant : @toolCommon.Quantifier
  lhs : Array[RefDef]
  rhs_quant : @toolCommon.Quantifier
  rhs : Array[RefDef]
}

///|
pub(all) struct ForbidsDef {
  lhs_quant : @toolCommon.Quantifier
  lhs : Array[RefDef]
  rhs : Array[RefDef]
}

///|
pub(all) enum ConstraintDef {
  RequiresAll(Array[RefDef])
  AllOrNone(Array[RefDef])
  RequiresAny(Array[RefDef])
  MutexGroups(Array[RefGroupDef])
  Implies(ImpliesDef)
  Forbids(ForbidsDef)
}

///|
pub(all) struct ResultSpecDef {
  type_ : @model.SchemaGraph
  doc : @toolCommon.Doc
  formatters : Array[@toolCommon.Formatter]
  default_formatter : String
}

///|
pub(all) struct ErrorCaseDef {
  name : String
  doc : @toolCommon.Doc
  kind : @toolCommon.ErrorKind
  exit_code : Byte
  payload : @model.SchemaGraph?
}

///|
/// Describes and converts a typed custom tool error. Implementations are
/// generated for error enums referenced by tool commands.
pub(open) trait ToolErrorSchema {
  fn error_cases() -> Result[Array[ErrorCaseDef], ToolBuildError]
  fn to_error_payload_value(Self) -> Result[@model.TypedSchemaValue, String]
  fn from_error_payload_value(@model.TypedSchemaValue) -> Result[Self, String]
}

///|
pub fn ToolBuildError::schema_model(message : String) -> ToolBuildError {
  SchemaModelError(message)
}

///|
pub fn ToolDef::tool_name(self : ToolDef) -> Result[String, ToolBuildError] {
  match self.commands.get(0) {
    Some(root) => Ok(root.name)
    None => Err(EmptyCommandTree)
  }
}

///|
pub fn tool_name(tool : @toolCommon.Tool) -> Result[String, ToolBuildError] {
  match tool.commands.nodes.get(0) {
    Some(root) => Ok(root.name)
    None => Err(EmptyCommandTree)
  }
}

///|
pub fn ToolDef::to_tool(
  self : ToolDef,
) -> @toolCommon.Tool raise ToolBuildError {
  guard self.commands.length() > 0 else { raise EmptyCommandTree }
  let defs = @model.merge_graph_defs(self.collect_graphs()) catch {
    e => raise SchemaModelError(e.to_string())
  }
  let encoder = @model.GraphEncoder::new(defs) catch {
    e => raise SchemaModelError(e.to_string())
  }
  let nodes : Array[@toolCommon.CommandNode] = []
  for command in self.commands {
    nodes.push(command.to_command_node(encoder))
  }
  let schema = encoder.finish() catch {
    e => raise SchemaModelError(e.to_string())
  }
  {
    version: self.version,
    commands: @toolCommon.CommandTree::{ nodes, },
    schema,
  }
}

///|
fn ToolDef::collect_graphs(self : ToolDef) -> Array[@model.SchemaGraph] {
  let graphs : Array[@model.SchemaGraph] = []
  for command in self.commands {
    command.collect_graphs(graphs)
  }
  graphs
}

///|
fn CommandNodeDef::collect_graphs(
  self : CommandNodeDef,
  graphs : Array[@model.SchemaGraph],
) -> Unit {
  self.globals.collect_graphs(graphs)
  if self.body is Some(body) {
    body.collect_graphs(graphs)
  }
}

///|
fn GlobalsDef::collect_graphs(
  self : GlobalsDef,
  graphs : Array[@model.SchemaGraph],
) -> Unit {
  for option in self.options {
    option.collect_graphs(graphs)
  }
}

///|
fn CommandBodyDef::collect_graphs(
  self : CommandBodyDef,
  graphs : Array[@model.SchemaGraph],
) -> Unit {
  for positional in self.positionals.fixed {
    graphs.push(positional.type_)
  }
  if self.positionals.tail is Some(tail) {
    graphs.push(tail.item_type)
  }
  for option in self.options {
    option.collect_graphs(graphs)
  }
  if self.result is Some(result) {
    graphs.push(result.type_)
  }
  for error in self.errors {
    if error.payload is Some(payload) {
      graphs.push(payload)
    }
  }
}

///|
fn OptionSpecDef::collect_graphs(
  self : OptionSpecDef,
  graphs : Array[@model.SchemaGraph],
) -> Unit {
  match self.shape {
    Scalar(g) | OptionalScalar(g) => graphs.push(g)
    RepeatableList(shape) => graphs.push(shape.item_type)
    RepeatableMap(shape) => graphs.push(shape.map_type)
  }
}

///|
fn CommandNodeDef::to_command_node(
  self : CommandNodeDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.CommandNode raise ToolBuildError {
  {
    name: self.name,
    aliases: self.aliases,
    doc: self.doc,
    globals: self.globals.to_globals(encoder),
    subcommands: self.subcommands,
    body: match self.body {
      Some(body) => Some(body.to_command_body(encoder))
      None => None
    },
  }
}

///|
fn GlobalsDef::to_globals(
  self : GlobalsDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.Globals raise ToolBuildError {
  let options : Array[@toolCommon.OptionSpec] = []
  for option in self.options {
    options.push(option.to_option_spec(encoder))
  }
  { options, flags: self.flags }
}

///|
fn CommandBodyDef::to_command_body(
  self : CommandBodyDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.CommandBody raise ToolBuildError {
  let options : Array[@toolCommon.OptionSpec] = []
  for option in self.options {
    options.push(option.to_option_spec(encoder))
  }
  let errors : Array[@toolCommon.ErrorCase] = []
  for error in self.errors {
    errors.push(error.to_error_case(encoder))
  }
  {
    positionals: self.positionals.to_positionals(encoder),
    options,
    flags: self.flags,
    constraints: {
      let constraints : Array[@toolCommon.Constraint] = []
      for constraint in self.constraints {
        constraints.push(constraint.to_constraint())
      }
      constraints
    },
    stdin: self.stdin,
    stdout: self.stdout,
    result: match self.result {
      Some(result) => Some(result.to_result_spec(encoder))
      None => None
    },
    errors,
    annotations: self.annotations,
  }
}

///|
fn PositionalsDef::to_positionals(
  self : PositionalsDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.Positionals raise ToolBuildError {
  let fixed : Array[@toolCommon.Positional] = []
  for positional in self.fixed {
    fixed.push(positional.to_positional(encoder))
  }
  {
    fixed,
    tail: match self.tail {
      Some(tail) => Some(tail.to_tail_positional(encoder))
      None => None
    },
  }
}

///|
fn PositionalDef::to_positional(
  self : PositionalDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.Positional raise ToolBuildError {
  {
    name: self.name,
    doc: self.doc,
    value_name: self.value_name,
    type_: encode_graph_root(encoder, self.type_),
    default: encode_default(self.default),
    required: self.required,
    accepts_stdio: self.accepts_stdio,
  }
}

///|
fn TailPositionalDef::to_tail_positional(
  self : TailPositionalDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.TailPositional raise ToolBuildError {
  {
    name: self.name,
    doc: self.doc,
    value_name: self.value_name,
    item_type: encode_graph_root(encoder, self.item_type),
    min: self.min,
    max: self.max,
    separator: self.separator,
    verbatim: self.verbatim,
    accepts_stdio: self.accepts_stdio,
  }
}

///|
fn OptionSpecDef::to_option_spec(
  self : OptionSpecDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.OptionSpec raise ToolBuildError {
  {
    long: self.long,
    short: self.short,
    aliases: self.aliases,
    doc: self.doc,
    value_name: self.value_name,
    shape: self.shape.to_option_shape(encoder),
    default: encode_default(self.default),
    required: self.required,
    env_var: self.env_var,
  }
}

///|
fn OptionShapeDef::to_option_shape(
  self : OptionShapeDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.OptionShape raise ToolBuildError {
  match self {
    Scalar(g) => @toolCommon.OptionShape::Scalar(encode_graph_root(encoder, g))
    OptionalScalar(g) =>
      @toolCommon.OptionShape::OptionalScalar(encode_graph_root(encoder, g))
    RepeatableList(shape) =>
      @toolCommon.OptionShape::RepeatableList({
        repetition: shape.repetition,
        item_type: encode_graph_root(encoder, shape.item_type),
      })
    RepeatableMap(shape) =>
      @toolCommon.OptionShape::RepeatableMap({
        repetition: shape.repetition,
        map_type: encode_graph_root(encoder, shape.map_type),
        duplicate_key_policy: shape.duplicate_key_policy,
      })
  }
}

///|
fn ConstraintDef::to_constraint(
  self : ConstraintDef,
) -> @toolCommon.Constraint raise ToolBuildError {
  match self {
    RequiresAll(refs) => @toolCommon.Constraint::RequiresAll(refs_to_wit(refs))
    AllOrNone(refs) => @toolCommon.Constraint::AllOrNone(refs_to_wit(refs))
    RequiresAny(refs) => @toolCommon.Constraint::RequiresAny(refs_to_wit(refs))
    MutexGroups(groups) => {
      let wit_groups : Array[@toolCommon.RefGroup] = []
      for group in groups {
        wit_groups.push(@toolCommon.RefGroup::{ refs: refs_to_wit(group.refs) })
      }
      @toolCommon.Constraint::MutexGroups(wit_groups)
    }
    Implies(impl_) =>
      @toolCommon.Constraint::Implies({
        lhs_quant: impl_.lhs_quant,
        lhs: refs_to_wit(impl_.lhs),
        rhs_quant: impl_.rhs_quant,
        rhs: refs_to_wit(impl_.rhs),
      })
    Forbids(forbids) =>
      @toolCommon.Constraint::Forbids({
        lhs_quant: forbids.lhs_quant,
        lhs: refs_to_wit(forbids.lhs),
        rhs: refs_to_wit(forbids.rhs),
      })
  }
}

///|
fn refs_to_wit(
  refs : Array[RefDef],
) -> Array[@toolCommon.Ref] raise ToolBuildError {
  let result : Array[@toolCommon.Ref] = []
  for ref_ in refs {
    result.push(ref_.to_ref())
  }
  result
}

///|
fn RefDef::to_ref(self : RefDef) -> @toolCommon.Ref raise ToolBuildError {
  match self {
    Present(name) => @toolCommon.Ref::Present(name)
    ValueIs(value_is) =>
      @toolCommon.Ref::ValueIs({
        name: value_is.name,
        value: match value_is.value {
          Resolved(value) => schema_value_to_wit(value)
          Deferred(_) => raise UnresolvedValueIsLiteral(value_is.name)
        },
      })
  }
}

///|
fn ResultSpecDef::to_result_spec(
  self : ResultSpecDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.ResultSpec raise ToolBuildError {
  {
    type_: encode_graph_root(encoder, self.type_),
    doc: self.doc,
    formatters: self.formatters,
    default_formatter: self.default_formatter,
  }
}

///|
fn ErrorCaseDef::to_error_case(
  self : ErrorCaseDef,
  encoder : @model.GraphEncoder,
) -> @toolCommon.ErrorCase raise ToolBuildError {
  {
    name: self.name,
    doc: self.doc,
    kind: self.kind,
    exit_code: self.exit_code,
    payload: match self.payload {
      Some(payload) => Some(encode_graph_root(encoder, payload))
      None => None
    },
  }
}

///|
fn encode_graph_root(
  encoder : @model.GraphEncoder,
  graph : @model.SchemaGraph,
) -> Int raise ToolBuildError {
  encoder.encode_type(graph.root) catch {
    e => raise SchemaModelError(e.to_string())
  }
}

///|
fn encode_default(
  value : @model.SchemaValue?,
) -> @types.SchemaValueTree? raise ToolBuildError {
  match value {
    Some(value) => Some(schema_value_to_wit(value))
    None => None
  }
}

///|
fn schema_value_to_wit(
  value : @model.SchemaValue,
) -> @types.SchemaValueTree raise ToolBuildError {
  @model.schema_value_to_wit(value) catch {
    e => raise SchemaModelError(e.to_string())
  }
}
