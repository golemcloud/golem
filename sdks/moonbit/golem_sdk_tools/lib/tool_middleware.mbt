///|
pub suberror ToolMiddlewareParseError {
  ToolMiddlewareParseError(String)
} derive(Debug)

///|
pub(all) struct ToolMiddlewareCommandInfo {
  command_path : Array[String]
  handler_name : String
  params : Array[(String, ParamType)]
  param_sources : Array[ToolMiddlewareParamSource]
  success_type : ParamType
  error_type : ParamType
  has_stdout : Bool
} derive(Debug, Eq)

///|
pub(all) enum ToolMiddlewareParamSource {
  CanonicalInput(String)
  Stdin
  Principal
} derive(Debug, Eq)

///|
pub(all) struct MonomorphicToolMiddlewareInfo {
  presented : ToolInfo
  expected : ToolInfo
  handlers : Array[ToolMiddlewareCommandInfo]
  underlying_methods : Array[ToolMiddlewareCommandInfo]
} derive(Debug, Eq)

///|
pub(all) struct UniversalToolMiddlewareInfo {
  handler_name : String
} derive(Debug, Eq)

///|
pub(all) enum ToolMiddlewareScopeInfo {
  Monomorphic(MonomorphicToolMiddlewareInfo)
  Universal(UniversalToolMiddlewareInfo)
} derive(Debug, Eq)

///|
pub(all) struct ToolMiddlewareInfo {
  source_name : String
  name : String
  aliases : Array[String]
  description : String
  scope : ToolMiddlewareScopeInfo
} derive(Debug, Eq)

///|
priv struct MonomorphicMiddlewareDecl {
  source_name : String
  name : String
  aliases : Array[String]
  description : String
  presented : String
  expected : String
}

///|
fn middleware_fail(message : String) -> Unit raise ToolMiddlewareParseError {
  raise ToolMiddlewareParseError(message)
}

///|
fn middleware_props(
  attr : @attribute.Attribute,
  name : String,
) -> Array[@attribute.Prop]? {
  guard attr.raw.has_prefix("#derive." + name + "(") else {
    return derive_props(attr, name)
  }
  let prefix = "#derive." + name
  let repaired = rewrite_command_alias_label(attr.raw)
  let reparsed = @attribute.Attribute::new(
    loc=attr.loc,
    ("derive", Some(name), repaired[prefix.length():].to_owned()),
  )
  derive_props(reparsed, name)
}

///|
fn middleware_annotation_kind(attr : @attribute.Attribute) -> String? {
  if middleware_props(attr, "tool_middleware") is Some(_) {
    Some("tool_middleware")
  } else if middleware_props(attr, "universal_tool_middleware") is Some(_) {
    Some("universal_tool_middleware")
  } else {
    None
  }
}

///|
fn require_middleware_identifier(
  value : String,
  kind : String,
  owner : String,
) -> Unit raise ToolMiddlewareParseError {
  if !is_tool_identifier(value) {
    middleware_fail(
      "Invalid " +
      kind +
      " identifier '" +
      value +
      "' on " +
      owner +
      "; expected lower-kebab-case",
    )
  }
}

///|
fn parse_middleware_metadata(
  props : Array[@attribute.Prop],
  annotation : String,
  owner : String,
  monomorphic : Bool,
) -> (String, Array[String], String?, String?) raise ToolMiddlewareParseError {
  let mut name : String? = None
  let aliases : Array[String] = []
  let mut presented : String? = None
  let mut expected : String? = None
  for prop in props {
    match prop_string(prop) {
      Some((None, value)) =>
        if name is None {
          name = Some(value)
        } else {
          middleware_fail("Duplicate middleware name on " + owner)
        }
      Some((Some("tool_alias" | "alias"), value)) => aliases.push(value)
      Some((Some("presented"), value)) if monomorphic =>
        if presented is None {
          presented = Some(value)
        } else {
          middleware_fail("Duplicate presented tool on " + owner)
        }
      Some((Some("expected"), value)) if monomorphic =>
        if expected is None {
          expected = Some(value)
        } else {
          middleware_fail("Duplicate expected tool on " + owner)
        }
      _ =>
        middleware_fail(
          "Malformed or unknown #derive." + annotation + " property on " + owner,
        )
    }
  }
  guard name is Some(name) else {
    middleware_fail("#derive." + annotation + " requires a canonical name")
    abort("")
  }
  require_middleware_identifier(name, "middleware", owner)
  let seen : Map[String, Unit] = {}
  for alias_name in aliases {
    require_middleware_identifier(alias_name, "middleware alias", owner)
    if alias_name == name || seen.contains(alias_name) {
      middleware_fail(
        "Duplicate middleware name or alias '" + alias_name + "' on " + owner,
      )
    }
    seen[alias_name] = ()
  }
  (name, aliases, presented, expected)
}

