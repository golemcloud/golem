///|
pub suberror ToolEmitError {
  ToolEmitError(String)
} derive(Debug, Eq)

///|
fn[A, B] Option::flat_map(self : A?, f : (A) -> B?) -> B? {
  match self {
    Some(value) => f(value)
    None => None
  }
}

///|
fn[T] Array::concat(self : Array[T], other : Array[T]) -> Array[T] {
  let result = self.copy()
  for value in other {
    result.push(value)
  }
  result
}

///|
priv enum ToolArgPlacement {
  GlobalOption
  GlobalFlag
  Positional
  Tail
  Option
  Flag
  Hidden(ToolHiddenType)
}

///|
priv enum ToolArgSurface {
  SurfacePositional
  SurfaceTail
  SurfaceOptionScalar
  SurfaceOptionRepeated
  SurfaceBoolFlag
  SurfaceCountFlag
  SurfaceStream
  SurfacePrincipal
} derive(Eq)

///|
priv struct ProjectedToolArg {
  source : String
  canonical : String
  aliases : Array[String]
  type_ : ParamType?
  value_type : ParamType?
  placement : ToolArgPlacement
  call_suffix : String
  info : ToolArgInfo?
  required : Bool
}

///|
fn emit_fail(message : String) -> Unit raise ToolEmitError {
  raise ToolEmitError(message)
}

///|
fn emit_opt_string(value : String?) -> String {
  match value {
    Some(value) => "Some(\"" + escape_string_lit(value) + "\")"
    None => "None"
  }
}

///|
fn emit_strings(values : Array[String]) -> String {
  "[" +
  values.map(value => "\"" + escape_string_lit(value) + "\"").join(", ") +
  "]"
}

///|
fn emit_examples(values : Array[ToolExample]) -> String {
  "[" +
  values
  .map(example => {
    "{ title: \"" +
    escape_string_lit(example.title.unwrap_or("")) +
    "\", body: \"" +
    escape_string_lit(example.body) +
    "\" }"
  })
  .join(", ") +
  "]"
}

///|
fn emit_doc(description : String, examples : Array[ToolExample]) -> String {
  let lines = description
    .split("\n")
    .map(line => line.trim().to_string())
    .to_array()
  let mut first = -1
  for i, line in lines {
    if first < 0 && !line.is_empty() {
      first = i
    }
  }
  let summary = if first < 0 { "" } else { lines[first] }
  let rest : Array[String] = []
  if first >= 0 {
    for i in (first + 1)..<lines.length() {
      rest.push(lines[i])
    }
  }
  "{ summary: \"" +
  escape_string_lit(summary) +
  "\", description: \"" +
  escape_string_lit(rest.join("\n").trim().to_string()) +
  "\", examples: " +
  emit_examples(examples) +
  " }"
}

///|
fn emit_type(pt : ParamType) -> String {
  param_type_to_type_string(pt)
}

///|
fn unwrap_optional(pt : ParamType) -> (ParamType, Bool) {
  match pt {
    Optional(inner) => (inner, true)
    other => (other, false)
  }
}

///|
fn list_item(pt : ParamType) -> ParamType? {
  match pt {
    List(inner) => Some(inner)
    _ => None
  }
}

///|
fn is_map(pt : ParamType) -> Bool {
  match pt {
    Parameterized("Map", _) | Parameterized("@map.Map", _) => true
    _ => false
  }
}

///|
fn arg_info(command : ToolCommandInfo, source : String) -> ToolArgInfo? {
  command.args.iter().find_first(arg => arg.source_param == source)
}

///|
fn has_refinements(arg : ToolArgInfo) -> String? {
  if arg.regex is Some(_) {
    return Some("regex")
  }
  if arg.min_length is Some(_) {
    return Some("min_length")
  }
  if arg.max_length is Some(_) {
    return Some("max_length")
  }
  if arg.kind == Some("file") ||
    arg.kind == Some("dir") ||
    arg.kind == Some("any") {
    return Some("kind")
  }
  if arg.direction is Some(_) {
    return Some("direction")
  }
  if arg.mimes.length() > 0 {
    return Some("mime")
  }
  if arg.schemes.length() > 0 {
    return Some("scheme")
  }
  if arg.bounds is Some(_) {
    return Some("bounds")
  }
  if arg.unit is Some(_) {
    return Some("unit")
  }
  None
}

///|
fn emit_char_lit(ch : Char) -> String {
  "'\\u{" + ch.to_int().to_string(radix=16) + "}'"
}

///|
fn parse_u32_metadata(value : String) -> UInt? {
  if value.is_empty() || value.length() > 10 {
    return None
  }
  let mut parsed : UInt64 = 0
  for ch in value {
    if ch < '0' || ch > '9' {
      return None
    }
    parsed = parsed * 10 + (ch.to_int() - '0'.to_int()).to_uint64()
    if parsed > 4294967295UL {
      return None
    }
  }
  Some(parsed.to_uint())
}

///|
fn structural_attr_allowed(surface : ToolArgSurface, attr : String) -> Bool {
  match (surface, attr) {
    (SurfacePositional, "required" | "default" | "accepts_stdio" | "value_name") =>
      true
    (SurfaceTail, "separator" | "verbatim" | "accepts_stdio" | "value_name") =>
      true
    (
      SurfaceOptionScalar,
      "short"
      | "alias"
      | "env"
      | "required"
      | "optional_scalar"
      | "default"
      | "value_name",
    ) => true
    (
      SurfaceOptionRepeated,
      "short"
      | "alias"
      | "env"
      | "required"
      | "repeatable"
      | "delim"
      | "default"
      | "value_name",
    ) => true
    (SurfaceBoolFlag, "short" | "alias" | "env" | "negatable" | "default") =>
      true
    (SurfaceCountFlag, "short" | "alias" | "env") => true
    (SurfaceStream | SurfacePrincipal, _) => false
    _ => false
  }
}

///|
fn validate_structural_attr(
  present : Bool,
  attr : String,
  surface : ToolArgSurface,
  source : String,
) -> Unit raise ToolEmitError {
  if present && !structural_attr_allowed(surface, attr) {
    emit_fail(
      "Argument '" +
      source +
      "' structural attribute '" +
      attr +
      "' is not valid on this surface",
    )
  }
}

///|
fn validate_structural_attrs(
  info : ToolArgInfo?,
  surface : ToolArgSurface,
  source : String,
) -> Unit raise ToolEmitError {
  guard info is Some(info) else { return }
  validate_structural_attr(info.short is Some(_), "short", surface, source)
  validate_structural_attr(info.aliases.length() > 0, "alias", surface, source)
  validate_structural_attr(info.env is Some(_), "env", surface, source)
  validate_structural_attr(
    info.required is Some(_),
    "required",
    surface,
    source,
  )
  validate_structural_attr(
    info.negatable is Some(_),
    "negatable",
    surface,
    source,
  )
  validate_structural_attr(
    info.optional_scalar is Some(_),
    "optional_scalar",
    surface,
    source,
  )
  validate_structural_attr(
    info.repeatable is Some(_),
    "repeatable",
    surface,
    source,
  )
  validate_structural_attr(info.delim is Some(_), "delim", surface, source)
  validate_structural_attr(
    info.defaults.length() > 0,
    "default",
    surface,
    source,
  )
  validate_structural_attr(
    info.separator is Some(_),
    "separator",
    surface,
    source,
  )
  validate_structural_attr(
    info.verbatim is Some(_),
    "verbatim",
    surface,
    source,
  )
  validate_structural_attr(
    info.accepts_stdio is Some(_),
    "accepts_stdio",
    surface,
    source,
  )
  validate_structural_attr(
    info.value_name is Some(_),
    "value_name",
    surface,
    source,
  )
}

