///|
priv enum EffectiveGlobal {
  Option(OptionSpecDef)
  Flag(@toolCommon.FlagSpec)
}

///|
priv enum ValueIsComparand {
  Typed(String, @model.SchemaGraph)
  Flag
}

///|
fn[T] append_arrays(left : Array[T], right : Array[T]) -> Array[T] {
  let result = left.copy()
  for value in right {
    result.push(value)
  }
  result
}

///|
fn option_collected_graph(option : OptionSpecDef) -> @model.SchemaGraph {
  match option.shape {
    Scalar(graph) | OptionalScalar(graph) => graph
    RepeatableList(shape) =>
      {
        defs: shape.item_type.defs,
        root: @model.schema_type(
          @model.SchemaTypeBody::List(shape.item_type.root),
        ),
      }
    RepeatableMap(shape) => shape.map_type
  }
}

///|
fn tail_collected_graph(tail : TailPositionalDef) -> @model.SchemaGraph {
  {
    defs: tail.item_type.defs,
    root: @model.schema_type(@model.SchemaTypeBody::List(tail.item_type.root)),
  }
}

///|
fn surface_names(
  name : String,
  aliases : Array[String],
  short : Char?,
) -> Array[String] {
  let names = [name]
  for surface_alias in aliases {
    names.push(surface_alias)
  }
  if short is Some(short) {
    names.push(short.to_string())
  }
  names
}

///|
fn surfaces_overlap(
  left_name : String,
  left_aliases : Array[String],
  left_short : Char?,
  right_name : String,
  right_aliases : Array[String],
  right_short : Char?,
) -> Bool {
  let right = surface_names(right_name, right_aliases, right_short)
  surface_names(left_name, left_aliases, left_short).any(name => {
    right.contains(name)
  })
}

///|
fn inherited_matches(
  inherited : EffectiveGlobal,
  name : String,
  aliases : Array[String],
  short : Char?,
) -> Bool {
  match inherited {
    Option(option) =>
      surfaces_overlap(
        option.long,
        option.aliases,
        option.short,
        name,
        aliases,
        short,
      )
    Flag(flag) =>
      surfaces_overlap(
        flag.long,
        flag.aliases,
        flag.short,
        name,
        aliases,
        short,
      )
  }
}

///|
fn compatible_option(
  inherited : EffectiveGlobal,
  graph : @model.SchemaGraph,
) -> Bool {
  match inherited {
    Option(option) => option_collected_graph(option) == graph
    Flag(_) => false
  }
}

///|
fn compatible_flag(
  inherited : EffectiveGlobal,
  flag : @toolCommon.FlagSpec,
) -> Bool {
  match (inherited, flag.shape) {
    (Flag({ shape: @toolCommon.FlagShape::BoolFlag(_), .. }), BoolFlag(_)) =>
      true
    (Flag({ shape: @toolCommon.FlagShape::CountFlag(_), .. }), CountFlag(_)) =>
      true
    _ => false
  }
}

///|
fn matching_inherited(
  inherited : Array[EffectiveGlobal],
  name : String,
  aliases : Array[String],
  short : Char?,
) -> Array[EffectiveGlobal] {
  inherited.filter(global => inherited_matches(global, name, aliases, short))
}

///|
fn conflict(name : String, command : String) -> Unit raise ToolBuildError {
  raise InheritedGlobalConflict(name, command)
}

///|
fn reconcile_globals(
  globals : GlobalsDef,
  inherited : Array[EffectiveGlobal],
  command : String,
) -> GlobalsDef raise ToolBuildError {
  let options : Array[OptionSpecDef] = []
  for option in globals.options {
    match
      matching_inherited(inherited, option.long, option.aliases, option.short) {
      [global] =>
        if !compatible_option(global, option_collected_graph(option)) {
          conflict(option.long, command)
        }
      [] => options.push(option)
      _ => conflict(option.long, command)
    }
  }
  let flags : Array[@toolCommon.FlagSpec] = []
  for flag in globals.flags {
    match matching_inherited(inherited, flag.long, flag.aliases, flag.short) {
      [global] =>
        if !compatible_flag(global, flag) {
          conflict(flag.long, command)
        }
      [] => flags.push(flag)
      _ => conflict(flag.long, command)
    }
  }
  { options, flags }
}