///|
fn function_owner(decl : @syntax.FunDecl) -> String? {
  match decl.type_name {
    Some({ name: Ident(name~), .. }) => Some(name)
    Some({ name: Dot(..), .. }) => None
    None => None
  }
}

///|
fn qualified_function_owner(decl : @syntax.FunDecl) -> String? {
  match decl.type_name {
    Some({ name: Dot(id~, ..), .. }) => Some(id)
    _ => None
  }
}

///|
fn reject_middleware_annotations(
  attrs : @list.List[@attribute.Attribute],
  allow_tool : Bool,
  allow_universal : Bool,
) -> Unit raise ToolMiddlewareParseError {
  for attr in attrs {
    match middleware_annotation_kind(attr) {
      Some("tool_middleware") if !allow_tool =>
        middleware_fail("#derive.tool_middleware must annotate an empty struct")
      Some("universal_tool_middleware") if !allow_universal =>
        middleware_fail(
          "#derive.universal_tool_middleware must annotate an async free function",
        )
      _ => ()
    }
  }
}

///|
fn reject_middleware_type_desc_annotations(
  desc : @syntax.TypeDesc,
) -> Unit raise ToolMiddlewareParseError {
  match desc {
    Record(fields~, constr_decl~) => {
      for field in fields {
        reject_middleware_annotations(field.attrs, false, false)
      }
      match constr_decl {
        Some(constr_decl) =>
          reject_middleware_annotations(constr_decl.attrs, false, false)
        None => ()
      }
    }
    Variant(constrs) | Error(EnumPayload(constrs)) =>
      for constr in constrs {
        reject_middleware_annotations(constr.attrs, false, false)
      }
    _ => ()
  }
}

///|
fn reject_local_type_middleware_annotations(
  local_types : @list.List[@syntax.LocalTypeDecl],
) -> Unit raise ToolMiddlewareParseError {
  for local_type in local_types {
    reject_middleware_type_desc_annotations(local_type.components)
  }
}

///|
fn reject_decl_body_middleware_annotations(
  body : @syntax.DeclBody,
) -> Unit raise ToolMiddlewareParseError {
  match body {
    DeclBody(local_types~, ..) =>
      reject_local_type_middleware_annotations(local_types)
    DeclNone | DeclStubs(_) => ()
  }
}

///|
fn validate_middleware_annotation_placement(
  impl_ : @syntax.Impl,
) -> Unit raise ToolMiddlewareParseError {
  match impl_ {
    TopTypeDef(decl) => {
      reject_middleware_annotations(decl.attrs, true, false)
      reject_middleware_type_desc_annotations(decl.components)
    }
    TopFuncDef(fun_decl~, decl_body~, ..) => {
      reject_middleware_annotations(fun_decl.attrs, false, true)
      reject_decl_body_middleware_annotations(decl_body)
    }
    TopImpl(attrs~, body~, ..) => {
      reject_middleware_annotations(attrs, false, false)
      reject_decl_body_middleware_annotations(body)
    }
    TopImplRelation(attrs~, ..) =>
      reject_middleware_annotations(attrs, false, false)
    TopLetDef(attrs~, ..) => reject_middleware_annotations(attrs, false, false)
    TopTest(attrs~, local_types~, ..) => {
      reject_middleware_annotations(attrs, false, false)
      reject_local_type_middleware_annotations(local_types)
    }
    TopTrait(decl) => {
      reject_middleware_annotations(decl.attrs, false, false)
      for trait_method in decl.methods {
        reject_middleware_annotations(trait_method.attrs, false, false)
      }
    }
    TopUsing(attrs~, ..) => reject_middleware_annotations(attrs, false, false)
    TopView(attrs~, view_constrs~, ..) => {
      reject_middleware_annotations(attrs, false, false)
      for constr in view_constrs {
        reject_middleware_annotations(constr.attrs, false, false)
      }
    }
    TopExpr(local_types~, ..) =>
      reject_local_type_middleware_annotations(local_types)
  }
}