///|
fn validate_hidden_attrs(
  info : ToolArgInfo?,
  hidden : ToolHiddenType,
  source : String,
) -> Unit raise ToolEmitError {
  let surface = if hidden == Principal {
    SurfacePrincipal
  } else {
    SurfaceStream
  }
  validate_structural_attrs(info, surface, source)
  guard info is Some(info) else { return }
  if hidden == Principal {
    emit_fail(
      "Principal parameter '" +
      source +
      "' does not accept #derive.arg metadata",
    )
  }
  if info.name is Some(_) ||
    info.scope is Some(_) ||
    info.kind is Some(_) ||
    info.regex is Some(_) ||
    info.min_length is Some(_) ||
    info.max_length is Some(_) ||
    info.direction is Some(_) ||
    info.schemes.length() > 0 ||
    info.min is Some(_) ||
    info.max is Some(_) ||
    info.bounds is Some(_) ||
    info.unit is Some(_) {
    emit_fail(
      "Stream parameter '" +
      source +
      "' accepts only documentation and MIME metadata",
    )
  }
}

///|
fn require_u32_metadata(
  value : String,
  attr : String,
  source : String,
) -> UInt raise ToolEmitError {
  match parse_u32_metadata(value) {
    Some(value) => value
    None => {
      emit_fail("Argument '" + source + "' " + attr + " must be a u32 literal")
      abort("")
    }
  }
}

///|
fn validate_u32_range(
  minimum : String?,
  maximum : String?,
  min_attr : String,
  max_attr : String,
  source : String,
) -> Unit raise ToolEmitError {
  let parsed_min = minimum.map(value => {
    require_u32_metadata(value, min_attr, source)
  })
  let parsed_max = maximum.map(value => {
    require_u32_metadata(value, max_attr, source)
  })
  if parsed_min is Some(minimum) &&
    parsed_max is Some(maximum) &&
    minimum > maximum {
    emit_fail(
      "Argument '" + source + "' " + min_attr + " cannot exceed " + max_attr,
    )
  }
}

///|
fn project_command(
  command : ToolCommandInfo,
  force_globals : Bool,
) -> Array[ProjectedToolArg] raise ToolEmitError {
  let projected : Array[ProjectedToolArg] = []
  let surfaces : Map[String, String] = {}
  let mut optional_positional = false
  let mut saw_tail = false
  if is_unit(command.success_type) && command.result is Some(_) {
    emit_fail(
      "Tool command '" +
      command.source_name +
      "' cannot declare result formatting for a unit success type",
    )
  }
  for param_index, pair in command.params {
    let (source, parameter) = pair
    match parameter {
      Hidden(OutputStream) | LabelledHidden(OutputStream) =>
        emit_fail(
          "Tool command '" +
          command.source_name +
          "' parameter '" +
          source +
          "' uses stdout, which is unsupported until the WASI stdout import is available",
        )
      Hidden(hidden) | LabelledHidden(hidden) => {
        let info = arg_info(command, source)
        validate_hidden_attrs(info, hidden, source)
        projected.push({
          source,
          canonical: source,
          aliases: [],
          type_: None,
          value_type: None,
          placement: Hidden(hidden),
          call_suffix: if parameter is LabelledHidden(_) {
            "~"
          } else {
            ""
          },
          info,
          required: true,
        })
      }
      Public(original)
      | LabelledPublic(original)
      | QuestionOptionalPublic(original) => {
        let info = arg_info(command, source)
        if info is Some(info) && info.bounds is Some(bounds) {
          let parts = bounds.split(",").to_array()
          if parts.length() != 2 || (parts[0].is_empty() && parts[1].is_empty()) {
            emit_fail(
              "Argument '" + source + "' bounds must use min,max encoding",
            )
          }
        }
        if info is Some(info) && info.defaults.length() > 0 {
          let (default_base, _) = unwrap_optional(original)
          if !is_map(default_base) &&
            list_item(default_base) is None &&
            info.kind != Some("flag") &&
            info.defaults.length() != 1 {
            emit_fail(
              "Scalar default for '" + source + "' requires exactly one value",
            )
          }
          if is_map(default_base) {
            for authored in info.defaults {
              if authored.find("=") is None {
                emit_fail("Map default for '" + source + "' must use key=value")
              }
            }
          }
        }
        let (base, optional) = unwrap_optional(original)
        let canonical = match info {
          Some(i) => i.name.unwrap_or(moonbit_name_to_kebab_case(source))
          None => moonbit_name_to_kebab_case(source)
        }
        let aliases = match info {
          Some(i) => i.aliases
          None => []
        }
        for surface in [canonical].concat(aliases) {
          if surfaces.get(surface) is Some(previous) {
            emit_fail(
              "Duplicate tool argument surface name '" +
              surface +
              "' on '" +
              previous +
              "' and '" +
              source +
              "'",
            )
          }
          surfaces[surface] = source
        }
        let scope = info.flat_map(i => i.scope)
        let kind = info.flat_map(i => i.kind)
        let bool_flag = base == Simple("Bool") &&
          scope != Some("positional") &&
          scope != Some("option") &&
          scope != Some("tail")
        let count_flag = kind == Some("count-flag")
        if count_flag && base != Simple("UInt") {
          emit_fail("count-flag argument '" + source + "' must have type UInt")
        }
        let explicit_flag = kind == Some("flag") || scope == Some("flag")
        if explicit_flag && base != Simple("Bool") {
          emit_fail("flag argument '" + source + "' must have type Bool")
        }
        let is_last_public = !command.params[param_index + 1:].any(p => {
          p.1 is Public(_) ||
          p.1 is LabelledPublic(_) ||
          p.1 is QuestionOptionalPublic(_)
        })
        let inferred_tail = !force_globals &&
          list_item(base) is Some(_) &&
          is_last_public &&
          scope is None &&
          kind is None
        let placement = if force_globals || scope == Some("global") {
          if bool_flag || count_flag || explicit_flag {
            GlobalFlag
          } else {
            GlobalOption
          }
        } else if explicit_flag || count_flag || bool_flag {
          Flag
        } else if scope == Some("tail") || inferred_tail {
          Tail
        } else if scope == Some("option") ||
          list_item(base) is Some(_) ||
          is_map(base) {
          Option
        } else {
          Positional
        }
        let surface = match placement {
          Positional => SurfacePositional
          Tail => SurfaceTail
          Option | GlobalOption =>
            if list_item(base) is Some(_) || is_map(base) {
              SurfaceOptionRepeated
            } else {
              SurfaceOptionScalar
            }
          Flag | GlobalFlag =>
            if count_flag {
              SurfaceCountFlag
            } else {
              SurfaceBoolFlag
            }
          Hidden(_) => abort("public parameter projected as hidden")
        }
        validate_structural_attrs(info, surface, source)
        if info is Some(info) {
          validate_u32_range(
            info.min_length,
            info.max_length,
            "min_length",
            "max_length",
            source,
          )
          if placement is Tail {
            validate_u32_range(info.min, info.max, "min", "max", source)
          }
          if surface == SurfaceBoolFlag && info.defaults.length() > 0 {
            if info.defaults.length() != 1 ||
              (info.defaults[0] != "true" && info.defaults[0] != "false") {
              emit_fail(
                "Boolean flag '" +
                source +
                "' default must be exactly true or false",
              )
            }
          }
          if (
              info.repeatable == Some("delimited") ||
              info.repeatable == Some("either")
            ) &&
            info.delim is None {
            emit_fail(
              "Argument '" +
              source +
              "' with repeatable='" +
              info.repeatable.unwrap() +
              "' requires delim",
            )
          }
          if info.delim is Some(_) &&
            info.repeatable != Some("delimited") &&
            info.repeatable != Some("either") {
            emit_fail(
              "Argument '" +
              source +
              "' declares delim without repeatable='delimited' or repeatable='either'",
            )
          }
        }
        if placement is Tail && optional {
          emit_fail("tail argument '" + source + "' cannot be optional")
        }
        if placement is Tail &&
          info.flat_map(i => i.verbatim) == Some(true) &&
          info.flat_map(i => i.separator) is None {
          emit_fail(
            "verbatim tail argument '" + source + "' requires a separator",
          )
        }
        if placement is Positional && saw_tail {
          emit_fail(
            "fixed positional argument '" +
            source +
            "' cannot follow a tail positional",
          )
        }
        if placement is Tail {
          saw_tail = true
        }
        if (placement is Flag || placement is GlobalFlag) && optional {
          emit_fail("flag argument '" + source + "' cannot be optional")
        }
        if info is Some(info) && (placement is Flag || placement is GlobalFlag) {
          if has_refinements(info) is Some(refinement) {
            emit_fail(
              "Argument '" +
              source +
              "' refinement '" +
              refinement +
              "' is not valid on a flag",
            )
          }
          if info.min is Some(_) {
            emit_fail("Argument '" + source + "' min is not valid on a flag")
          }
          if info.max is Some(_) && !count_flag {
            emit_fail(
              "Argument '" + source + "' max is only valid on a count-flag",
            )
          }
          if count_flag &&
            info.max is Some(max) &&
            parse_u32_metadata(max) is None {
            emit_fail(
              "Argument '" + source + "' count-flag max must be a u32 literal",
            )
          }
        }
        if info is Some(info) && is_map(base) {
          if has_refinements(info) is Some(refinement) {
            emit_fail(
              "Argument '" +
              source +
              "' refinement '" +
              refinement +
              "' is not valid on a map option",
            )
          }
          if info.min is Some(_) || info.max is Some(_) {
            emit_fail(
              "Argument '" +
              source +
              "' numeric refinements are not valid on a map option",
            )
          }
        }
        if info.flat_map(i => i.short) is Some(short) {
          let short_surface = short.to_string()
          if surfaces.get(short_surface) is Some(previous) {
            emit_fail(
              "Duplicate tool argument surface name '" +
              short_surface +
              "' on '" +
              previous +
              "' and '" +
              source +
              "'",
            )
          }
          surfaces[short_surface] = source
        }
        let required_by_default = placement is Positional
        let required = !optional &&
          info.flat_map(i => i.required).unwrap_or(required_by_default)
        if placement is Positional {
          if optional_positional && required {
            emit_fail(
              "required positional argument '" +
              source +
              "' follows an optional positional",
            )
          }
          if !required {
            optional_positional = true
          }
        }
        let value_type = if placement is Tail {
          list_item(base)
        } else {
          Some(base)
        }
        projected.push({
          source,
          canonical,
          aliases,
          type_: Some(original),
          value_type,
          placement,
          call_suffix: match parameter {
            LabelledPublic(_) => "~"
            QuestionOptionalPublic(_) => "?"
            _ => ""
          },
          info,
          required,
        })
      }
    }
  }
  if projected.filter(arg => arg.placement is Tail).length() > 1 {
    emit_fail(
      "Tool command '" +
      command.source_name +
      "' has more than one tail positional",
    )
  }
  projected
}