///|
fn reconcile_body(
  body : CommandBodyDef,
  inherited : Array[EffectiveGlobal],
  command : String,
) -> CommandBodyDef raise ToolBuildError {
  let fixed : Array[PositionalDef] = []
  for positional in body.positionals.fixed {
    match matching_inherited(inherited, positional.name, [], None) {
      [global] =>
        if !compatible_option(global, positional.type_) {
          conflict(positional.name, command)
        }
      [] => fixed.push(positional)
      _ => conflict(positional.name, command)
    }
  }
  let tail = match body.positionals.tail {
    Some(tail) =>
      match matching_inherited(inherited, tail.name, [], None) {
        [global] => {
          if !compatible_option(global, tail_collected_graph(tail)) {
            conflict(tail.name, command)
          }
          None
        }
        [] => Some(tail)
        _ => {
          conflict(tail.name, command)
          None
        }
      }
    None => None
  }
  let options : Array[OptionSpecDef] = []
  for option in body.options {
    match
      matching_inherited(inherited, option.long, option.aliases, option.short) {
      [global] =>
        if !compatible_option(global, option_collected_graph(option)) {
          conflict(option.long, command)
        }
      [] => options.push(option)
      _ => conflict(option.long, command)
    }
  }
  let flags : Array[@toolCommon.FlagSpec] = []
  for flag in body.flags {
    match matching_inherited(inherited, flag.long, flag.aliases, flag.short) {
      [global] =>
        if !compatible_flag(global, flag) {
          conflict(flag.long, command)
        }
      [] => flags.push(flag)
      _ => conflict(flag.long, command)
    }
  }
  { ..body, positionals: { fixed, tail }, options, flags }
}

///|
fn globals_as_effective(globals : GlobalsDef) -> Array[EffectiveGlobal] {
  let result : Array[EffectiveGlobal] = []
  for option in globals.options {
    result.push(Option(option))
  }
  for flag in globals.flags {
    result.push(Flag(flag))
  }
  result
}

///|
fn register_value_is_surface(
  scope : Map[String, ValueIsComparand],
  name : String,
  aliases : Array[String],
  short : Char?,
  comparand : ValueIsComparand,
) -> Unit {
  scope[name] = comparand
  for surface_alias in aliases {
    scope[surface_alias] = comparand
  }
  if short is Some(short) {
    scope[short.to_string()] = comparand
  }
}

///|
fn register_value_is_global(
  scope : Map[String, ValueIsComparand],
  global : EffectiveGlobal,
) -> Unit {
  match global {
    Option(option) =>
      register_value_is_surface(
        scope,
        option.long,
        option.aliases,
        option.short,
        Typed(option.long, option_collected_graph(option)),
      )
    Flag(flag) =>
      register_value_is_surface(
        scope,
        flag.long,
        flag.aliases,
        flag.short,
        Flag,
      )
  }
}

///|
fn value_is_scope(
  inherited : Array[EffectiveGlobal],
  globals : GlobalsDef,
  body : CommandBodyDef,
) -> Map[String, ValueIsComparand] {
  let scope : Map[String, ValueIsComparand] = {}
  for global in inherited {
    register_value_is_global(scope, global)
  }
  for global in globals_as_effective(globals) {
    register_value_is_global(scope, global)
  }
  for positional in body.positionals.fixed {
    register_value_is_surface(
      scope,
      positional.name,
      [],
      None,
      Typed(positional.name, positional.type_),
    )
  }
  if body.positionals.tail is Some(tail) {
    register_value_is_surface(
      scope,
      tail.name,
      [],
      None,
      Typed(tail.name, tail_collected_graph(tail)),
    )
  }
  for option in body.options {
    register_value_is_surface(
      scope,
      option.long,
      option.aliases,
      option.short,
      Typed(option.long, option_collected_graph(option)),
    )
  }
  for flag in body.flags {
    register_value_is_surface(scope, flag.long, flag.aliases, flag.short, Flag)
  }
  scope
}

///|
fn resolve_value_is_ref(
  ref_ : RefDef,
  scope : Map[String, ValueIsComparand],
) -> RefDef raise ToolBuildError {
  match ref_ {
    ValueIs({ name, value: Deferred(literal) }) =>
      match scope.get(name) {
        Some(Typed(canonical_name, graph)) => {
          let value = value_is_literal_to_schema_value(graph, literal) catch {
            _ => raise ValueIsTypeMismatch(name)
          }
          ValueIs({ name: canonical_name, value: Resolved(value) })
        }
        Some(Flag) => raise ValueIsTypeMismatch(name)
        None => ref_
      }
    _ => ref_
  }
}

///|
fn resolve_value_is_refs(
  refs : Array[RefDef],
  scope : Map[String, ValueIsComparand],
) -> Array[RefDef] raise ToolBuildError {
  refs.map(ref_ => resolve_value_is_ref(ref_, scope))
}