///|
fn parse_source_impls(
  files : Array[(String, String)],
) -> Array[@syntax.Impl] raise ToolMiddlewareParseError {
  let impls : Array[@syntax.Impl] = []
  for pair in files {
    let (filename, source) = pair
    let (parsed, reports) = @parser.parse_string(source)
    if reports.length() > 0 {
      let messages : Array[String] = []
      for report in reports {
        messages.push(report.to_string())
      }
      middleware_fail(
        "Failed to parse " + filename + ": " + messages.join("; "),
      )
    }
    for impl_ in parsed {
      impls.push(impl_)
    }
  }
  impls
}

///|
fn find_local_tool(
  tools : Array[ToolInfo],
  middleware_name : String,
  reference : String,
) -> ToolInfo raise ToolMiddlewareParseError {
  if reference.contains(".") || reference.has_prefix("@") {
    middleware_fail(
      "Tool middleware '" +
      middleware_name +
      "' references tool '" +
      reference +
      "' outside its package; GOL-34 supports same-package tool shapes only",
    )
  }
  let matches = tools.filter(tool => tool.source_name == reference)
  guard matches is [tool] else {
    middleware_fail(
      "Tool middleware '" +
      middleware_name +
      "' references tool '" +
      reference +
      "' outside its package; GOL-34 supports same-package tool shapes only",
    )
    abort("")
  }
  tool
}

///|
fn resolve_surface_subtree(
  tools : Array[ToolInfo],
  middleware_name : String,
  reference : String,
) -> ToolInfo raise ToolMiddlewareParseError {
  if reference.contains(".") ||
    reference.contains("::") ||
    reference.has_prefix("@") {
    middleware_fail(
      "Tool middleware '" +
      middleware_name +
      "' references tool '" +
      reference +
      "' outside its package; GOL-34 supports same-package tool shapes only",
    )
  }
  let matches = tools.filter(tool => tool.source_name == reference)
  guard matches is [tool] else {
    middleware_fail("Subtree tool '" + reference + "' could not be resolved")
    abort("")
  }
  tool
}

///|
fn middleware_output_stream_type() -> ParamType {
  Parameterized("@asyncCore.Stream", [Simple("Byte")])
}

///|
fn projected_middleware_param(
  parameter : ToolParamType,
  include_principal : Bool,
) -> ParamType? {
  match parameter {
    Public(type_) | LabelledPublic(type_) | QuestionOptionalPublic(type_) =>
      Some(type_)
    Hidden(Principal) | LabelledHidden(Principal) =>
      if include_principal {
        Some(Simple("@tool.Principal"))
      } else {
        None
      }
    Hidden(InputStream) | LabelledHidden(InputStream) =>
      Some(Simple("@toolMiddleware.MiddlewareStdin"))
    Hidden(OutputStream) | LabelledHidden(OutputStream) => None
  }
}

///|
priv struct ProjectedMiddlewareParam {
  source_name : String
  type_ : ParamType
  identities : Array[String]
  source : ToolMiddlewareParamSource
}