///|
fn graph(pt : ParamType) -> String {
  "@schema.into_schema_graph((@schema.TypeTag::{ } : @schema.TypeTag[" +
  emit_type(pt) +
  "]))"
}

///|
fn refined_graph(arg : ProjectedToolArg) -> String {
  let mut result = graph(arg.value_type.unwrap())
  guard arg.info is Some(info) else { return result }
  if info.regex is Some(_) ||
    info.min_length is Some(_) ||
    info.max_length is Some(_) {
    result = "@tool.refine_text(" +
      result +
      ", " +
      emit_opt_string(info.regex) +
      ", " +
      info.min_length
      .map(v => "Some(" + parse_u32_metadata(v).unwrap().to_string() + "U)")
      .unwrap_or("None") +
      ", " +
      info.max_length
      .map(v => "Some(" + parse_u32_metadata(v).unwrap().to_string() + "U)")
      .unwrap_or("None") +
      ")"
  }
  if info.direction is Some(_) ||
    info.mimes.length() > 0 ||
    info.kind == Some("file") ||
    info.kind == Some("dir") {
    let direction = info.direction
      .map(v => {
        let case_name = if v == "inout" { "IN_OUT" } else { v.to_upper() }
        "Some(@types.PathDirection::" + case_name + ")"
      })
      .unwrap_or("None")
    let kind = match info.kind {
      Some("file") => "Some(@types.PathKind::FILE)"
      Some("dir") => "Some(@types.PathKind::DIRECTORY)"
      _ => "None"
    }
    result = "@tool.refine_path(" +
      result +
      ", " +
      direction +
      ", " +
      kind +
      ", " +
      (if info.mimes.length() > 0 {
        "Some(" + emit_strings(info.mimes) + ")"
      } else {
        "None"
      }) +
      ")"
  }
  if info.schemes.length() > 0 {
    result = "@tool.refine_url(" +
      result +
      ", Some(" +
      emit_strings(info.schemes) +
      "))"
  }
  let mut minimum = info.min
  let mut maximum = info.max
  if info.bounds is Some(bounds) {
    let parts = bounds.split(",").to_array()
    if !parts[0].is_empty() {
      minimum = Some(parts[0].to_owned())
    }
    if !parts[1].is_empty() {
      maximum = Some(parts[1].to_owned())
    }
  }
  if minimum is Some(_) || maximum is Some(_) || info.unit is Some(_) {
    result = "@tool.refine_numeric(" +
      result +
      ", " +
      emit_opt_string(minimum) +
      ", " +
      emit_opt_string(maximum) +
      ", " +
      emit_opt_string(info.unit) +
      ")"
  }
  result
}

///|
fn emit_default(arg : ProjectedToolArg) -> String {
  guard arg.info is Some(info) && info.defaults.length() > 0 else {
    return "None"
  }
  let base = arg.value_type.unwrap()
  let literal = if is_map(base) {
    let entries : Array[String] = []
    for authored in info.defaults {
      match authored.find("=") {
        Some(i) if i > 0 =>
          entries.push(
            "(\"" +
            escape_string_lit(authored[:i].to_owned()) +
            "\", \"" +
            escape_string_lit(authored[i + 1:].to_owned()) +
            "\")",
          )
        _ => abort("map default validated while projecting")
      }
    }
    "@tool.ToolLiteral::Map([" + entries.join(", ") + "])"
  } else if list_item(base) is Some(_) {
    "@tool.ToolLiteral::List(" + emit_strings(info.defaults) + ")"
  } else {
    if info.defaults.length() != 1 {
      abort("scalar default validated while projecting")
    }
    "@tool.ToolLiteral::Scalar(\"" + escape_string_lit(info.defaults[0]) + "\")"
  }
  "Some(@tool.literal_to_schema_value(" +
  refined_graph(arg) +
  ", " +
  literal +
  "))"
}

///|
fn emit_repetition(info : ToolArgInfo?) -> String {
  match info.flat_map(i => i.repeatable) {
    Some("delimited") =>
      "@toolCommon.Repetition::Delimited(" +
      emit_char_lit(info.flat_map(i => i.delim).unwrap_or(',')) +
      ")"
    Some("either") =>
      "@toolCommon.Repetition::Either(" +
      emit_char_lit(info.flat_map(i => i.delim).unwrap_or(',')) +
      ")"
    _ => "@toolCommon.Repetition::Repeated"
  }
}

///|
fn emit_short(info : ToolArgInfo?) -> String {
  match info.flat_map(i => i.short) {
    Some(short) => "Some(" + emit_char_lit(short) + ")"
    None => "None"
  }
}

