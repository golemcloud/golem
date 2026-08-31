///|
pub(all) enum ComponentRole {
  Ordinary
  ToolMiddleware
  Combined
} derive(Debug, Eq)

///|
pub fn ComponentRole::name(self : Self) -> String {
  match self {
    Ordinary => "ordinary"
    ToolMiddleware => "tool-middleware"
    Combined => "combined"
  }
}

///|
pub fn ComponentRole::gen_package(self : Self) -> String {
  match self {
    Ordinary => "gen"
    ToolMiddleware => "gen-tool-middleware"
    Combined => "gen-agent-tool-middleware"
  }
}

///|
pub fn ComponentRole::allows_agent_registration(self : Self) -> Bool {
  self is Ordinary || self is Combined
}

///|
pub fn ComponentRole::allows_tool_registration(self : Self) -> Bool {
  self is Ordinary || self is Combined
}

///|
pub fn ComponentRole::allows_tool_middleware_registration(self : Self) -> Bool {
  self is ToolMiddleware || self is Combined
}

///|
pub suberror ComponentRoleError {
  ComponentRoleError(String)
} derive(Debug)

///|
fn component_role_fail(message : String) -> Unit raise ComponentRoleError {
  raise ComponentRoleError(message)
}

///|
pub fn raise_component_role_error(
  message : String,
) -> Unit raise ComponentRoleError {
  component_role_fail(message)
}

///|
pub fn parse_component_role(
  value : String,
) -> ComponentRole raise ComponentRoleError {
  match value {
    "ordinary" => Ordinary
    "tool-middleware" => ToolMiddleware
    "combined" => Combined
    _ => {
      component_role_fail(
        "Invalid component role '" +
        value +
        "'; expected ordinary, tool-middleware, or combined",
      )
      abort("")
    }
  }
}

///|
pub(all) struct AgentsCommandOptions {
  project_root : String
  component_dir : String
  role : ComponentRole
} derive(Debug, Eq)

///|
pub(all) struct ReexportsCommandOptions {
  sdk_path : String
  target_dir : String
  role : ComponentRole
} derive(Debug, Eq)

///|
fn set_cli_option(
  option : String,
  current : String?,
  value : String,
) -> String raise ComponentRoleError {
  if current is Some(_) {
    component_role_fail("Duplicate " + option + " option")
  }
  value
}

///|
pub fn parse_agents_command_args(
  args : Array[String],
) -> AgentsCommandOptions raise ComponentRoleError {
  if args.is_empty() {
    component_role_fail(
      "Usage: golem_sdk_tools agents <project-root> --component-dir <component-dir> --role <role>",
    )
  }
  let mut component_dir : String? = None
  let mut role_name : String? = None
  let mut i = 1
  while i < args.length() {
    let option = args[i]
    if option != "--component-dir" && option != "--role" {
      component_role_fail("Unknown agents option '" + option + "'")
    }
    if i + 1 >= args.length() {
      component_role_fail("Missing value for " + option)
    }
    let value = args[i + 1]
    if option == "--component-dir" {
      component_dir = Some(set_cli_option(option, component_dir, value))
    } else {
      role_name = Some(set_cli_option(option, role_name, value))
    }
    i = i + 2
  }
  guard component_dir is Some(component_dir) else {
    component_role_fail("Missing required --component-dir option")
    abort("")
  }
  guard role_name is Some(role_name) else {
    component_role_fail("Missing required --role option")
    abort("")
  }
  {
    project_root: args[0],
    component_dir,
    role: parse_component_role(role_name),
  }
}

///|
pub fn parse_reexports_command_args(
  args : Array[String],
) -> ReexportsCommandOptions raise ComponentRoleError {
  if args.length() < 2 {
    component_role_fail(
      "Usage: golem_sdk_tools reexports <sdk-path> <target-dir> --role <role>",
    )
  }
  let mut role_name : String? = None
  let mut i = 2
  while i < args.length() {
    let option = args[i]
    if option != "--role" {
      component_role_fail("Unknown reexports option '" + option + "'")
    }
    if i + 1 >= args.length() {
      component_role_fail("Missing value for --role")
    }
    role_name = Some(set_cli_option(option, role_name, args[i + 1]))
    i = i + 2
  }
  guard role_name is Some(role_name) else {
    component_role_fail("Missing required --role option")
    abort("")
  }
  {
    sdk_path: args[0],
    target_dir: args[1],
    role: parse_component_role(role_name),
  }
}