///|
fn middleware_param_identities(
  command : ToolCommandInfo,
  source_name : String,
  parameter : ToolParamType,
) -> Array[String] {
  match parameter {
    Hidden(_) | LabelledHidden(_) => [source_name]
    Public(_) | LabelledPublic(_) | QuestionOptionalPublic(_) => {
      let info = command.args
        .iter()
        .find_first(arg => arg.source_param == source_name)
      match info {
        Some(info) =>
          [info.name.unwrap_or(moonbit_name_to_kebab_case(source_name))].concat(
            info.aliases,
          )
        None => [moonbit_name_to_kebab_case(source_name)]
      }
    }
  }
}

///|
fn append_projected_params(
  existing : Array[ProjectedMiddlewareParam],
  command : ToolCommandInfo,
  include_principal : Bool,
) -> Array[ProjectedMiddlewareParam] {
  let result = existing.copy()
  let inherited_identities : Map[String, Unit] = {}
  for parameter in result {
    for identity in parameter.identities {
      inherited_identities[identity] = ()
    }
  }
  for pair in command.params {
    let identities = middleware_param_identities(command, pair.0, pair.1)
    if identities.any(identity => inherited_identities.contains(identity)) {
      continue
    }
    match projected_middleware_param(pair.1, include_principal) {
      Some(type_) => {
        let source = match pair.1 {
          Public(_) | LabelledPublic(_) | QuestionOptionalPublic(_) =>
            ToolMiddlewareParamSource::CanonicalInput(identities[0])
          Hidden(InputStream) | LabelledHidden(InputStream) =>
            ToolMiddlewareParamSource::Stdin
          Hidden(Principal) | LabelledHidden(Principal) =>
            ToolMiddlewareParamSource::Principal
          Hidden(OutputStream) | LabelledHidden(OutputStream) =>
            abort("output streams are not middleware handler parameters")
        }
        result.push({ source_name: pair.0, type_, identities, source, })
        for identity in identities {
          inherited_identities[identity] = ()
        }
      }
      None => ()
    }
  }
  result
}

///|
fn append_projected_root_globals(
  existing : Array[ProjectedMiddlewareParam],
  tool : ToolInfo,
) -> Array[ProjectedMiddlewareParam] raise ToolMiddlewareParseError {
  guard tool.root_command is Some(root_index) else { return existing.copy() }
  let command = tool.commands[root_index]
  let projected = project_command(command, command.subtree is Some(_)) catch {
    ToolEmitError(message) => {
      middleware_fail(message)
      abort("")
    }
  }
  let result = existing.copy()
  let inherited_identities : Map[String, Unit] = {}
  for parameter in result {
    for identity in parameter.identities {
      inherited_identities[identity] = ()
    }
  }
  for argument in projected {
    guard argument.placement is GlobalOption || argument.placement is GlobalFlag else {
      continue
    }
    let identities = projected_arg_identity_names(argument)
    if identities.any(identity => inherited_identities.contains(identity)) {
      continue
    }
    guard argument.type_ is Some(type_) else {
      middleware_fail(
        "Global tool argument '" + argument.source + "' has no public type",
      )
      continue
    }
    result.push({
      source_name: argument.source,
      type_,
      identities,
      source: ToolMiddlewareParamSource::CanonicalInput(argument.canonical),
    })
    for identity in identities {
      inherited_identities[identity] = ()
    }
  }
  result
}

///|
fn middleware_success_type(command : ToolCommandInfo) -> ParamType {
  let value = command.success_type.unwrap_or(Simple("Unit"))
  let has_stdout = command.params.any(pair => {
    pair.1 == Hidden(OutputStream) || pair.1 == LabelledHidden(OutputStream)
  })
  if has_stdout {
    if value == Simple("Unit") {
      middleware_output_stream_type()
    } else {
      Tuple([value, middleware_output_stream_type()])
    }
  } else {
    value
  }
}

///|
fn middleware_error_type(command : ToolCommandInfo) -> ParamType {
  command.error_type.unwrap_or(Simple("@tool.NoToolError"))
}