///|
fn emit_arg_doc(arg : ProjectedToolArg) -> String {
  emit_doc(arg.info.flat_map(i => i.doc).unwrap_or(""), [])
}

///|
fn emit_option(arg : ProjectedToolArg) -> String {
  let base = arg.value_type.unwrap()
  let shape = if is_map(base) {
    "@tool.OptionShapeDef::RepeatableMap({ repetition: " +
    emit_repetition(arg.info) +
    ", map_type: " +
    refined_graph(arg) +
    ", duplicate_key_policy: @toolCommon.DuplicateKeyPolicy::REJECT })"
  } else if list_item(base) is Some(item) {
    "@tool.OptionShapeDef::RepeatableList({ repetition: " +
    emit_repetition(arg.info) +
    ", item_type: " +
    refined_graph({ ..arg, value_type: Some(item) }) +
    " })"
  } else if arg.info.flat_map(i => i.optional_scalar) == Some(true) {
    "@tool.OptionShapeDef::OptionalScalar(" + refined_graph(arg) + ")"
  } else {
    "@tool.OptionShapeDef::Scalar(" + refined_graph(arg) + ")"
  }
  "{ long: \"" +
  escape_string_lit(arg.canonical) +
  "\", short: " +
  emit_short(arg.info) +
  ", aliases: " +
  emit_strings(arg.aliases) +
  ", doc: " +
  emit_arg_doc(arg) +
  ", value_name: " +
  emit_opt_string(arg.info.flat_map(i => i.value_name)) +
  ", shape: " +
  shape +
  ", default: " +
  emit_default(arg) +
  ", required: " +
  arg.required.to_string() +
  ", env_var: " +
  emit_opt_string(arg.info.flat_map(i => i.env)) +
  " }"
}

///|
fn emit_flag(arg : ProjectedToolArg) -> String {
  let count = arg.info.flat_map(i => i.kind) == Some("count-flag")
  let shape = if count {
    let maximum = match arg.info.flat_map(i => i.max) {
      Some(v) => "Some(" + parse_u32_metadata(v).unwrap().to_string() + "U)"
      None => "None"
    }
    "@toolCommon.FlagShape::CountFlag(" + maximum + ")"
  } else {
    let default = arg.info
      .map(i => i.defaults.contains("true"))
      .unwrap_or(false)
    "@toolCommon.FlagShape::BoolFlag({ default: " +
    default.to_string() +
    ", negatable: " +
    arg.info.flat_map(i => i.negatable).unwrap_or(false).to_string() +
    " })"
  }
  "{ long: \"" +
  escape_string_lit(arg.canonical) +
  "\", short: " +
  emit_short(arg.info) +
  ", aliases: " +
  emit_strings(arg.aliases) +
  ", doc: " +
  emit_arg_doc(arg) +
  ", shape: " +
  shape +
  ", env_var: " +
  emit_opt_string(arg.info.flat_map(i => i.env)) +
  " }"
}

///|
fn emit_positional(arg : ProjectedToolArg) -> String {
  "{ name: \"" +
  escape_string_lit(arg.canonical) +
  "\", doc: " +
  emit_arg_doc(arg) +
  ", value_name: " +
  emit_opt_string(arg.info.flat_map(i => i.value_name)) +
  ", type_: " +
  refined_graph(arg) +
  ", default: " +
  emit_default(arg) +
  ", required: " +
  arg.required.to_string() +
  ", accepts_stdio: " +
  arg.info.flat_map(i => i.accepts_stdio).unwrap_or(false).to_string() +
  " }"
}

///|
fn emit_tail(arg : ProjectedToolArg) -> String {
  let maximum = match arg.info.flat_map(i => i.max) {
    Some(v) => "Some(" + parse_u32_metadata(v).unwrap().to_string() + "U)"
    None => "None"
  }
  let item_arg = match arg.info {
    Some(info) => { ..arg, info: Some({ ..info, min: None, max: None }) }
    None => arg
  }
  "Some({ name: \"" +
  escape_string_lit(arg.canonical) +
  "\", doc: " +
  emit_arg_doc(arg) +
  ", value_name: " +
  emit_opt_string(arg.info.flat_map(i => i.value_name)) +
  ", item_type: " +
  refined_graph(item_arg) +
  ", min: " +
  arg.info
  .flat_map(i => i.min)
  .map(v => parse_u32_metadata(v).unwrap().to_string())
  .unwrap_or("0") +
  "U, max: " +
  maximum +
  ", separator: " +
  emit_opt_string(arg.info.flat_map(i => i.separator)) +
  ", verbatim: " +
  arg.info.flat_map(i => i.verbatim).unwrap_or(false).to_string() +
  ", accepts_stdio: " +
  arg.info.flat_map(i => i.accepts_stdio).unwrap_or(false).to_string() +
  " })"
}

///|
fn emit_present_ref(name : String) -> String {
  "@tool.RefDef::Present(\"" + escape_string_lit(name) + "\")"
}

///|
fn emit_value_is_ref(authored : String) -> String {
  let separator = authored.find("=").unwrap()
  let name = authored[:separator].to_owned()
  let value = authored[separator + 1:].to_owned()
  "@tool.RefDef::ValueIs({ name: \"" +
  escape_string_lit(name) +
  "\", value: @tool.ValueIsLiteralDef::Deferred(@tool.ToolLiteral::Scalar(\"" +
  escape_string_lit(value) +
  "\")) })"
}

///|
fn emit_refs(present : Array[String], value_is : Array[String]) -> String {
  let refs = present.map(emit_present_ref)
  for authored in value_is {
    refs.push(emit_value_is_ref(authored))
  }
  "[" + refs.join(", ") + "]"
}

///|
fn emit_constraints(command : ToolCommandInfo) -> String {
  let values : Array[String] = []
  for c in command.constraints {
    match c.kind {
      "requires_all" =>
        values.push(
          "@tool.ConstraintDef::RequiresAll(" +
          emit_refs(c.refs, c.value_is) +
          ")",
        )
      "requires_any" =>
        values.push(
          "@tool.ConstraintDef::RequiresAny(" +
          emit_refs(c.refs, c.value_is) +
          ")",
        )
      "all_or_none" =>
        values.push(
          "@tool.ConstraintDef::AllOrNone(" +
          emit_refs(c.refs, c.value_is) +
          ")",
        )
      "mutex_groups" =>
        values.push(
          "@tool.ConstraintDef::MutexGroups([" +
          c.refs
          .map(r => "{ refs: [" + emit_present_ref(r) + "] }")
          .concat(
            c.value_is.map(r => "{ refs: [" + emit_value_is_ref(r) + "] }"),
          )
          .join(", ") +
          "])",
        )
      "implies" =>
        values.push(
          "@tool.ConstraintDef::Implies({ lhs_quant: @toolCommon.Quantifier::" +
          (if c.lhs_quant == Some("any") { "ANY" } else { "ALL" }) +
          ", lhs: " +
          emit_refs(c.lhs, c.lhs_value_is) +
          ", rhs_quant: @toolCommon.Quantifier::" +
          (if c.rhs_quant == Some("any") { "ANY" } else { "ALL" }) +
          ", rhs: " +
          emit_refs(c.rhs, c.rhs_value_is) +
          " })",
        )
      "forbids" =>
        values.push(
          "@tool.ConstraintDef::Forbids({ lhs_quant: @toolCommon.Quantifier::" +
          (if c.lhs_quant == Some("any") { "ANY" } else { "ALL" }) +
          ", lhs: " +
          emit_refs(c.lhs, c.lhs_value_is) +
          ", rhs: " +
          emit_refs(c.rhs, c.rhs_value_is) +
          " })",
        )
      _ => ()
    }
  }
  "[" + values.join(", ") + "]"
}