///|
fn resolve_value_is_constraint(
  constraint : ConstraintDef,
  scope : Map[String, ValueIsComparand],
) -> ConstraintDef raise ToolBuildError {
  match constraint {
    RequiresAll(refs) => RequiresAll(resolve_value_is_refs(refs, scope))
    AllOrNone(refs) => AllOrNone(resolve_value_is_refs(refs, scope))
    RequiresAny(refs) => RequiresAny(resolve_value_is_refs(refs, scope))
    MutexGroups(groups) =>
      MutexGroups(
        groups.map(group => { refs: resolve_value_is_refs(group.refs, scope) }),
      )
    Implies(implies) =>
      Implies({
        ..implies,
        lhs: resolve_value_is_refs(implies.lhs, scope),
        rhs: resolve_value_is_refs(implies.rhs, scope),
      })
    Forbids(forbids) =>
      Forbids({
        ..forbids,
        lhs: resolve_value_is_refs(forbids.lhs, scope),
        rhs: resolve_value_is_refs(forbids.rhs, scope),
      })
  }
}

///|
fn resolve_value_is_body(
  body : CommandBodyDef,
  inherited : Array[EffectiveGlobal],
  globals : GlobalsDef,
) -> CommandBodyDef raise ToolBuildError {
  let scope = value_is_scope(inherited, globals, body)
  {
    ..body,
    constraints: body.constraints.map(constraint => {
      resolve_value_is_constraint(constraint, scope)
    }),
  }
}

///|
fn normalize_command(
  commands : Array[CommandNodeDef],
  index : Int,
  inherited : Array[EffectiveGlobal],
  visited : Map[Int, Unit],
  stack : Map[Int, Unit],
) -> Unit raise ToolBuildError {
  guard index >= 0 && index < commands.length() else {
    raise InvalidCommandTree("command index \{index} is out of bounds")
  }
  if stack.contains(index) {
    raise InvalidCommandTree("command tree contains a cycle at index \{index}")
  }
  if visited.contains(index) {
    raise InvalidCommandTree("command index \{index} has more than one parent")
  }
  visited[index] = ()
  stack[index] = ()
  let node = commands[index]
  let globals = reconcile_globals(node.globals, inherited, node.name)
  let body = match node.body {
    Some(body) => {
      let body = reconcile_body(body, inherited, node.name)
      Some(resolve_value_is_body(body, inherited, globals))
    }
    None => None
  }
  commands[index] = { ..node, globals, body }
  let child_inherited = inherited.copy()
  for global in globals_as_effective(globals) {
    child_inherited.push(global)
  }
  for child in node.subcommands {
    normalize_command(commands, child, child_inherited, visited, stack)
  }
  stack.remove(index)
}

///|
pub fn ToolDef::normalize_inherited_globals(
  self : ToolDef,
) -> ToolDef raise ToolBuildError {
  guard self.commands.length() > 0 else { raise EmptyCommandTree }
  let commands = self.commands.copy()
  normalize_command(commands, 0, [], {}, {})
  { ..self, commands, }
}

///|
fn shift_node(node : CommandNodeDef, offset : Int) -> CommandNodeDef {
  { ..node, subcommands: node.subcommands.map(index => index + offset) }
}

///|
pub fn ToolDef::graft_subtree(
  self : ToolDef,
  child : ToolDef,
  expected_name : String,
  parent_globals : GlobalsDef,
  override_name : String?,
  override_doc : @toolCommon.Doc?,
  override_aliases : Array[String]?,
) -> ToolDef raise ToolBuildError {
  guard self.commands.length() > 0 && child.commands.length() > 0 else {
    raise EmptyCommandTree
  }
  let child_commands = child.commands.copy()
  let child_root = child_commands[0]
  if override_name is None && child_root.name != expected_name {
    raise SubtreeRootNameMismatch(expected_name, child_root.name)
  }
  let graft_name = override_name.unwrap_or(child_root.name)
  let inherited = globals_as_effective(parent_globals)
  let child_globals = reconcile_globals(
    child_root.globals,
    inherited,
    graft_name,
  )
  let child_body = match child_root.body {
    Some(body) => Some(reconcile_body(body, inherited, graft_name))
    None => None
  }
  child_commands[0] = {
    ..child_root,
    name: graft_name,
    aliases: override_aliases.unwrap_or(child_root.aliases),
    doc: override_doc.unwrap_or(child_root.doc),
    globals: {
      options: append_arrays(parent_globals.options, child_globals.options),
      flags: append_arrays(parent_globals.flags, child_globals.flags),
    },
    body: child_body,
  }
  let commands = self.commands.copy()
  let offset = commands.length()
  for node in child_commands {
    commands.push(shift_node(node, offset))
  }
  let root = commands[0]
  let subcommands = root.subcommands.copy()
  subcommands.push(offset)
  commands[0] = { ..root, subcommands, }
  { ..self, commands, }
}