///|
fn add_middleware_surface(
  surfaces : Array[ToolMiddlewareCommandInfo],
  handlers : Map[String, Array[String]],
  command_path : Array[String],
  source_path : Array[String],
  params : Array[ProjectedMiddlewareParam],
  command : ToolCommandInfo,
) -> Unit raise ToolMiddlewareParseError {
  let handler_name = source_path.join("__")
  guard !handler_name.is_empty() else {
    middleware_fail("Middleware command path produced an empty handler name")
    return
  }
  match handlers.get(handler_name) {
    Some(previous) if previous != command_path =>
      middleware_fail(
        "Flattened middleware handler name '" +
        handler_name +
        "' collides for command paths '" +
        previous.join("/") +
        "' and '" +
        command_path.join("/") +
        "'",
      )
    Some(_) =>
      middleware_fail(
        "Duplicate middleware handler surface '" + handler_name + "'",
      )
    None => handlers[handler_name] = command_path.copy()
  }
  let has_stdout = command.params.any(pair => {
    pair.1 == Hidden(OutputStream) || pair.1 == LabelledHidden(OutputStream)
  })
  surfaces.push({
    command_path,
    handler_name,
    params: params.map(parameter => (parameter.source_name, parameter.type_)),
    param_sources: params.map(parameter => parameter.source),
    success_type: middleware_success_type(command),
    error_type: middleware_error_type(command),
    has_stdout,
  })
}

///|
fn collect_middleware_surfaces(
  current : ToolInfo,
  tools : Array[ToolInfo],
  middleware_name : String,
  command_path : Array[String],
  source_path : Array[String],
  inherited : Array[ProjectedMiddlewareParam],
  visiting : Array[String],
  surfaces : Array[ToolMiddlewareCommandInfo],
  handlers : Map[String, Array[String]],
  include_principal : Bool,
) -> Unit raise ToolMiddlewareParseError {
  if visiting.contains(current.source_name) {
    middleware_fail(
      "Tool subtree cycle encountered while projecting middleware surface for '" +
      current.source_name +
      "'",
    )
  }
  let next_visiting = visiting.concat([current.source_name])
  let inherited_here = append_projected_root_globals(inherited, current)
  for index, command in current.commands {
    let is_root = current.root_command == Some(index)
    let command_inherited = if is_root { inherited } else { inherited_here }
    match command.subtree {
      Some(reference) => {
        let child = resolve_surface_subtree(tools, middleware_name, reference)
        collect_middleware_surfaces(
          child,
          tools,
          middleware_name,
          command_path.concat([command.name]),
          source_path.concat([command.source_name]),
          append_projected_params(command_inherited, command, include_principal),
          next_visiting,
          surfaces,
          handlers,
          include_principal,
        )
      }
      None => {
        let leaf_command_path = if is_root {
          command_path.copy()
        } else {
          command_path.concat([command.name])
        }
        let leaf_source_path = if is_root && !source_path.is_empty() {
          source_path.copy()
        } else {
          source_path.concat([command.source_name])
        }
        add_middleware_surface(
          surfaces,
          handlers,
          leaf_command_path,
          leaf_source_path,
          append_projected_params(command_inherited, command, include_principal),
          command,
        )
      }
    }
  }
}

///|
fn middleware_surfaces(
  tool : ToolInfo,
  tools : Array[ToolInfo],
  middleware_name : String,
  include_principal : Bool,
) -> Array[ToolMiddlewareCommandInfo] raise ToolMiddlewareParseError {
  let surfaces : Array[ToolMiddlewareCommandInfo] = []
  collect_middleware_surfaces(
    tool,
    tools,
    middleware_name,
    [],
    [],
    [],
    [],
    surfaces,
    {},
    include_principal,
  )
  surfaces
}

///|
fn parse_positional_handler_params(
  decl : @syntax.FunDecl,
  owner : String,
) -> Array[(String, ParamType)] raise ToolMiddlewareParseError {
  let params : Array[(String, ParamType)] = []
  for parameter in decl.decl_params.unwrap_or(@list.List::from_array([])) {
    match parameter {
      Positional(binder~, ty=Some(type_)) => {
        if binder.name == "self" {
          middleware_fail(
            "Tool middleware handler '" + owner + "' must be static",
          )
        }
        let parsed = syntax_type_to_param_type(type_) catch {
          AgentParseError(message) => {
            middleware_fail(message)
            abort("")
          }
        }
        params.push((binder.name, parsed))
      }
      _ =>
        middleware_fail(
          "Every parameter of tool middleware handler '" +
          owner +
          "' must be positional, named, and typed",
        )
    }
  }
  params
}