///|
fn is_unit(pt : ParamType?) -> Bool {
  pt is None || pt == Some(Simple("Unit"))
}

///|
fn emit_result(command : ToolCommandInfo) -> String {
  if is_unit(command.success_type) {
    return "None"
  }
  let formatters = match command.result {
    Some(r) =>
      "[" +
      r.formatters
      .map(formatter => {
        "{ name: \"" +
        escape_string_lit(formatter) +
        "\", doc: " +
        emit_doc("", []) +
        " }"
      })
      .join(", ") +
      "]"
    None => "[{ name: \"default\", doc: " + emit_doc("", []) + " }]"
  }
  let default = command.result
    .map(r => r.defaults.get(0).unwrap_or(r.formatters[0]))
    .unwrap_or("default")
  "Some({ type_: " +
  graph(command.success_type.unwrap()) +
  ", doc: " +
  emit_doc(command.description, command.examples) +
  ", formatters: " +
  formatters +
  ", default_formatter: \"" +
  escape_string_lit(default) +
  "\" })"
}

///|
fn emit_errors(command : ToolCommandInfo) -> String {
  "[" +
  command.errors
  .map(error => {
    let exit = error.exit_code.unwrap_or(
      if error.kind.has_prefix("usage") {
        "2"
      } else {
        "1"
      },
    )
    let payload = match error.payload {
      Some(pt) => "Some(" + graph(pt) + ")"
      None => "None"
    }
    "{ name: \"" +
    escape_string_lit(moonbit_name_to_kebab_case(error.name)) +
    "\", doc: " +
    emit_doc(error.description, error.examples) +
    ", kind: @toolCommon.ErrorKind::" +
    (if error.kind.has_prefix("usage") {
      "USAGE_ERROR"
    } else {
      "RUNTIME_ERROR"
    }) +
    ", exit_code: " +
    exit +
    ", payload: " +
    payload +
    " }"
  })
  .join(", ") +
  "]"
}

///|
fn emit_stream(command : ToolCommandInfo, hidden : ToolHiddenType) -> String {
  for pair in command.params {
    if pair.1 == Hidden(hidden) || pair.1 == LabelledHidden(hidden) {
      let info = arg_info(command, pair.0)
      return "Some({ doc: " +
        emit_doc(info.flat_map(i => i.doc).unwrap_or(""), []) +
        ", mime: " +
        emit_strings(info.map(i => i.mimes).unwrap_or([])) +
        ", required: true })"
    }
  }
  "None"
}

///|
fn emit_body(
  command : ToolCommandInfo,
  args : Array[ProjectedToolArg],
) -> String {
  let fixed = args.filter(a => a.placement is Positional).map(emit_positional)
  let tail = args
    .iter()
    .find_first(a => a.placement is Tail)
    .map(emit_tail)
    .unwrap_or("None")
  let options = args.filter(a => a.placement is Option).map(emit_option)
  let flags = args.filter(a => a.placement is Flag).map(emit_flag)
  let annotations = "Some({ read_only: " +
    command.read_only.unwrap_or(false).to_string() +
    ", destructive: " +
    command.destructive.unwrap_or(true).to_string() +
    ", idempotent: " +
    command.idempotent.unwrap_or(false).to_string() +
    ", open_world: " +
    command.open_world.unwrap_or(true).to_string() +
    " })"
  "Some({ positionals: { fixed: [" +
  fixed.join(", ") +
  "], tail: " +
  tail +
  " }, options: [" +
  options.join(", ") +
  "], flags: [" +
  flags.join(", ") +
  "], constraints: " +
  emit_constraints(command) +
  ", stdin: " +
  emit_stream(command, InputStream) +
  ", stdout: None, result: " +
  emit_result(command) +
  ", errors: " +
  emit_errors(command) +
  ", annotations: " +
  annotations +
  " })"
}

///|
fn descriptor_name(tool : ToolInfo) -> String {
  "__golem_tool_def_" + tool.source_name
}

///|
fn subtree_target_name(target : String) -> String {
  let normalized = target.replace(old="::", new=".")
  let mut result = normalized
  for part in normalized.split(".") {
    result = part.to_owned()
  }
  result
}

///|
fn resolve_subtree_tool(
  tools : Array[ToolInfo],
  target : String,
) -> ToolInfo raise ToolEmitError {
  let source_name = subtree_target_name(target)
  let matches = tools.filter(tool => tool.source_name == source_name)
  if matches.length() == 0 {
    emit_fail("Subtree target '" + target + "' was not found")
  }
  if matches.length() > 1 {
    emit_fail("Subtree target '" + target + "' is ambiguous")
  }
  matches[0]
}

///|
fn validate_subtree_visit(
  tools : Array[ToolInfo],
  tool : ToolInfo,
  stack : Array[String],
  complete : Map[String, Unit],
) -> Unit raise ToolEmitError {
  if complete.contains(tool.source_name) {
    return
  }
  if stack.contains(tool.source_name) {
    emit_fail(
      "Tool subtree cycle: " + stack.concat([tool.source_name]).join(" -> "),
    )
  }
  let child_stack = stack.concat([tool.source_name])
  for command in tool.commands {
    if command.subtree is Some(target) {
      validate_subtree_visit(
        tools,
        resolve_subtree_tool(tools, target),
        child_stack,
        complete,
      )
    }
  }
  complete[tool.source_name] = ()
}

///|
fn validate_subtree_cycles(tools : Array[ToolInfo]) -> Unit raise ToolEmitError {
  let complete : Map[String, Unit] = {}
  for tool in tools {
    validate_subtree_visit(tools, tool, [], complete)
  }
}

///|
fn validate_sibling_commands(tool : ToolInfo) -> Unit raise ToolEmitError {
  let surfaces : Map[String, String] = {}
  for index, command in tool.commands {
    if Some(index) == tool.root_command {
      continue
    }
    for surface in [command.name].concat(command.aliases) {
      match surfaces.get(surface) {
        Some(previous) =>
          emit_fail(
            "Duplicate sibling command surface '" +
            surface +
            "' on '" +
            previous +
            "' and '" +
            command.source_name +
            "'",
          )
        None => surfaces[surface] = command.source_name
      }
    }
  }
}

///|
fn projected_surfaces(args : Array[ProjectedToolArg]) -> Array[String] {
  let surfaces : Array[String] = []
  for arg in args {
    if arg.placement is Hidden(_) {
      continue
    }
    surfaces.push(arg.canonical)
    for surface_alias in arg.aliases {
      surfaces.push(surface_alias)
    }
    if arg.info.flat_map(info => info.short) is Some(short) {
      surfaces.push(short.to_string())
    }
  }
  surfaces
}

///|
fn validate_inherited_arg_surfaces(
  command : ToolCommandInfo,
  args : Array[ProjectedToolArg],
  inherited : Array[String],
) -> Unit raise ToolEmitError {
  for arg in args {
    if arg.placement is GlobalOption || arg.placement is GlobalFlag {
      continue
    }
    for surface in projected_surfaces([arg]) {
      if inherited.contains(surface) {
        emit_fail(
          "Argument surface '" +
          surface +
          "' on command '" +
          command.source_name +
          "' conflicts with an inherited global argument",
        )
      }
    }
  }
}

///|
fn validate_tool_inherited_surfaces(
  tools : Array[ToolInfo],
  tool : ToolInfo,
  inherited : Array[String],
) -> Unit raise ToolEmitError {
  let plans : Array[Array[ProjectedToolArg]] = []
  for command in tool.commands {
    plans.push(project_command(command, command.subtree is Some(_)))
  }
  let root = tool.root_command
  let root_args = root.map(index => plans[index]).unwrap_or([])
  if root is Some(index) {
    validate_inherited_arg_surfaces(tool.commands[index], root_args, inherited)
  }
  let inherited_here = inherited.concat(
    projected_surfaces(
      root_args.filter(arg => {
        arg.placement is GlobalOption || arg.placement is GlobalFlag
      }),
    ),
  )
  for index, command in tool.commands {
    if Some(index) == root {
      continue
    }
    validate_inherited_arg_surfaces(command, plans[index], inherited_here)
    if command.subtree is Some(target) {
      validate_tool_inherited_surfaces(
        tools,
        resolve_subtree_tool(tools, target),
        inherited_here.concat(projected_surfaces(plans[index])),
      )
    }
  }
}

