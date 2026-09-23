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
pub(all) struct AgentsCommandOptions {
  project_root : String
  component_dir : String
} derive(Debug, Eq)

///|
pub(all) struct ReexportsCommandOptions {
  sdk_path : String
  target_dir : String
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
      "Usage: golem_sdk_tools agents <project-root> --component-dir <component-dir>",
    )
  }
  let mut component_dir : String? = None
  let mut i = 1
  while i < args.length() {
    let option = args[i]
    if option != "--component-dir" {
      component_role_fail("Unknown agents option '" + option + "'")
    }
    if i + 1 >= args.length() {
      component_role_fail("Missing value for " + option)
    }
    let value = args[i + 1]
    component_dir = Some(set_cli_option(option, component_dir, value))
    i = i + 2
  }
  guard component_dir is Some(component_dir) else {
    component_role_fail("Missing required --component-dir option")
    abort("")
  }
  { project_root: args[0], component_dir, }
}

///|
pub fn parse_reexports_command_args(
  args : Array[String],
) -> ReexportsCommandOptions raise ComponentRoleError {
  if args.length() < 2 {
    component_role_fail(
      "Usage: golem_sdk_tools reexports <sdk-path> <target-dir>",
    )
  }
  if args.length() > 2 {
    component_role_fail("Unknown reexports option '" + args[2] + "'")
  }
  { sdk_path: args[0], target_dir: args[1], }
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
    TopImpl(attrs~, ..) => attrs_have_middleware_annotation(attrs)
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
