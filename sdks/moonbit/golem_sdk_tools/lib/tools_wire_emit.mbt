///|
fn emit_wire_constraints(
  command : ToolCommandInfo,
  fields : Array[ProjectedToolArg],
) -> String {
  let present = name => {
    "@toolCommon.Ref::Present(\"" + escape_string_lit(name) + "\")"
  }
  let value_is = (authored : String) => {
    let split = authored.find("=").unwrap()
    let name = authored[:split].to_owned()
    let token = authored[split + 1:].to_owned()
    let field = fields
      .iter()
      .find_first(arg => projected_arg_identity_names(arg).contains(name))
      .unwrap()
    "@toolCommon.Ref::ValueIs({ name: \"" +
    escape_string_lit(name) +
    "\", value: @tool.wire_value_is_literal(" +
    refined_graph(field, wire=true) +
    ", @tool.ToolLiteral::Scalar(\"" +
    escape_string_lit(token) +
    "\")) })"
  }
  let refs = (names : Array[String], values : Array[String]) => {
    "[" + names.map(present).concat(values.map(value_is)).join(", ") + "]"
  }
  let constraints = command.constraints.map(c => {
    let prefix = "@toolCommon.Constraint::"
    match c.kind {
      "requires_all" => prefix + "RequiresAll(" + refs(c.refs, c.value_is) + ")"
      "requires_any" => prefix + "RequiresAny(" + refs(c.refs, c.value_is) + ")"
      "all_or_none" => prefix + "AllOrNone(" + refs(c.refs, c.value_is) + ")"
      "mutex_groups" =>
        prefix +
        "MutexGroups([" +
        c.refs
        .map(present)
        .concat(c.value_is.map(value_is))
        .map(r => "{ refs: [" + r + "] }")
        .join(", ") +
        "])"
      "implies" =>
        prefix +
        "Implies({ lhs_quant: @toolCommon.Quantifier::" +
        (if c.lhs_quant == Some("any") { "ANY" } else { "ALL" }) +
        ", lhs: " +
        refs(c.lhs, c.lhs_value_is) +
        ", rhs_quant: @toolCommon.Quantifier::" +
        (if c.rhs_quant == Some("any") { "ANY" } else { "ALL" }) +
        ", rhs: " +
        refs(c.rhs, c.rhs_value_is) +
        " })"
      "forbids" =>
        prefix +
        "Forbids({ lhs_quant: @toolCommon.Quantifier::" +
        (if c.lhs_quant == Some("any") { "ANY" } else { "ALL" }) +
        ", lhs: " +
        refs(c.lhs, c.lhs_value_is) +
        ", rhs: " +
        refs(c.rhs, c.rhs_value_is) +
        " })"
      _ => abort("constraint kind validated by parser")
    }
  })
  "[" + constraints.join(", ") + "]"
}

///|
fn emit_wire_globals(args : Array[ProjectedToolArg]) -> String {
  let options = args
    .filter(arg => arg.placement is GlobalOption)
    .map(arg => emit_option(arg, wire=true))
  let flags = args.filter(arg => arg.placement is GlobalFlag).map(emit_flag)
  "{ options: [" + options.join(", ") + "], flags: [" + flags.join(", ") + "] }"
}

///|
fn emit_wire_tool_node(
  nodes : Array[String],
  tool : ToolInfo,
  tools : Array[ToolInfo],
  ancestors : Array[ProjectedToolArg],
  mount : ToolCommandInfo?,
) -> Int raise ToolEmitError {
  let index = nodes.length()
  nodes.push("")
  let plans = tool.commands.map(command => {
    project_command(command, command.subtree is Some(_))
  })
  let root = tool.root_command
  let root_args = root.map(i => plans[i]).unwrap_or([])
  let mount_globals = without_captured_args(
    mount.map(command => project_command(command, true)).unwrap_or([]),
    projected_identity_names(ancestors),
  )
  let root_ancestors = ancestors.concat(mount_globals)
  let local_root = without_captured_args(
    root_args,
    projected_identity_names(root_ancestors),
  )
  let globals = mount_globals.concat(
    local_root
    .filter(arg => arg.placement is GlobalOption)
    .concat(local_root.filter(arg => arg.placement is GlobalFlag)),
  )
  let inherited = ancestors.concat(globals)
  let children : Array[String] = []
  for command_index, command in tool.commands {
    if Some(command_index) == root {
      continue
    }
    if command.subtree is Some(target) {
      children.push(
        emit_wire_tool_node(
          nodes,
          resolve_subtree_tool(tools, target),
          tools,
          inherited,
          Some(command),
        ).to_string(),
      )
    } else {
      children.push(nodes.length().to_string())
      let args = without_captured_args(
        plans[command_index],
        projected_identity_names(inherited),
      )
      nodes.push(
        "{ name: \"" +
        escape_string_lit(command.name) +
        "\", aliases: " +
        emit_strings(command.aliases) +
        ", doc: " +
        emit_doc(command.description, command.examples) +
        ", globals: " +
        emit_wire_globals(args) +
        ", subcommands: [], body: " +
        emit_body(
          command,
          args,
          wire_fields=direct_input_args(plans[command_index], inherited),
        ) +
        " }",
      )
    }
  }
  let name = match mount {
    Some(command) if command.explicit_name => command.name
    _ => tool.name
  }
  let aliases = match mount {
    Some(command) if !command.aliases.is_empty() => command.aliases
    _ => root.map(i => tool.commands[i].aliases).unwrap_or([])
  }
  let description = match mount {
    Some(command) if !command.description.is_empty() => command.description
    _ => tool.description
  }
  let examples = match mount {
    Some(command) if !command.examples.is_empty() => command.examples
    _ => tool.examples
  }
  let body = root
    .map(i => {
      emit_body(
        tool.commands[i],
        local_root,
        wire_fields=direct_input_args(root_args, root_ancestors),
      )
    })
    .unwrap_or("None")
  nodes[index] = "{ name: \"" +
    escape_string_lit(name) +
    "\", aliases: " +
    emit_strings(aliases) +
    ", doc: " +
    emit_doc(description, examples) +
    ", globals: " +
    emit_wire_globals(globals) +
    ", subcommands: [" +
    children.join(", ") +
    "], body: " +
    body +
    " }"
  index
}

///|
fn emit_wire_tool_descriptor(
  buf : StringBuilder,
  tool : ToolInfo,
  tools : Array[ToolInfo],
) -> Unit raise ToolEmitError {
  let nodes : Array[String] = []
  ignore(emit_wire_tool_node(nodes, tool, tools, [], None))
  buf.write_string(
    "///|\nfn __golem_tool_wire_" +
    tool.source_name +
    "() -> @toolCommon.Tool {\n  try {\n    let __golem_schema = @schema.WireTypeEncoder::new()\n    let nodes : Array[@toolCommon.CommandNode] = [" +
    nodes.join(",\n") +
    "]\n    let root = __golem_schema.push(@types.RecordType([]))\n    { version: \"" +
    escape_string_lit(tool.version.unwrap_or("0.0.0")) +
    "\", commands: { nodes }, schema: { type_nodes: __golem_schema.nodes, defs: __golem_schema.defs, root } }\n  } catch { error => abort(\"invalid generated tool descriptor: \" + repr(error)) }\n}\n",
  )
}
