///|
pub fn render_help(
  tool : @toolCommon.Tool,
  command_path : Array[String],
) -> Result[String, ToolBuildError] {
  let path = match resolve_command_path_indices(tool, command_path) {
    Ok(path) => path
    Err(e) => return Err(e)
  }
  let idx = path[path.length() - 1]
  let node = match command_node_at(tool, idx) {
    Some(node) => node
    None => return Err(EmptyCommandTree)
  }
  let out = StringBuilder::new()
  out.write_string("Usage: " + node.name + "\n\n")
  out.write_string(node.doc.summary + "\n")
  out.write_string(node.doc.description + "\n")
  let globals = effective_globals(tool, path)
  if globals.length() > 0 {
    out.write_string("\nGlobals:\n")
    for global in globals {
      match global {
        OptionField(option) =>
          out.write_string(
            "  --" + option.long + "\t" + option.doc.summary + "\n",
          )
        FlagField(flag) =>
          out.write_string("  --" + flag.long + "\t" + flag.doc.summary + "\n")
      }
    }
  }
  if node.body is Some(body) {
    if body.positionals.fixed.length() > 0 {
      out.write_string("\nPositionals:\n")
      for positional in body.positionals.fixed {
        out.write_string(
          "  " + positional.name + "\t" + positional.doc.summary + "\n",
        )
      }
    }
    if body.positionals.tail is Some(tail) {
      out.write_string("\nTail:\n")
      out.write_string("  " + tail.name + "...\t" + tail.doc.summary + "\n")
    }
    if body.options.length() > 0 {
      out.write_string("\nOptions:\n")
      for option in body.options {
        out.write_string(
          "  --" + option.long + "\t" + option.doc.summary + "\n",
        )
      }
    }
    if body.flags.length() > 0 {
      out.write_string("\nFlags:\n")
      for flag in body.flags {
        out.write_string("  --" + flag.long + "\t" + flag.doc.summary + "\n")
      }
    }
  }
  if node.subcommands.length() > 0 {
    out.write_string("\nSubcommands:\n")
    for child_idx in node.subcommands {
      if command_node_at(tool, child_idx) is Some(child) {
        out.write_string("  " + child.name + "\t" + child.doc.summary + "\n")
      }
    }
  }
  Ok(out.to_string())
}

///|
pub fn render_argument_help(
  tool : @toolCommon.Tool,
  command_path : Array[String],
  arg_name : String,
) -> Result[String, ToolBuildError] {
  let path = match resolve_command_path_indices(tool, command_path) {
    Ok(path) => path
    Err(e) => return Err(e)
  }
  for global in effective_globals(tool, path) {
    match global {
      OptionField(option) =>
        if option.long == arg_name || option.aliases.contains(arg_name) {
          return Ok(render_option_help(option, true))
        }
      FlagField(flag) =>
        if flag.long == arg_name || flag.aliases.contains(arg_name) {
          return Ok(render_flag_help(flag, true))
        }
    }
  }
  let idx = path[path.length() - 1]
  match command_node_at(tool, idx) {
    Some(node) =>
      match node.body {
        Some(body) => {
          for positional in body.positionals.fixed {
            if positional.name == arg_name {
              let required = if positional.required { ", required" } else { "" }
              return Ok(
                positional.name +
                " (positional" +
                required +
                ")\n" +
                positional.doc.summary +
                "\n" +
                positional.doc.description +
                "\n",
              )
            }
          }
          if body.positionals.tail is Some(tail) {
            if tail.name == arg_name {
              return Ok(
                tail.name +
                "... (tail positional)\n" +
                tail.doc.summary +
                "\n" +
                tail.doc.description +
                "\n",
              )
            }
          }
          for option in body.options {
            if option.long == arg_name || option.aliases.contains(arg_name) {
              return Ok(render_option_help(option, false))
            }
          }
          for flag in body.flags {
            if flag.long == arg_name || flag.aliases.contains(arg_name) {
              return Ok(render_flag_help(flag, false))
            }
          }
        }
        None => ()
      }
    None => ()
  }
  Err(CommandNotFound(arg_name))
}

///|
pub fn resolve_command_path(
  tool : @toolCommon.Tool,
  command_path : Array[String],
) -> Result[Int, ToolBuildError] {
  match resolve_command_path_indices(tool, command_path) {
    Ok(path) => Ok(path[path.length() - 1])
    Err(e) => Err(e)
  }
}

///|
priv enum EffectiveCommandField {
  OptionField(@toolCommon.OptionSpec)
  FlagField(@toolCommon.FlagSpec)
}

///|
fn resolve_command_path_indices(
  tool : @toolCommon.Tool,
  command_path : Array[String],
) -> Result[Array[Int], ToolBuildError] {
  guard tool.commands.nodes.length() > 0 else { return Err(EmptyCommandTree) }
  let indices : Array[Int] = [0]
  let mut idx = 0
  for part in command_path {
    let node = match command_node_at(tool, idx) {
      Some(node) => node
      None => return Err(CommandNotFound(part))
    }
    let mut found : Int? = None
    for child_idx in node.subcommands {
      if found is None {
        match command_node_at(tool, child_idx) {
          Some(child) =>
            if child.name == part || child.aliases.contains(part) {
              found = Some(child_idx)
            }
          None => ()
        }
      }
    }
    match found {
      Some(next) => {
        idx = next
        indices.push(next)
      }
      None => return Err(CommandNotFound(part))
    }
  }
  Ok(indices)
}

///|
fn command_node_at(
  tool : @toolCommon.Tool,
  idx : Int,
) -> @toolCommon.CommandNode? {
  if idx < 0 || idx >= tool.commands.nodes.length() {
    None
  } else {
    Some(tool.commands.nodes[idx])
  }
}

///|
fn effective_globals(
  tool : @toolCommon.Tool,
  path : Array[Int],
) -> Array[EffectiveCommandField] {
  let result : Array[EffectiveCommandField] = []
  for idx in path {
    if command_node_at(tool, idx) is Some(node) {
      for option in node.globals.options {
        result.push(OptionField(option))
      }
      for flag in node.globals.flags {
        result.push(FlagField(flag))
      }
    }
  }
  result
}

///|
fn render_option_help(option : @toolCommon.OptionSpec, global : Bool) -> String {
  "--" +
  option.long +
  " (option" +
  (if global { ", global" } else { "" }) +
  ")\n" +
  option.doc.summary +
  "\n" +
  option.doc.description +
  "\n"
}

///|
fn render_flag_help(flag : @toolCommon.FlagSpec, global : Bool) -> String {
  "--" +
  flag.long +
  " (flag" +
  (if global { ", global" } else { "" }) +
  ")\n" +
  flag.doc.summary +
  "\n" +
  flag.doc.description +
  "\n"
}