///|
fn validate_inherited_surfaces(
  tools : Array[ToolInfo],
) -> Unit raise ToolEmitError {
  for tool in tools {
    validate_tool_inherited_surfaces(tools, tool, [])
  }
}

///|
fn value_is_name(authored : String) -> String {
  let separator = authored.find("=").unwrap()
  authored[:separator].to_owned()
}

///|
fn constraint_ref_names(constraint : ToolConstraintInfo) -> Array[String] {
  let names = constraint.refs.concat(constraint.lhs).concat(constraint.rhs)
  for
    authored in constraint.value_is
    .concat(constraint.lhs_value_is)
    .concat(constraint.rhs_value_is) {
    names.push(value_is_name(authored))
  }
  names
}

///|
fn validate_constraint_refs(
  command : ToolCommandInfo,
  args : Array[ProjectedToolArg],
  inherited : Array[ProjectedToolArg],
) -> Unit raise ToolEmitError {
  let surfaces = projected_surfaces(args).concat(projected_surfaces(inherited))
  for constraint in command.constraints {
    for name in constraint_ref_names(constraint) {
      if !surfaces.contains(name) {
        emit_fail(
          "Constraint reference '" +
          name +
          "' on command '" +
          command.source_name +
          "' does not resolve to a declared argument",
        )
      }
    }
  }
}

///|
fn emit_globals(args : Array[ProjectedToolArg]) -> String {
  let options = args
    .filter(arg => arg.placement is GlobalOption)
    .map(emit_option)
  let flags = args.filter(arg => arg.placement is GlobalFlag).map(emit_flag)
  "{ options: [" + options.join(", ") + "], flags: [" + flags.join(", ") + "] }"
}

///|
fn emit_descriptor(
  buf : StringBuilder,
  tool : ToolInfo,
  tools : Array[ToolInfo],
) -> Unit raise ToolEmitError {
  let plans : Array[Array[ProjectedToolArg]] = []
  for command in tool.commands {
    plans.push(project_command(command, command.subtree is Some(_)))
  }
  buf.write_string(
    "///|\nfn " +
    descriptor_name(tool) +
    "() -> @tool.ToolDef {\n  try {\n    let def : @tool.ToolDef = { version: \"" +
    escape_string_lit(tool.version.unwrap_or("0.0.0")) +
    "\", commands: [\n",
  )
  let root = tool.root_command
  let root_args = root.map(i => plans[i]).unwrap_or([])
  let inherited_root_args = root_args.filter(arg => {
    arg.placement is GlobalOption || arg.placement is GlobalFlag
  })
  for index, command in tool.commands {
    validate_constraint_refs(
      command,
      plans[index],
      if Some(index) == root {
        []
      } else {
        inherited_root_args
      },
    )
  }
  let globals_options = root_args
    .filter(a => a.placement is GlobalOption)
    .map(emit_option)
  let globals_flags = root_args
    .filter(a => a.placement is GlobalFlag)
    .map(emit_flag)
  let leaves : Array[Int] = []
  for i in 0..<tool.commands.length() {
    if Some(i) != root && tool.commands[i].subtree is None {
      leaves.push(i)
    }
  }
  let emitted_leaf_indices : Array[String] = []
  for i in 0..<leaves.length() {
    emitted_leaf_indices.push((i + 1).to_string())
  }
  let root_body = root
    .map(i => emit_body(tool.commands[i], plans[i]))
    .unwrap_or("None")
  let root_aliases = root
    .map(i => emit_strings(tool.commands[i].aliases))
    .unwrap_or("[]")
  buf.write_string(
    "    { name: \"" +
    escape_string_lit(tool.name) +
    "\", aliases: " +
    root_aliases +
    ", doc: " +
    emit_doc(tool.description, tool.examples) +
    ", globals: { options: [" +
    globals_options.join(", ") +
    "], flags: [" +
    globals_flags.join(", ") +
    "] }, subcommands: [" +
    emitted_leaf_indices.join(", ") +
    "], body: " +
    root_body +
    " },\n",
  )
  for i in leaves {
    let command = tool.commands[i]
    buf.write_string(
      "    { name: \"" +
      escape_string_lit(command.name) +
      "\", aliases: " +
      emit_strings(command.aliases) +
      ", doc: " +
      emit_doc(command.description, command.examples) +
      ", globals: " +
      emit_globals(plans[i]) +
      ", subcommands: [], body: " +
      emit_body(command, plans[i]) +
      " },\n",
    )
  }
  buf.write_string("  ] }\n")
  for i, command in tool.commands {
    if command.subtree is Some(target) {
      let child = resolve_subtree_tool(tools, target)
      let explicit_name = command.name !=
        moonbit_name_to_kebab_case(command.source_name)
      let override_name = if explicit_name {
        "Some(\"" + escape_string_lit(command.name) + "\")"
      } else {
        "None"
      }
      let override_doc = if command.description.is_empty() {
        "None"
      } else {
        "Some(" + emit_doc(command.description, command.examples) + ")"
      }
      let override_aliases = if command.aliases.is_empty() {
        "None"
      } else {
        "Some(" + emit_strings(command.aliases) + ")"
      }
      buf.write_string(
        "  let def = def.graft_subtree(" +
        descriptor_name(child) +
        "(), \"" +
        escape_string_lit(command.name) +
        "\", " +
        emit_globals(plans[i]) +
        ", " +
        override_name +
        ", " +
        override_doc +
        ", " +
        override_aliases +
        ")\n",
      )
    }
  }
  buf.write_string(
    "    def.normalize_inherited_globals()\n  } catch { error => abort(\"invalid generated tool definition: \" + repr(error)) }\n}\n\n",
  )
  emit_invoker(buf, tool, tools, plans, leaves)
}

///|
fn emit_invocation_value(value : String) -> String {
  "Ok(@tool.invocation_result_value(" +
  value +
  ") catch { error => return Err(error) })"
}

///|
fn emit_custom_error(command : ToolCommandInfo, error_value : String) -> String {
  let arms = command.errors.map(error => {
    let pattern = match error.payload {
      Some(_) => error.name + "(payload)"
      None => error.name
    }
    let payload = if error.payload is Some(_) { "payload" } else { "()" }
    pattern +
    " => {\nlet payload = @schema.try_into_typed_schema_value(" +
    payload +
    ") catch { encode_error => return Err(@toolCommon.ToolError::InvalidResult(\"failed serializing custom tool error: \" + encode_error.to_string())) }\nlet payload = @model.typed_schema_value_to_wit(payload) catch { encode_error => return Err(@toolCommon.ToolError::InvalidResult(\"failed serializing custom tool error: \" + encode_error.to_string())) }\nErr(@toolCommon.ToolError::CustomError(payload))\n}"
  })
  "match " + error_value + " {\n" + arms.join("\n") + "\n}"
}

///|
fn emit_raised_error_capture(command : ToolCommandInfo) -> String {
  command.errors
  .map(error => {
    match error.payload {
      Some(_) => error.name + "(payload) => Err(" + error.name + "(payload))"
      None => error.name + " => Err(" + error.name + ")"
    }
  })
  .join("\n")
}