///|
fn handler_return_type(
  decl : @syntax.FunDecl,
  owner : String,
) -> ParamType raise ToolMiddlewareParseError {
  guard decl.return_type is Some(return_type) else {
    middleware_fail(
      "Tool middleware handler '" + owner + "' requires a return type",
    )
    abort("")
  }
  syntax_type_to_param_type(return_type) catch {
    AgentParseError(message) => {
      middleware_fail(message)
      abort("")
    }
  }
}

///|
fn expected_handler_return(surface : ToolMiddlewareCommandInfo) -> ParamType {
  ResultType(
    surface.success_type,
    Parameterized("@toolMiddleware.ToolInvokeError", [surface.error_type]),
  )
}

///|
fn validate_handler_signature(
  decl : @syntax.FunDecl,
  middleware : MonomorphicMiddlewareDecl,
  surface : ToolMiddlewareCommandInfo,
) -> Unit raise ToolMiddlewareParseError {
  let owner = middleware.source_name + "::" + surface.handler_name
  if !(decl.vis is Pub(..)) {
    middleware_fail("Tool middleware handler '" + owner + "' must be public")
  }
  if decl.is_async is None {
    middleware_fail("Tool middleware handler '" + owner + "' must be async")
  }
  let actual = parse_positional_handler_params(decl, owner)
  let expected : Array[(String, ParamType)] = [
    ("underlying", Simple(middleware.expected + "Underlying")),
  ]
  for pair in surface.params {
    expected.push(pair)
  }
  if actual.length() != expected.length() {
    middleware_fail(
      "Tool middleware handler '" +
      owner +
      "' has \{actual.length()} parameters; expected \{expected.length()}",
    )
  }
  for index in 0..<expected.length() {
    if actual[index] != expected[index] {
      middleware_fail(
        "Tool middleware handler '" +
        owner +
        "' parameter \{index + 1} must be '" +
        expected[index].0 +
        "' with the generated middleware type",
      )
    }
  }
  if handler_return_type(decl, owner) != expected_handler_return(surface) {
    middleware_fail(
      "Tool middleware handler '" +
      owner +
      "' must return the presented command's middleware result projection",
    )
  }
}

///|
fn validate_monomorphic_handlers(
  impls : Array[@syntax.Impl],
  middleware : MonomorphicMiddlewareDecl,
  surfaces : Array[ToolMiddlewareCommandInfo],
) -> Unit raise ToolMiddlewareParseError {
  let expected : Map[String, ToolMiddlewareCommandInfo] = {}
  for surface in surfaces {
    expected[surface.handler_name] = surface
  }
  let found : Map[String, Unit] = {}
  for impl_ in impls {
    guard impl_ is TopFuncDef(fun_decl~, ..) else { continue }
    if qualified_function_owner(fun_decl) == Some(middleware.source_name) {
      middleware_fail(
        "Tool middleware '" +
        middleware.name +
        "' defines handler '" +
        fun_decl.name.name +
        "' outside its package; GOL-34 supports same-package middleware handlers only",
      )
    }
    guard function_owner(fun_decl) == Some(middleware.source_name) else {
      continue
    }
    let handler_name = fun_decl.name.name
    guard expected.get(handler_name) is Some(surface) else {
      if fun_decl.vis is Pub(..) {
        middleware_fail(
          "Tool middleware '" +
          middleware.name +
          "' has unexpected public handler '" +
          middleware.source_name +
          "::" +
          handler_name +
          "'",
        )
      }
      continue
    }
    if found.contains(handler_name) {
      middleware_fail(
        "Tool middleware '" +
        middleware.name +
        "' defines handler '" +
        handler_name +
        "' more than once",
      )
    }
    validate_handler_signature(fun_decl, middleware, surface)
    found[handler_name] = ()
  }
  for surface in surfaces {
    if !found.contains(surface.handler_name) {
      middleware_fail(
        "Tool middleware '" +
        middleware.name +
        "' is missing async handler '" +
        middleware.source_name +
        "::" +
        surface.handler_name +
        "' for command path '" +
        surface.command_path.join("/") +
        "'",
      )
    }
  }
}