///|
fn is_middleware_attribute(attr : @attribute.Attribute) -> Bool {
  attr.raw == "#derive.tool_middleware" ||
  attr.raw.has_prefix("#derive.tool_middleware(") ||
  attr.raw == "#derive.universal_tool_middleware" ||
  attr.raw.has_prefix("#derive.universal_tool_middleware(")
}

///|
fn attrs_have_middleware_annotation(
  attrs : @list.List[@attribute.Attribute],
) -> Bool {
  attrs.any(is_middleware_attribute)
}

///|
fn type_desc_has_middleware_annotation(desc : @syntax.TypeDesc) -> Bool {
  match desc {
    Record(fields~) =>
      fields.any(field => attrs_have_middleware_annotation(field.attrs))
    Variant(constrs) | Error(EnumPayload(constrs)) =>
      constrs.any(constr => attrs_have_middleware_annotation(constr.attrs))
    _ => false
  }
}

///|
fn impl_has_middleware_annotation(impl_ : @syntax.Impl) -> Bool {
  match impl_ {
    TopTypeDef(decl) =>
      attrs_have_middleware_annotation(decl.attrs) ||
      type_desc_has_middleware_annotation(decl.components)
    TopFuncDef(fun_decl~, ..) =>
      attrs_have_middleware_annotation(fun_decl.attrs)
    TopImpl(attrs~, body~, ..) =>
      attrs_have_middleware_annotation(attrs)
    TopImplRelation(attrs~, ..) => attrs_have_middleware_annotation(attrs)
    TopLetDef(attrs~, ..) => attrs_have_middleware_annotation(attrs)
    TopTest(attrs~, ..) => attrs_have_middleware_annotation(attrs)
    TopTrait(decl) =>
      attrs_have_middleware_annotation(decl.attrs) ||
      decl.methods.any(trait_method => {
        attrs_have_middleware_annotation(trait_method.attrs)
      })
    TopUsing(attrs~, ..) => attrs_have_middleware_annotation(attrs)
    TopView(attrs~, view_constrs~, ..) =>
      attrs_have_middleware_annotation(attrs) ||
      view_constrs.any(constr => attrs_have_middleware_annotation(constr.attrs))
    TopExtend(attrs~, ..) => attrs_have_middleware_annotation(attrs)
    TopExpr(..) | TopImport(..) => false
  }
}

///|
pub fn has_middleware_annotations(
  files : Array[(String, String)],
) -> Bool raise ComponentRoleError {
  for pair in files {
    let (filename, source) = pair
    let (impls, reports) = @parser.parse_string(source)
    if reports.length() > 0 {
      let messages : Array[String] = []
      for report in reports {
        messages.push(report.to_string())
      }
      component_role_fail(
        "Failed to parse " + filename + ": " + messages.join("; "),
      )
    }
    for impl_ in impls {
      if impl_has_middleware_annotation(impl_) {
        return true
      }
    }
  }
  false
}

///|
pub fn validate_component_role(
  role : ComponentRole,
  agent_count : Int,
  middleware_annotations : Bool,
) -> Unit raise ComponentRoleError {
  if role is Ordinary && middleware_annotations {
    component_role_fail(
      "ordinary component role does not export tool middleware",
    )
  }
  if role is ToolMiddleware && agent_count > 0 {
    component_role_fail("tool-middleware component role does not export agents")
  }
}

///|
pub fn component_target_dir(
  project_root : String,
  component_dir : String,
) -> String {
  let root = if project_root.length() > 1 && project_root.has_suffix("/") {
    project_root[:project_root.length() - 1].to_owned()
  } else {
    project_root
  }
  let target = if component_dir.length() > 1 && component_dir.has_suffix("/") {
    component_dir[:component_dir.length() - 1].to_owned()
  } else {
    component_dir
  }
  if target == "" || target == "." {
    root
  } else if target.has_prefix("/") {
    target
  } else {
    root + "/" + target
  }
}

///|
pub fn component_role_marker(role : ComponentRole) -> String {
  "# Generated by golem_sdk_tools; do not edit.\n" + role.name() + "\n"
}

///|
pub fn parse_component_role_marker(
  contents : String,
) -> ComponentRole raise ComponentRoleError {
  let lines = contents
    .split("\n")
    .map(fn(line) { line.trim().to_owned() })
    .filter(fn(line) { line != "" && !line.has_prefix("#") })
    .collect()
  guard lines is [role] else {
    component_role_fail("Invalid generated component-role marker")
    abort("")
  }
  parse_component_role(role)
}