///|
fn emit_success_call(command : ToolCommandInfo, call : String) -> String {
  if command.error_type is Some(_) {
    if command.error_via_result {
      "match " +
      call +
      " { Ok(value) => " +
      (if is_unit(command.success_type) {
        "Ok(@tool.invocation_result_empty())"
      } else {
        emit_invocation_value("value")
      }) +
      "\nErr(error) => " +
      emit_custom_error(command, "error") +
      " }"
    } else {
      "let outcome = try { Ok(" +
      call +
      ") } catch { " +
      emit_raised_error_capture(command) +
      " }\nmatch outcome { Ok(value) => " +
      (if is_unit(command.success_type) {
        "Ok(@tool.invocation_result_empty())"
      } else {
        emit_invocation_value("value")
      }) +
      "\nErr(error) => " +
      emit_custom_error(command, "error") +
      " }"
    }
  } else if is_unit(command.success_type) {
    "let _ = " + call + "; Ok(@tool.invocation_result_empty())"
  } else {
    "let value = " + call + "; " + emit_invocation_value("value")
  }
}

///|
fn emit_invoker(
  buf : StringBuilder,
  tool : ToolInfo,
  tools : Array[ToolInfo],
  plans : Array[Array[ProjectedToolArg]],
  leaves : Array[Int],
) -> Unit raise ToolEmitError {
  let name = "__golem_tool_invoke_" + tool.source_name
  let context = "tool " + tool.name + " invocation"
  buf.write_string(
    "///|\nfn " +
    name +
    "(__golem_command_path : Array[String], __golem_input : @types.TypedSchemaValue, __golem_stdin : @streams.InputStream?, __golem_principal : @agentCommon.Principal) -> Result[@toolCommon.InvocationResult, @toolCommon.ToolError] {\n  let __golem_def = " +
    descriptor_name(tool) +
    "()\n  let __golem_index = match __golem_def.command_index_by_path(__golem_command_path) {\n    Some(__golem_index) => __golem_index\n    None => return Err(@toolCommon.ToolError::InvalidCommandPath(__golem_command_path))\n  }\n  let __golem_decoded = @tool.decode_canonical_input(__golem_def, __golem_index, __golem_input, \"" +
    escape_string_lit(context) +
    "\") catch { __golem_error => return Err(__golem_error) }\n",
  )
  for command in tool.commands {
    if command.subtree is Some(target) {
      let child_name = resolve_subtree_tool(tools, target).name
      let names = [command.name].concat(command.aliases)
      buf.write_string(
        "  if __golem_command_path.length() > 0 && " +
        emit_strings(names) +
        ".contains(__golem_command_path[0]) {\n    return @tool.invoke_registered_subtool(\"" +
        escape_string_lit(child_name) +
        "\", __golem_command_path[1:].to_array(), __golem_decoded, __golem_stdin, __golem_principal)\n  }\n",
      )
    }
  }
  buf.write_string("  match __golem_index {\n")
  let commands : Array[(Int, Int)] = []
  if tool.root_command is Some(root) {
    commands.push((0, root))
  }
  for out_index, command_index in leaves {
    commands.push((out_index + 1, command_index))
  }
  for pair in commands {
    let command = tool.commands[pair.1]
    let args = plans[pair.1]
    let command_context = "tool " + tool.name + " command " + command.name
    buf.write_string("    " + pair.0.to_string() + " => {\n")
    let call_args : Array[String] = []
    for arg in args {
      match arg.placement {
        Hidden(Principal) => call_args.push("__golem_principal")
        Hidden(InputStream) => {
          buf.write_string(
            "      let " +
            arg.source +
            " = match __golem_stdin { Some(__golem_stream) => __golem_stream, None => return Err(@toolCommon.ToolError::InvalidInput(\"required stdin stream is missing\")) }\n",
          )
          call_args.push(arg.source)
        }
        Hidden(OutputStream) => ()
        _ => {
          buf.write_string(
            "      let " +
            arg.source +
            " : " +
            emit_type(arg.type_.unwrap()) +
            " = @tool.decode_canonical_field(__golem_decoded, \"" +
            escape_string_lit(arg.canonical) +
            "\", \"" +
            escape_string_lit(command_context) +
            "\") catch { __golem_error => return Err(__golem_error) }\n",
          )
          call_args.push(arg.source)
        }
      }
    }
    let invocation = tool.source_name +
      "::" +
      command.source_name +
      "(" +
      ({
        let emitted_call_args : Array[String] = []
        for index, value in call_args {
          emitted_call_args.push(value + args[index].call_suffix)
        }
        emitted_call_args.join(", ")
      }) +
      ")"
    buf.write_string(
      "      " + emit_success_call(command, invocation) + "\n    }\n",
    )
  }
  buf.write_string(
    "    _ => Err(@toolCommon.ToolError::InvalidCommandPath(__golem_command_path))\n  }\n}\n\n",
  )
}

///|
pub fn generate_tool_registration(
  tools : Array[ToolInfo],
) -> @list.List[@syntax.Impl] raise ToolEmitError {
  validate_subtree_cycles(tools)
  validate_inherited_surfaces(tools)
  for tool in tools {
    validate_sibling_commands(tool)
  }
  let buf = StringBuilder::new()
  for tool in tools {
    emit_descriptor(buf, tool, tools)
  }
  if tools.length() > 0 {
    buf.write_string("///|\nfn init {\n  try {\n")
    for tool in tools {
      buf.write_string(
        "    @tool.register_tool_def_invoker(" +
        descriptor_name(tool) +
        "(), __golem_tool_invoke_" +
        tool.source_name +
        ")\n",
      )
    }
    buf.write_string(
      "  } catch { error => abort(\"failed to register generated tools: \" + repr(error)) }\n}\n",
    )
  }
  parse_generated_impls(buf.to_string(), "tool registration")
}

///|
fn client_args(
  own : Array[ProjectedToolArg],
  inherited : Array[ProjectedToolArg],
) -> Array[ProjectedToolArg] {
  let result : Array[ProjectedToolArg] = []
  let own_surfaces : Map[String, Unit] = {}
  for arg in own {
    own_surfaces[arg.canonical] = ()
    for surface_alias in arg.aliases {
      own_surfaces[surface_alias] = ()
    }
  }
  for arg in inherited {
    if !own_surfaces.contains(arg.canonical) && !(arg.placement is Hidden(_)) {
      result.push(arg)
    }
  }
  for arg in own {
    if !(arg.placement is Hidden(Principal | OutputStream)) {
      result.push(arg)
    }
  }
  result
}

///|
fn arg_surface_names(arg : ProjectedToolArg) -> Array[String] {
  [arg.canonical].concat(arg.aliases)
}

///|
fn without_captured_args(
  args : Array[ProjectedToolArg],
  captured : Array[String],
) -> Array[ProjectedToolArg] {
  args.filter(arg => {
    !arg_surface_names(arg).any(name => captured.contains(name))
  })
}

///|
fn extend_captured(
  captured : Array[String],
  args : Array[ProjectedToolArg],
) -> Array[String] {
  let result = captured.copy()
  for arg in args {
    for name in arg_surface_names(arg) {
      if !result.contains(name) {
        result.push(name)
      }
    }
  }
  result
}

///|
fn wrapper_name(root : ToolInfo, path : Array[String]) -> String {
  root.client_pascal_case() + path.map(wire_to_pascal_case).join("") + "Client"
}

///|
fn error_decoder_name(receiver : String, command : ToolCommandInfo) -> String {
  "__golem_tool_decode_error_" + receiver + "_" + command.source_name
}