///|
fn expected_universal_return_type() -> ParamType {
  ResultType(
    Simple("@toolMiddleware.RawInvocationResult"),
    Parameterized("@toolMiddleware.ToolInvokeError", [
      Simple("@toolMiddleware.RawTypedSchemaValue"),
    ]),
  )
}

///|
fn validate_universal_handler(
  decl : @syntax.FunDecl,
  middleware_name : String,
) -> Unit raise ToolMiddlewareParseError {
  let owner = "universal tool middleware '" + middleware_name + "'"
  if decl.type_name is Some(_) {
    middleware_fail(
      "#derive.universal_tool_middleware must annotate an async free function",
    )
  }
  if !(decl.vis is Pub(..)) {
    middleware_fail(owner + " handler must be public")
  }
  if decl.is_async is None {
    middleware_fail(owner + " handler must be async")
  }
  let params = parse_positional_handler_params(decl, decl.name.name)
  let expected : Array[(String, ParamType)] = [
    ("invocation", Simple("@toolMiddleware.RawToolInvocation")),
    ("underlying", Simple("@toolMiddleware.UnderlyingTool")),
  ]
  if params != expected {
    middleware_fail(
      owner +
      " handler must accept exactly '(invocation : @toolMiddleware.RawToolInvocation, underlying : @toolMiddleware.UnderlyingTool)'",
    )
  }
  if handler_return_type(decl, decl.name.name) !=
    expected_universal_return_type() {
    middleware_fail(
      owner +
      " handler must return 'Result[@toolMiddleware.RawInvocationResult, @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue]]'",
    )
  }
}

///|
fn validate_middleware_name_collisions(
  middlewares : Array[ToolMiddlewareInfo],
) -> Unit raise ToolMiddlewareParseError {
  let owners : Map[String, String] = {}
  for middleware in middlewares {
    for name in [middleware.name].concat(middleware.aliases) {
      match owners.get(name) {
        Some(existing) =>
          middleware_fail(
            "Tool middleware name or alias '" +
            name +
            "' collides between '" +
            existing +
            "' and '" +
            middleware.name +
            "'",
          )
        None => owners[name] = middleware.name
      }
    }
  }
}

///|
fn reject_qualified_tool_subtrees_for_middleware(
  impls : Array[@syntax.Impl],
) -> Unit raise ToolMiddlewareParseError {
  let has_monomorphic = impls.any(impl_ => {
    match impl_ {
      TopTypeDef(decl) =>
        decl.attrs.any(attr => {
          middleware_props(attr, "tool_middleware") is Some(_)
        })
      _ => false
    }
  })
  guard has_monomorphic else { return }
  for impl_ in impls {
    guard impl_ is TopFuncDef(fun_decl~, ..) else { continue }
    for attr in fun_decl.attrs {
      guard derive_props(attr, "command") is Some(props) else { continue }
      for prop in props {
        guard prop_string(prop) is Some((Some("subtree"), reference)) else {
          continue
        }
        if reference.contains(".") ||
          reference.contains("::") ||
          reference.has_prefix("@") {
          middleware_fail(
            "Tool middleware references tool '" +
            reference +
            "' outside its package; GOL-34 supports same-package tool shapes only",
          )
        }
      }
    }
  }
}