///|
fn emit_error_decoder(
  buf : StringBuilder,
  receiver : String,
  command : ToolCommandInfo,
) -> Unit {
  guard command.error_type is Some(error_type) else { return }
  buf.write_string(
    "///|\nfn " +
    error_decoder_name(receiver, command) +
    "(__golem_value : @model.TypedSchemaValue) -> Result[" +
    emit_type(error_type) +
    ", String] {\n",
  )
  for error in command.errors {
    let payload_type = error.payload.map(emit_type).unwrap_or("Unit")
    let error_value = match error.payload {
      Some(_) => error.name + "(__golem_payload)"
      None => error.name
    }
    buf.write_string(
      "  let __golem_decoded : " +
      payload_type +
      "? = try { Some(@schema.from_value_as(__golem_value.value)) } catch { _ => None }\n  if __golem_decoded is Some(__golem_payload) { return Ok(" +
      error_value +
      ") }\n",
    )
  }
  buf.write_string(
    "  Err(\"remote tool error payload did not match any declared error case\")\n}\n\n",
  )
}

///|
fn emit_tool_client_method(
  buf : StringBuilder,
  root : ToolInfo,
  receiver : String,
  command : ToolCommandInfo,
  args : Array[ProjectedToolArg],
  path : Array[String],
  has_prefix : Bool,
) -> Unit {
  let signature = args
    .map(arg => {
      arg.source +
      " : " +
      emit_type(arg.type_.unwrap_or(Simple("@streams.InputStream")))
    })
    .join(", ")
  let error = command.error_type.map(emit_type).unwrap_or("@tool.NoToolError")
  let success = command.success_type.map(emit_type).unwrap_or("Unit")
  emit_error_decoder(buf, receiver, command)
  buf.write_string(
    "///|\npub fn " +
    receiver +
    "::" +
    command.source_name +
    "(self : " +
    receiver +
    (if signature.is_empty() { "" } else { ", " + signature }) +
    ") -> Result[" +
    success +
    ", @tool.ToolError[" +
    error +
    "]] {\n",
  )
  buf.write_string(
    "  let __golem_def = " +
    descriptor_name(root) +
    "()\n  let __golem_index = match __golem_def.command_index_by_path(" +
    emit_strings(path) +
    ") {\n    Some(__golem_index) => __golem_index\n    None => return Err(@tool.tool_protocol_error(\"generated command path is missing from descriptor\"))\n  }\n  let __golem_model = __golem_def.canonical_input_model(__golem_index) catch { __golem_error => return Err(@tool.tool_protocol_error(repr(__golem_error))) }\n  let __golem_values = " +
    (if has_prefix { "self.prefix.copy()" } else { "[]" }) +
    "\n",
  )
  let mut stdin = "None"
  for arg in args {
    match arg.placement {
      Hidden(InputStream) => stdin = "Some(" + arg.source + ")"
      Hidden(_) => ()
      _ =>
        buf.write_string(
          "  __golem_values.push((\"" +
          escape_string_lit(arg.canonical) +
          "\", @schema.to_value_as(" +
          arg.source +
          ")))\n",
        )
    }
  }
  buf.write_string(
    "  let __golem_input = @tool.build_canonical_input(__golem_model, __golem_values) catch { __golem_error => return Err(@tool.tool_protocol_error(repr(__golem_error))) }\n",
  )
  let invoke = if command.error_type is Some(_) {
    "self.client.invoke_and_await"
  } else {
    "self.client.invoke_and_await_no_tool_error"
  }
  buf.write_string(
    "  match " +
    invoke +
    "(" +
    emit_strings(path) +
    ", __golem_input, " +
    stdin +
    (if command.error_type is Some(_) {
      ", " + error_decoder_name(receiver, command)
    } else {
      ""
    }) +
    ") {\n    Err(__golem_error) => Err(__golem_error)\n    Ok(__golem_result) => @tool." +
    (if is_unit(command.success_type) {
      "decode_result_empty"
    } else {
      "decode_result_value"
    }) +
    "(__golem_result) }\n}\n\n",
  )
}

///|
fn emit_subtree_navigation(
  buf : StringBuilder,
  receiver : String,
  child_receiver : String,
  command : ToolCommandInfo,
  args : Array[ProjectedToolArg],
  has_prefix : Bool,
) -> Unit {
  let signature = args
    .map(arg => arg.source + " : " + emit_type(arg.type_.unwrap()))
    .join(", ")
  buf.write_string(
    "///|\npub fn " +
    receiver +
    "::" +
    command.source_name +
    "(self : " +
    receiver +
    (if signature.is_empty() { "" } else { ", " + signature }) +
    ") -> " +
    child_receiver +
    " {\n  let __golem_prefix = " +
    (if has_prefix { "self.prefix.copy()" } else { "[]" }) +
    "\n",
  )
  for arg in args {
    buf.write_string(
      "  __golem_prefix.push((\"" +
      escape_string_lit(arg.canonical) +
      "\", @schema.to_value_as(" +
      arg.source +
      ")))\n",
    )
  }
  buf.write_string("  { client: self.client, prefix: __golem_prefix }\n}\n\n")
}

///|
fn emit_client_scope(
  buf : StringBuilder,
  root : ToolInfo,
  current : ToolInfo,
  tools : Array[ToolInfo],
  receiver : String,
  path : Array[String],
  captured : Array[String],
  has_prefix : Bool,
) -> Unit raise ToolEmitError {
  let plans : Array[Array[ProjectedToolArg]] = []
  for command in current.commands {
    plans.push(project_command(command, command.subtree is Some(_)))
  }
  let root_globals = current.root_command
    .map(index => {
      plans[index].filter(arg => {
        arg.placement is GlobalOption || arg.placement is GlobalFlag
      })
    })
    .unwrap_or([])
  if current.root_command is Some(index) {
    let args = without_captured_args(plans[index], captured)
    emit_tool_client_method(
      buf,
      root,
      receiver,
      current.commands[index],
      args,
      path,
      has_prefix,
    )
  }
  for index, command in current.commands {
    if Some(index) != current.root_command {
      let inherited_args = client_args(plans[index], root_globals)
      let args = without_captured_args(inherited_args, captured)
      let command_path = path.concat([command.name])
      match command.subtree {
        Some(target) => {
          let child = resolve_subtree_tool(tools, target)
          let child_receiver = wrapper_name(root, command_path)
          buf.write_string(
            "///|\npub(all) struct " +
            child_receiver +
            " { client : @tool.ToolClient; prefix : Array[(String, @model.SchemaValue)] }\n\n///|\npub fn " +
            child_receiver +
            "::drop(self : " +
            child_receiver +
            ") -> Unit { self.client.drop() }\n\n",
          )
          emit_subtree_navigation(
            buf, receiver, child_receiver, command, args, has_prefix,
          )
          emit_client_scope(
            buf,
            root,
            child,
            tools,
            child_receiver,
            command_path,
            extend_captured(captured, args),
            true,
          )
        }
        None =>
          emit_tool_client_method(
            buf, root, receiver, command, args, command_path, has_prefix,
          )
      }
    }
  }
}

///|
pub fn generate_tool_client_code(
  tools : Array[ToolInfo],
) -> @list.List[@syntax.Impl] raise ToolEmitError {
  validate_subtree_cycles(tools)
  validate_inherited_surfaces(tools)
  for tool in tools {
    validate_sibling_commands(tool)
  }
  let buf = StringBuilder::new()
  for tool in tools {
    let client = tool.client_pascal_case() + "Client"
    buf.write_string(
      "///|\npub(all) struct " +
      client +
      " { client : @tool.ToolClient }\n\n///|\npub fn " +
      client +
      "::new() -> " +
      client +
      " { { client: @tool.ToolClient::new(\"" +
      escape_string_lit(tool.name) +
      "\") } }\n\n///|\npub fn " +
      client +
      "::drop(self : " +
      client +
      ") -> Unit { self.client.drop() }\n\n",
    )
    emit_client_scope(buf, tool, tool, tools, client, [], [], false)
  }
  parse_generated_impls(buf.to_string(), "tool clients")
}