///|
pub fn parse_tool_middlewares(
  files : Array[(String, String)],
) -> Array[ToolMiddlewareInfo] raise ToolMiddlewareParseError {
  let impls = parse_source_impls(files)
  for impl_ in impls {
    validate_middleware_annotation_placement(impl_)
  }
  reject_qualified_tool_subtrees_for_middleware(impls)
  let tools = parse_tools(files) catch {
    ToolParseError(message) => {
      middleware_fail(message)
      abort("")
    }
  }
  let monomorphic : Array[MonomorphicMiddlewareDecl] = []
  let result : Array[ToolMiddlewareInfo] = []
  for impl_ in impls {
    match impl_ {
      TopTypeDef(decl) => {
        let middleware_attrs = decl.attrs
          .filter(attr => middleware_annotation_kind(attr) is Some(_))
          .to_array()
        if middleware_attrs.length() > 1 {
          middleware_fail(
            "Middleware declaration '" +
            decl.tycon +
            "' has more than one middleware annotation",
          )
        }
        for attr in middleware_attrs {
          match middleware_annotation_kind(attr) {
            Some("universal_tool_middleware") =>
              middleware_fail(
                "#derive.universal_tool_middleware must annotate an async free function",
              )
            Some("tool_middleware") => {
              match decl.components {
                Record(fields~, ..) if fields.is_empty() => ()
                _ =>
                  middleware_fail(
                    "#derive.tool_middleware struct '" +
                    decl.tycon +
                    "' must be empty",
                  )
              }
              let (name, aliases, presented, expected) = parse_middleware_metadata(
                middleware_props(attr, "tool_middleware").unwrap(),
                "tool_middleware",
                "middleware " + decl.tycon,
                true,
              )
              guard presented is Some(presented) else {
                middleware_fail(
                  "#derive.tool_middleware on '" +
                  decl.tycon +
                  "' requires presented=\"Tool\"",
                )
                continue
              }
              monomorphic.push({
                source_name: decl.tycon,
                name,
                aliases,
                description: tool_doc(decl.doc),
                presented,
                expected: expected.unwrap_or(presented),
              })
            }
            _ => ()
          }
        }
      }
      TopFuncDef(fun_decl~, ..) => {
        let middleware_attrs = fun_decl.attrs
          .filter(attr => middleware_annotation_kind(attr) is Some(_))
          .to_array()
        if middleware_attrs.length() > 1 {
          middleware_fail(
            "Middleware handler '" +
            fun_decl.name.name +
            "' has more than one middleware annotation",
          )
        }
        for attr in middleware_attrs {
          match middleware_annotation_kind(attr) {
            Some("tool_middleware") =>
              middleware_fail(
                "#derive.tool_middleware must annotate an empty struct",
              )
            Some("universal_tool_middleware") => {
              if fun_decl.type_name is Some(_) {
                middleware_fail(
                  "#derive.universal_tool_middleware must annotate an async free function",
                )
              }
              let (name, aliases, presented, expected) = parse_middleware_metadata(
                middleware_props(attr, "universal_tool_middleware").unwrap(),
                "universal_tool_middleware",
                "function " + fun_decl.name.name,
                false,
              )
              ignore(presented)
              ignore(expected)
              validate_universal_handler(fun_decl, name)
              result.push({
                source_name: fun_decl.name.name,
                name,
                aliases,
                description: tool_doc(fun_decl.doc),
                scope: Universal({ handler_name: fun_decl.name.name, }),
              })
            }
            _ => ()
          }
        }
      }
      _ => ()
    }
  }
  for middleware in monomorphic {
    let presented = find_local_tool(
      tools,
      middleware.name,
      middleware.presented,
    )
    let expected = find_local_tool(tools, middleware.name, middleware.expected)
    let handlers = middleware_surfaces(presented, tools, middleware.name, true)
    let underlying_methods = middleware_surfaces(
      expected,
      tools,
      middleware.name,
      false,
    )
    validate_monomorphic_handlers(impls, middleware, handlers)
    result.push({
      source_name: middleware.source_name,
      name: middleware.name,
      aliases: middleware.aliases,
      description: middleware.description,
      scope: Monomorphic({ presented, expected, handlers, underlying_methods, }),
    })
  }
  validate_middleware_name_collisions(result)
  result
}
