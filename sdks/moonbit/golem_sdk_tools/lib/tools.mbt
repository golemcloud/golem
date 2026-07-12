///|
pub suberror ToolParseError {
  ToolParseError(String)
} derive(Debug)

///|
pub(all) enum ToolHiddenType {
  Principal
  InputStream
  OutputStream
} derive(Debug, Eq)

///|
pub(all) enum ToolParamType {
  Public(ParamType)
  LabelledPublic(ParamType)
  QuestionOptionalPublic(ParamType)
  Hidden(ToolHiddenType)
  LabelledHidden(ToolHiddenType)
} derive(Debug, Eq)

///|
pub(all) struct ToolExample {
  body : String
  title : String?
} derive(Debug, Eq)

///|
pub(all) struct ToolArgInfo {
  source_param : String
  name : String?
  scope : String?
  kind : String?
  short : Char?
  aliases : Array[String]
  env : String?
  required : Bool?
  negatable : Bool?
  optional_scalar : Bool?
  repeatable : String?
  delim : Char?
  defaults : Array[String]
  separator : String?
  verbatim : Bool?
  accepts_stdio : Bool?
  regex : String?
  min_length : String?
  max_length : String?
  direction : String?
  mimes : Array[String]
  schemes : Array[String]
  min : String?
  max : String?
  bounds : String?
  unit : String?
  doc : String?
  value_name : String?
} derive(Debug, Eq)

///|
pub(all) struct ToolConstraintInfo {
  kind : String
  refs : Array[String]
  lhs : Array[String]
  rhs : Array[String]
  lhs_quant : String?
  rhs_quant : String?
  lhs_value_is : Array[String]
  rhs_value_is : Array[String]
  value_is : Array[String]
} derive(Debug, Eq)

///|
pub(all) struct ToolResultInfo {
  formatters : Array[String]
  options : Array[(String, String)]
  defaults : Array[String]
} derive(Debug, Eq)

///|
pub(all) struct ToolErrorCaseInfo {
  name : String
  payload : ParamType?
  kind : String
  exit_code : String?
  description : String
  examples : Array[ToolExample]
} derive(Debug, Eq)

///|
pub(all) struct ToolCommandInfo {
  source_name : String
  name : String
  aliases : Array[String]
  subtree : String?
  read_only : Bool?
  destructive : Bool?
  idempotent : Bool?
  open_world : Bool?
  params : Array[(String, ToolParamType)]
  args : Array[ToolArgInfo]
  success_type : ParamType?
  error_type : ParamType?
  error_via_result : Bool
  errors : Array[ToolErrorCaseInfo]
  result : ToolResultInfo?
  constraints : Array[ToolConstraintInfo]
  examples : Array[ToolExample]
  description : String
  is_root : Bool
} derive(Debug, Eq)

///|
pub(all) struct ToolInfo {
  source_name : String
  name : String
  version : String?
  description : String
  examples : Array[ToolExample]
  commands : Array[ToolCommandInfo]
  mut root_command : Int?
} derive(Debug, Eq)

///|
pub fn ToolInfo::wire_name(self : Self) -> String {
  self.name
}

///|
pub fn ToolInfo::client_pascal_case(self : Self) -> String {
  wire_to_pascal_case(self.name)
}

///|
pub fn moonbit_name_to_kebab_case(name : String) -> String {
  let out = StringBuilder::new()
  let chars = name.to_array()
  let mut wrote = false
  let mut dash = false
  for i, ch in chars {
    if ch == '_' || ch == '-' {
      if wrote && !dash {
        out.write_char('-')
        dash = true
      }
    } else {
      let upper = ch >= 'A' && ch <= 'Z'
      let prev_lower = i > 0 && chars[i - 1] >= 'a' && chars[i - 1] <= 'z'
      let next_lower = i + 1 < chars.length() &&
        chars[i + 1] >= 'a' &&
        chars[i + 1] <= 'z'
      if upper && i > 0 && (prev_lower || next_lower) {
        out.write_char('-')
      }
      if upper {
        out.write_char((ch.to_int() + 32).unsafe_to_char())
      } else {
        out.write_char(ch)
      }
      wrote = true
      dash = false
    }
  }
  out.to_string()
}

///|
pub fn wire_to_pascal_case(name : String) -> String {
  let out = StringBuilder::new()
  let mut upper = true
  for ch in name {
    if ch == '-' || ch == '_' {
      upper = true
    } else if upper {
      out.write_char((ch.to_int() - 32).unsafe_to_char())
      upper = false
    } else {
      out.write_char(ch)
    }
  }
  out.to_string()
}

///|
fn tool_fail(message : String) -> Unit raise ToolParseError {
  raise ToolParseError(message)
}

///|
fn tool_doc(doc : @syntax.DocString) -> String {
  let lines = doc.content.to_array()
  lines.join("\n").trim().to_string()
}

///|
fn attr_id(attr : @attribute.Attribute) -> (String?, String)? {
  match attr.parsed {
    Some(Ident(id) | Apply(id, _)) => Some((id.qual, id.name))
    _ => {
      for name in ["arg", "constraint", "command", "result", "example"] {
        if attr.raw.has_prefix("#derive." + name) {
          return Some((Some("derive"), name))
        }
      }
      None
    }
  }
}

///|
fn derive_props(
  attr : @attribute.Attribute,
  name : String,
) -> Array[@attribute.Prop]? {
  match attr.parsed {
    Some(Ident({ qual: Some("derive"), name: n })) if n == name => Some([])
    Some(Apply({ qual: Some("derive"), name: n }, props)) if n == name =>
      Some(props.to_array())
    _ => None
  }
}

///|
fn attribute_space(ch : Char) -> Bool {
  ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r'
}

///|
fn rewrite_command_alias_label(raw : String) -> String {
  let chars = raw.to_array()
  let buf = StringBuilder::new()
  let mut i = 0
  let mut in_string = false
  let mut escaped = false
  while i < chars.length() {
    let ch = chars[i]
    if in_string {
      buf.write_char(ch)
      if escaped {
        escaped = false
      } else if ch == '\\' {
        escaped = true
      } else if ch == '"' {
        in_string = false
      }
      i = i + 1
      continue
    }
    if ch == '"' {
      in_string = true
      buf.write_char(ch)
      i = i + 1
      continue
    }
    let is_alias = i + 5 <= chars.length() &&
      chars[i] == 'a' &&
      chars[i + 1] == 'l' &&
      chars[i + 2] == 'i' &&
      chars[i + 3] == 'a' &&
      chars[i + 4] == 's'
    if is_alias {
      let mut next = i + 5
      while next < chars.length() && attribute_space(chars[next]) {
        next = next + 1
      }
      if next < chars.length() && chars[next] == '=' {
        buf.write_string("tool_alias")
        i = i + 5
        continue
      }
    }
    buf.write_char(ch)
    i = i + 1
  }
  buf.to_string()
}

///|
fn command_props(attr : @attribute.Attribute) -> Array[@attribute.Prop]? {
  guard attr.raw.has_prefix("#derive.command(") else {
    return derive_props(attr, "command")
  }
  let prefix = "#derive.command"
  let repaired = rewrite_command_alias_label(attr.raw)
  let reparsed = @attribute.Attribute::new(
    loc=attr.loc,
    ("derive", Some("command"), repaired[prefix.length():].to_owned()),
  )
  derive_props(reparsed, "command")
}

///|
fn arg_props(attr : @attribute.Attribute) -> Array[@attribute.Prop]? {
  guard attr.raw.has_prefix("#derive.arg(") else {
    return derive_props(attr, "arg")
  }
  let prefix = "#derive.arg"
  let repaired = rewrite_command_alias_label(attr.raw)
  let reparsed = @attribute.Attribute::new(
    loc=attr.loc,
    ("derive", Some("arg"), repaired[prefix.length():].to_owned()),
  )
  derive_props(reparsed, "arg")
}

///|
fn prop_string(prop : @attribute.Prop) -> (String?, String)? {
  match prop {
    Expr(String(s)) => Some((None, s))
    Labeled(k, String(s)) => Some((Some(k), s))
    _ => None
  }
}

///|
fn prop_bool(prop : @attribute.Prop) -> (String, Bool)? {
  match prop {
    Labeled(k, Bool(v)) => Some((k, v))
    _ => None
  }
}

///|
fn is_tool_identifier(value : String) -> Bool {
  let chars = value.to_array()
  if chars.is_empty() || chars[0] < 'a' || chars[0] > 'z' {
    return false
  }
  let mut after_hyphen = false
  for ch in chars {
    if (ch >= 'a' && ch <= 'z') || (ch >= '0' && ch <= '9') {
      after_hyphen = false
    } else if ch == '-' && !after_hyphen {
      after_hyphen = true
    } else {
      return false
    }
  }
  !after_hyphen
}

///|
fn require_tool_identifier(
  value : String,
  kind : String,
  owner : String,
) -> Unit raise ToolParseError {
  if !is_tool_identifier(value) {
    tool_fail(
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
fn parse_examples(
  attrs : @list.List[@attribute.Attribute],
  owner : String,
) -> Array[ToolExample] raise ToolParseError {
  let result : Array[ToolExample] = []
  for attr in attrs {
    match derive_props(attr, "example") {
      Some(props) => {
        let mut body : String? = None
        let mut title : String? = None
        for p in props {
          match prop_string(p) {
            Some((None, v)) =>
              if body is None {
                body = Some(v)
              } else {
                tool_fail("Duplicate example body on " + owner)
              }
            Some((Some("title"), v)) =>
              if title is None {
                title = Some(v)
              } else {
                tool_fail("Duplicate example title on " + owner)
              }
            _ =>
              tool_fail(
                "Malformed or unknown #derive.example property on " + owner,
              )
          }
        }
        guard body is Some(body) else {
          tool_fail("#derive.example on " + owner + " requires a body")
          continue
        }
        result.push({ body, title })
      }
      None => ()
    }
  }
  result
}

///|
fn[T] set_once(
  slot : Ref[T?],
  value : T,
  label : String,
) -> Unit raise ToolParseError {
  if slot.val is Some(_) {
    tool_fail("Duplicate '" + label + "' property")
  }
  slot.val = Some(value)
}

///|
fn hex_digit_value(ch : Char) -> Int? {
  if ch >= '0' && ch <= '9' {
    Some(ch.to_int() - '0'.to_int())
  } else if ch >= 'a' && ch <= 'f' {
    Some(ch.to_int() - 'a'.to_int() + 10)
  } else if ch >= 'A' && ch <= 'F' {
    Some(ch.to_int() - 'A'.to_int() + 10)
  } else {
    None
  }
}

///|
fn invalid_tool_char(
  property : String,
  owner : String,
) -> Char raise ToolParseError {
  tool_fail("Argument " + property + " on " + owner + " must be one character")
  abort("")
}

///|
fn parse_tool_char(
  value : String,
  property : String,
  owner : String,
) -> Char raise ToolParseError {
  let chars = value.to_array()
  if chars.length() == 1 {
    return chars[0]
  }
  if chars.length() == 2 && chars[0] == '\\' {
    return match chars[1] {
      '\\' => '\\'
      '\'' => '\''
      '"' => '"'
      'n' => '\n'
      'r' => '\r'
      't' => '\t'
      'b' => (8).unsafe_to_char()
      'f' => (12).unsafe_to_char()
      'v' => (11).unsafe_to_char()
      '0' => (0).unsafe_to_char()
      _ => invalid_tool_char(property, owner)
    }
  }
  if chars.length() == 4 && chars[0] == '\\' && chars[1] == 'x' {
    guard hex_digit_value(chars[2]) is Some(high) else {
      return invalid_tool_char(property, owner)
    }
    guard hex_digit_value(chars[3]) is Some(low) else {
      return invalid_tool_char(property, owner)
    }
    return (high * 16 + low).unsafe_to_char()
  }
  if chars.length() >= 5 &&
    chars.length() <= 10 &&
    chars[0] == '\\' &&
    chars[1] == 'u' &&
    chars[2] == '{' &&
    chars[chars.length() - 1] == '}' {
    let mut codepoint = 0
    for ch in chars[3:chars.length() - 1] {
      guard hex_digit_value(ch) is Some(digit) else {
        return invalid_tool_char(property, owner)
      }
      codepoint = codepoint * 16 + digit
    }
    if codepoint <= 0x10ffff && (codepoint < 0xd800 || codepoint > 0xdfff) {
      return codepoint.unsafe_to_char()
    }
  }
  invalid_tool_char(property, owner)
}

///|
fn parse_arg(
  props : Array[@attribute.Prop],
  owner : String,
) -> ToolArgInfo raise ToolParseError {
  let source : Ref[String?] = Ref(None)
  let name : Ref[String?] = Ref(None)
  let scope : Ref[String?] = Ref(None)
  let kind : Ref[String?] = Ref(None)
  let short : Ref[Char?] = Ref(None)
  let env : Ref[String?] = Ref(None)
  let required : Ref[Bool?] = Ref(None)
  let negatable : Ref[Bool?] = Ref(None)
  let optional_scalar : Ref[Bool?] = Ref(None)
  let repeatable : Ref[String?] = Ref(None)
  let delim : Ref[Char?] = Ref(None)
  let separator : Ref[String?] = Ref(None)
  let verbatim : Ref[Bool?] = Ref(None)
  let accepts_stdio : Ref[Bool?] = Ref(None)
  let regex : Ref[String?] = Ref(None)
  let min_length : Ref[String?] = Ref(None)
  let max_length : Ref[String?] = Ref(None)
  let direction : Ref[String?] = Ref(None)
  let min : Ref[String?] = Ref(None)
  let max : Ref[String?] = Ref(None)
  let bounds : Ref[String?] = Ref(None)
  let unit : Ref[String?] = Ref(None)
  let doc : Ref[String?] = Ref(None)
  let value_name : Ref[String?] = Ref(None)
  let aliases : Array[String] = []
  let defaults : Array[String] = []
  let mimes : Array[String] = []
  let schemes : Array[String] = []
  for p in props {
    match prop_string(p) {
      Some((None, v)) => set_once(source, v, "source_param")
      Some((Some(k), v)) =>
        match k {
          "name" => set_once(name, v, k)
          "scope" => set_once(scope, v, k)
          "kind" => set_once(kind, v, k)
          "short" => set_once(short, parse_tool_char(v, k, owner), k)
          "alias" | "tool_alias" => aliases.push(v)
          "env" => set_once(env, v, k)
          "repeatable" => set_once(repeatable, v, k)
          "delim" => set_once(delim, parse_tool_char(v, k, owner), k)
          "default" => defaults.push(v)
          "separator" => set_once(separator, v, k)
          "regex" => set_once(regex, v, k)
          "min_length" => set_once(min_length, v, k)
          "max_length" => set_once(max_length, v, k)
          "direction" => set_once(direction, v, k)
          "mime" => mimes.push(v)
          "scheme" => schemes.push(v)
          "min" => set_once(min, v, k)
          "max" => set_once(max, v, k)
          "bounds" => set_once(bounds, v, k)
          "unit" => set_once(unit, v, k)
          "doc" => set_once(doc, v, k)
          "value_name" => set_once(value_name, v, k)
          _ => tool_fail("Unknown #derive.arg property '" + k + "' on " + owner)
        }
      _ =>
        match prop_bool(p) {
          Some((k, v)) =>
            match k {
              "required" => set_once(required, v, k)
              "negatable" => set_once(negatable, v, k)
              "optional_scalar" => set_once(optional_scalar, v, k)
              "verbatim" => set_once(verbatim, v, k)
              "accepts_stdio" => set_once(accepts_stdio, v, k)
              _ =>
                tool_fail(
                  "Unknown boolean #derive.arg property '" + k + "' on " + owner,
                )
            }
          None => tool_fail("Malformed #derive.arg property on " + owner)
        }
    }
  }
  guard source.val is Some(source_param) else {
    tool_fail("#derive.arg on " + owner + " requires a source parameter")
    abort("")
  }
  if scope.val is Some(s) &&
    !["global", "positional", "option", "flag", "tail"].contains(s) {
    tool_fail("Invalid argument scope '" + s + "'")
  }
  if kind.val is Some(k) &&
    !["flag", "count-flag", "file", "dir", "any"].contains(k) {
    tool_fail("Invalid argument kind '" + k + "'")
  }
  if repeatable.val is Some(r) &&
    !["repeated", "delimited", "either"].contains(r) {
    tool_fail("Invalid repeatable mode '" + r + "'")
  }
  if direction.val is Some(d) && !["input", "output", "inout"].contains(d) {
    tool_fail("Invalid direction '" + d + "'")
  }
  if optional_scalar.val == Some(true) && repeatable.val is Some(_) {
    tool_fail("optional_scalar cannot be combined with repeatable")
  }
  require_tool_identifier(
    name.val.unwrap_or(moonbit_name_to_kebab_case(source_param)),
    "argument",
    owner,
  )
  for alias in aliases {
    require_tool_identifier(alias, "argument alias", owner)
  }
  {
    source_param,
    name: name.val,
    scope: scope.val,
    kind: kind.val,
    short: short.val,
    aliases,
    env: env.val,
    required: required.val,
    negatable: negatable.val,
    optional_scalar: optional_scalar.val,
    repeatable: repeatable.val,
    delim: delim.val,
    defaults,
    separator: separator.val,
    verbatim: verbatim.val,
    accepts_stdio: accepts_stdio.val,
    regex: regex.val,
    min_length: min_length.val,
    max_length: max_length.val,
    direction: direction.val,
    mimes,
    schemes,
    min: min.val,
    max: max.val,
    bounds: bounds.val,
    unit: unit.val,
    doc: doc.val,
    value_name: value_name.val,
  }
}

///|
fn parse_constraint(
  props : Array[@attribute.Prop],
  owner : String,
) -> ToolConstraintInfo raise ToolParseError {
  let mut kind : String? = None
  let refs : Array[String] = []
  let lhs : Array[String] = []
  let rhs : Array[String] = []
  let lhs_quant : Ref[String?] = Ref(None)
  let rhs_quant : Ref[String?] = Ref(None)
  let lhs_value_is : Array[String] = []
  let rhs_value_is : Array[String] = []
  let value_is : Array[String] = []
  for p in props {
    match prop_string(p) {
      Some((None, v)) =>
        if kind is None {
          kind = Some(v)
        } else {
          refs.push(v)
        }
      Some((Some(k), v)) =>
        match k {
          "lhs" => lhs.push(v)
          "rhs" => rhs.push(v)
          "lhs_quant" => set_once(lhs_quant, v, k)
          "rhs_quant" => set_once(rhs_quant, v, k)
          "lhs_value_is" => lhs_value_is.push(v)
          "rhs_value_is" => rhs_value_is.push(v)
          "value_is" => value_is.push(v)
          _ =>
            tool_fail(
              "Unknown #derive.constraint property '" + k + "' on " + owner,
            )
        }
      _ => tool_fail("Malformed #derive.constraint property on " + owner)
    }
  }
  guard kind is Some(kind) else {
    tool_fail("#derive.constraint requires a kind")
    abort("")
  }
  if ![
      "requires_all", "requires_any", "all_or_none", "mutex_groups", "implies", "forbids",
    ].contains(kind) {
    tool_fail("Unknown constraint kind '" + kind + "'")
  }
  let relational = kind == "implies" || kind == "forbids"
  if relational && refs.length() > 0 {
    tool_fail(kind + " uses lhs/rhs properties, not positional references")
  }
  if !relational &&
    (
      lhs.length() > 0 ||
      rhs.length() > 0 ||
      lhs_value_is.length() > 0 ||
      rhs_value_is.length() > 0 ||
      lhs_quant.val is Some(_) ||
      rhs_quant.val is Some(_)
    ) {
    tool_fail(kind + " does not accept lhs/rhs properties")
  }
  for pair in [("lhs_quant", lhs_quant.val), ("rhs_quant", rhs_quant.val)] {
    if pair.1 is Some(value) && value != "all" && value != "any" {
      tool_fail("Unknown constraint quantifier '" + value + "' for " + pair.0)
    }
  }
  if kind == "forbids" && rhs_quant.val is Some(_) {
    tool_fail("forbids does not accept rhs_quant")
  }
  let check_value_is = fn(entry : String) raise ToolParseError {
    match entry.find("=") {
      Some(i) if i > 0 && i + 1 < entry.length() => ()
      _ =>
        tool_fail(
          "Malformed value-is reference '" +
          entry +
          "' on " +
          owner +
          "; expected name=value",
        )
    }
  }
  for entry in value_is {
    check_value_is(entry)
  }
  for entry in lhs_value_is {
    check_value_is(entry)
  }
  for entry in rhs_value_is {
    check_value_is(entry)
  }
  {
    kind,
    refs,
    lhs,
    rhs,
    lhs_quant: lhs_quant.val,
    rhs_quant: rhs_quant.val,
    lhs_value_is,
    rhs_value_is,
    value_is,
  }
}

///|
fn tool_type(ty : @syntax.Type) -> ParamType raise ToolParseError {
  syntax_type_to_param_type(ty) catch {
    AgentParseError(m) => raise ToolParseError(m)
  }
}

///|
fn hidden_or_public(ty : @syntax.Type) -> ToolParamType raise ToolParseError {
  let pt = tool_type(ty)
  match pt {
    Simple("@common.Principal" | "@agentCommon.Principal") => Hidden(Principal)
    Simple("@streams.InputStream") => Hidden(InputStream)
    Simple("@streams.OutputStream") => Hidden(OutputStream)
    Simple(name) if name == "Principal" ||
      name == "InputStream" ||
      name == "OutputStream" =>
      raise ToolParseError(
        "Hidden SDK type '" + name + "' must use its exact qualified name",
      )
    _ => Public(pt)
  }
}

///|
fn labelled(parameter : ToolParamType) -> ToolParamType {
  match parameter {
    Public(type_) => LabelledPublic(type_)
    Hidden(type_) => LabelledHidden(type_)
    LabelledPublic(_) | QuestionOptionalPublic(_) | LabelledHidden(_) =>
      parameter
  }
}

///|
fn question_optional(
  parameter : ToolParamType,
) -> ToolParamType raise ToolParseError {
  match parameter {
    Public(type_) => QuestionOptionalPublic(Optional(type_))
    Hidden(_) | LabelledHidden(_) => {
      tool_fail("Hidden SDK parameters cannot use question-optional syntax")
      abort("")
    }
    LabelledPublic(type_) | QuestionOptionalPublic(type_) =>
      QuestionOptionalPublic(Optional(type_))
  }
}

///|
fn parse_error_cases(
  error_type : ParamType?,
  declarations : Map[String, @syntax.TypeDecl],
) -> Array[ToolErrorCaseInfo] raise ToolParseError {
  guard error_type is Some(Simple(name)) else {
    if error_type is Some(_) {
      tool_fail("Tool command error type must be a named enum or suberror")
    }
    return []
  }
  let bare = match decode_qualified(name) {
    Some((_, id)) => id
    None => name
  }
  guard declarations.get(bare) is Some(decl) else {
    tool_fail(
      "Tool error type '" + name + "' was not declared in the parsed sources",
    )
    return []
  }
  let constrs = match decl.components {
    Variant(cs) => cs
    Error(EnumPayload(cs)) => cs
    _ => {
      tool_fail(
        "Tool error type '" + name + "' must be an enum or variant suberror",
      )
      abort("")
    }
  }
  let result : Array[ToolErrorCaseInfo] = []
  for c in constrs {
    let payload : ParamType? = match c.args {
      None => None
      Some(ps) => {
        let a = ps.to_array()
        if a.length() > 1 {
          tool_fail(
            "Error case '" + c.name.name + "' may have at most one payload",
          )
        }
        if a.length() == 0 {
          None
        } else {
          Some(tool_type(a[0].ty))
        }
      }
    }
    let mut found = false
    let mut kind : String? = None
    let mut exit_code : String? = None
    for attr in c.attrs {
      match derive_props(attr, "error") {
        Some(props) => {
          if found {
            tool_fail("Duplicate #derive.error on case '" + c.name.name + "'")
          }
          found = true
          for p in props {
            match prop_string(p) {
              Some((Some("kind"), v)) =>
                if kind is None {
                  kind = Some(v)
                } else {
                  tool_fail("Duplicate error kind")
                }
              Some((Some("exit_code"), v)) =>
                if exit_code is None {
                  exit_code = Some(v)
                } else {
                  tool_fail("Duplicate exit_code")
                }
              _ =>
                tool_fail(
                  "Malformed or unknown #derive.error property on case '" +
                  c.name.name +
                  "'",
                )
            }
          }
        }
        None => ()
      }
    }
    guard found && kind is Some(kind) else {
      tool_fail(
        "Error case '" + c.name.name + "' requires #derive.error(kind=...)",
      )
      continue
    }
    if !["usage", "usage-error", "runtime", "runtime-error"].contains(kind) {
      tool_fail("Invalid error kind '" + kind + "'")
    }
    if exit_code is Some(code) {
      if code.is_empty() {
        tool_fail("Invalid error exit_code '" + code + "'")
      }
      let mut numeric_code = 0
      for ch in code {
        if ch < '0' || ch > '9' {
          tool_fail("Invalid error exit_code '" + code + "'")
        }
        numeric_code = numeric_code * 10 + ch.to_int() - '0'.to_int()
        if numeric_code > 255 {
          tool_fail("Invalid error exit_code '" + code + "': expected 0..255")
        }
      }
    }
    require_tool_identifier(
      moonbit_name_to_kebab_case(c.name.name),
      "error",
      "error type " + name,
    )
    result.push({
      name: c.name.name,
      payload,
      kind,
      exit_code,
      description: tool_doc(c.doc),
      examples: parse_examples(c.attrs, "error case " + c.name.name),
    })
  }
  result
}

///|
fn parse_command(
  decl : @syntax.FunDecl,
  tool_name : String,
  errors : Map[String, @syntax.TypeDecl],
) -> ToolCommandInfo raise ToolParseError {
  let owner = decl.type_name
    .map(t => {
      match t.name {
        Ident(name=n) => n
        Dot(id=n, ..) => n
      }
    })
    .unwrap_or("") +
    "::" +
    decl.name.name
  if decl.is_async is Some(_) {
    tool_fail(
      "Async tool command '" +
      owner +
      "' is unsupported: the MoonBit WIT export invoker is synchronous and no block_on is available",
    )
  }
  let params : Array[(String, ToolParamType)] = []
  for p in decl.decl_params.unwrap_or(@list.List::from_array([])) {
    match p {
      Positional(binder~, ty=Some(ty)) => {
        if binder.name == "self" {
          tool_fail(
            "Tool command '" + owner + "' must not have a self receiver",
          )
        }
        params.push((binder.name, hidden_or_public(ty)))
      }
      Labelled(binder~, ty=Some(ty)) | Optional(binder~, ty=Some(ty), ..) => {
        if binder.name == "self" {
          tool_fail(
            "Tool command '" + owner + "' must not have a self receiver",
          )
        }
        params.push((binder.name, labelled(hidden_or_public(ty))))
      }
      QuestionOptional(binder~, ty=Some(ty)) => {
        if binder.name == "self" {
          tool_fail(
            "Tool command '" + owner + "' must not have a self receiver",
          )
        }
        params.push((binder.name, question_optional(hidden_or_public(ty))))
      }
      _ =>
        tool_fail(
          "Every parameter of tool command '" +
          owner +
          "' must be named and typed",
        )
    }
  }
  let args : Array[ToolArgInfo] = []
  let constraints : Array[ToolConstraintInfo] = []
  let examples = parse_examples(decl.attrs, owner)
  let aliases : Array[String] = []
  let name_override : Ref[String?] = Ref(None)
  let subtree : Ref[String?] = Ref(None)
  let read_only : Ref[Bool?] = Ref(None)
  let destructive : Ref[Bool?] = Ref(None)
  let idempotent : Ref[Bool?] = Ref(None)
  let open_world : Ref[Bool?] = Ref(None)
  let mut result : ToolResultInfo? = None
  let mut command_seen = false
  for attr in decl.attrs {
    match attr_id(attr) {
      Some((Some("derive"), "arg")) => {
        let props = match arg_props(attr) {
          Some(props) => props
          None => {
            tool_fail("Malformed #derive.arg on " + owner)
            abort("")
          }
        }
        let arg = parse_arg(props, owner)
        if args.any(existing => existing.source_param == arg.source_param) {
          tool_fail(
            "#derive.arg for parameter '" +
            arg.source_param +
            "' is duplicate on " +
            owner,
          )
        }
        args.push(arg)
      }
      Some((Some("derive"), "constraint")) => {
        let props = match derive_props(attr, "constraint") {
          Some(props) => props
          None => {
            tool_fail("Malformed #derive.constraint on " + owner)
            abort("")
          }
        }
        constraints.push(parse_constraint(props, owner))
      }
      Some((Some("derive"), "command")) => {
        if command_seen {
          tool_fail("Duplicate #derive.command on " + owner)
        }
        command_seen = true
        let props = match command_props(attr) {
          Some(props) => props
          None => {
            tool_fail("Malformed #derive.command on " + owner)
            abort("")
          }
        }
        for p in props {
          match prop_string(p) {
            Some((Some("name"), v)) => set_once(name_override, v, "name")
            Some((Some("alias" | "tool_alias"), v)) => aliases.push(v)
            Some((Some("subtree"), v)) => set_once(subtree, v, "subtree")
            _ =>
              match prop_bool(p) {
                Some(("read_only", v)) => set_once(read_only, v, "read_only")
                Some(("destructive", v)) =>
                  set_once(destructive, v, "destructive")
                Some(("idempotent", v)) => set_once(idempotent, v, "idempotent")
                Some(("open_world", v)) => set_once(open_world, v, "open_world")
                _ =>
                  tool_fail(
                    "Malformed or unknown #derive.command property on " + owner,
                  )
              }
          }
        }
      }
      Some((Some("derive"), "result")) => {
        if result is Some(_) {
          tool_fail("Duplicate #derive.result on " + owner)
        }
        let formatters : Array[String] = []
        let options : Array[(String, String)] = []
        let defaults : Array[String] = []
        let props = match derive_props(attr, "result") {
          Some(props) => props
          None => {
            tool_fail("Malformed #derive.result on " + owner)
            abort("")
          }
        }
        for p in props {
          match prop_string(p) {
            Some((None, v)) | Some((Some("formatter"), v)) => formatters.push(v)
            Some((Some("default"), v)) => defaults.push(v)
            Some((Some(k), v)) => options.push((k, v))
            _ => tool_fail("Malformed #derive.result property on " + owner)
          }
        }
        guard formatters.length() > 0 else {
          tool_fail("#derive.result requires a formatter")
          continue
        }
        if defaults.length() > 1 {
          tool_fail("#derive.result accepts at most one default formatter")
        }
        if defaults is [default] && !formatters.contains(default) {
          tool_fail(
            "Default result formatter '" +
            default +
            "' is not one of the declared formatters",
          )
        }
        if options.length() > 0 {
          tool_fail("Unknown #derive.result property '" + options[0].0 + "'")
        }
        for formatter in formatters {
          require_tool_identifier(formatter, "result formatter", owner)
        }
        result = Some({ formatters, options, defaults })
      }
      Some((Some("derive"), "example")) | None => ()
      Some((Some("derive"), n)) if n == "prompt_hint" ||
        n == "endpoint" ||
        n == "read_only" => ()
      _ => ()
    }
  }
  for arg in args {
    if !params.any(p => p.0 == arg.source_param) {
      tool_fail(
        "#derive.arg references missing parameter '" +
        arg.source_param +
        "' on " +
        owner,
      )
    }
  }
  let mut input_streams = 0
  let mut output_streams = 0
  for p in params {
    match p.1 {
      Hidden(InputStream) | LabelledHidden(InputStream) => input_streams += 1
      Hidden(OutputStream) | LabelledHidden(OutputStream) => output_streams += 1
      _ => ()
    }
  }
  if input_streams > 1 || output_streams > 1 {
    tool_fail("Tool command '" + owner + "' has duplicate stream parameters")
  }
  let mapped = moonbit_name_to_kebab_case(decl.name.name)
  let is_natural_root = mapped == tool_name
  if is_natural_root && name_override.val is Some(n) && n != tool_name {
    tool_fail("Root command '" + owner + "' cannot override its wire name")
  }
  if !is_natural_root && name_override.val == Some(tool_name) {
    tool_fail(
      "Command '" + owner + "' cannot override itself into the tool root",
    )
  }
  let name = name_override.val.unwrap_or(mapped)
  require_tool_identifier(name, "command", owner)
  for alias in aliases {
    require_tool_identifier(alias, "command alias", owner)
  }
  let mut error_via_result = false
  let (success_type, error_type) : (ParamType?, ParamType?) = match
    decl.return_type {
    None =>
      (
        None,
        match decl.error_type {
          ErrorType(ty~) => Some(tool_type(ty))
          DefaultErrorType(..) | MaybeError(..) => {
            tool_fail("Tool command '" + owner + "' must use a typed raise")
            None
          }
          _ => None
        },
      )
    Some(ty) =>
      match tool_type(ty) {
        ResultType(ok, err) => {
          error_via_result = true
          if decl.error_type is ErrorType(..) {
            tool_fail(
              "Tool command '" +
              owner +
              "' cannot combine Result[T,E] with typed raise",
            )
          }
          (Some(ok), Some(err))
        }
        other =>
          (
            Some(other),
            match decl.error_type {
              ErrorType(ty~) => Some(tool_type(ty))
              DefaultErrorType(..) | MaybeError(..) => {
                tool_fail("Tool command '" + owner + "' must use a typed raise")
                None
              }
              _ => None
            },
          )
      }
  }
  {
    source_name: decl.name.name,
    name,
    aliases,
    subtree: subtree.val,
    read_only: read_only.val,
    destructive: destructive.val,
    idempotent: idempotent.val,
    open_world: open_world.val,
    params,
    args,
    success_type,
    error_type,
    error_via_result,
    errors: parse_error_cases(error_type, errors),
    result,
    constraints,
    examples,
    description: tool_doc(decl.doc),
    is_root: name == tool_name,
  }
}

///|
pub fn parse_tools(
  files : Array[(String, String)],
) -> Array[ToolInfo] raise ToolParseError {
  let impls : Array[@syntax.Impl] = []
  let errors : Map[String, @syntax.TypeDecl] = {}
  for pair in files {
    let (filename, source) = pair
    let (parsed, reports) = @parser.parse_string(source)
    if reports.length() > 0 {
      let messages : Array[String] = []
      for report in reports {
        messages.push(report.to_string())
      }
      tool_fail("Failed to parse " + filename + ": " + messages.join("; "))
    }
    for item in parsed {
      impls.push(item)
      match item {
        TopTypeDef(d) => errors[d.tycon] = d
        _ => ()
      }
    }
  }
  let result : Array[ToolInfo] = []
  let by_source : Map[String, Int] = {}
  for item in impls {
    match item {
      TopTypeDef(decl) =>
        for attr in decl.attrs {
          match derive_props(attr, "tool") {
            Some(props) => {
              if by_source.contains(decl.tycon) {
                tool_fail(
                  "Duplicate #derive.tool declaration for '" + decl.tycon + "'",
                )
              }
              match decl.components {
                Record(fields~, ..) if fields.is_empty() => ()
                _ =>
                  tool_fail(
                    "#derive.tool struct '" + decl.tycon + "' must be empty",
                  )
              }
              let mut name : String? = None
              let mut version : String? = None
              for p in props {
                match prop_string(p) {
                  Some((None, v)) =>
                    if name is None {
                      name = Some(v)
                    } else {
                      tool_fail("Duplicate tool name")
                    }
                  Some((Some("version"), v)) =>
                    if version is None {
                      version = Some(v)
                    } else {
                      tool_fail("Duplicate tool version")
                    }
                  _ =>
                    tool_fail(
                      "Malformed or unknown #derive.tool property on '" +
                      decl.tycon +
                      "'",
                    )
                }
              }
              let wire = name.unwrap_or(moonbit_name_to_kebab_case(decl.tycon))
              require_tool_identifier(wire, "tool", "tool " + decl.tycon)
              by_source[decl.tycon] = result.length()
              result.push({
                source_name: decl.tycon,
                name: wire,
                version,
                description: tool_doc(decl.doc),
                examples: parse_examples(decl.attrs, "tool " + decl.tycon),
                commands: [],
                root_command: None,
              })
            }
            None => ()
          }
        }
      _ => ()
    }
  }
  for item in impls {
    match item {
      TopFuncDef(fun_decl~, ..) =>
        match fun_decl.type_name {
          Some({ name: Ident(name=owner), .. }) =>
            match by_source.get(owner) {
              Some(idx) if fun_decl.vis is Pub(..) => {
                if fun_decl.name.name == "new" || fun_decl.name.name == "drop" {
                  tool_fail(
                    "Tool command '" +
                    owner +
                    "::" +
                    fun_decl.name.name +
                    "' is reserved for generated typed client lifecycle methods",
                  )
                }
                let command = parse_command(fun_decl, result[idx].name, errors)
                if command.is_root {
                  if result[idx].root_command is Some(_) {
                    tool_fail(
                      "Tool '" +
                      result[idx].name +
                      "' has more than one root command",
                    )
                  }
                  result[idx].root_command = Some(result[idx].commands.length())
                }
                result[idx].commands.push(command)
              }
              _ => ()
            }
          _ => ()
        }
      _ => ()
    }
  }
  // Resolve subtree references after every local tool has been collected. A
  // qualified spelling denotes its final type segment; local type names are
  // unique by construction.
  for tool in result {
    for command in tool.commands {
      if command.subtree is Some(target) {
        let mut bare = target
        for part in target.split(".") {
          bare = part.to_owned()
        }
        let matches = result.filter(candidate => candidate.source_name == bare)
        if matches.length() == 0 {
          tool_fail(
            "Subtree target '" +
            target +
            "' on " +
            tool.source_name +
            "::" +
            command.source_name +
            " was not found",
          )
        }
        if matches.length() > 1 {
          tool_fail("Subtree target '" + target + "' is ambiguous")
        }
        let non_unit_result = match command.success_type {
          None | Some(Simple("Unit")) => false
          _ => true
        }
        if command.constraints.length() > 0 ||
          command.result is Some(_) ||
          command.error_type is Some(_) ||
          command.read_only is Some(_) ||
          command.destructive is Some(_) ||
          command.idempotent is Some(_) ||
          command.open_world is Some(_) ||
          non_unit_result {
          tool_fail(
            "Subtree command '" +
            tool.source_name +
            "::" +
            command.source_name +
            "' cannot have body-only metadata or a non-Unit result",
          )
        }
        for pair in command.params {
          match pair.1 {
            Public(_) | LabelledPublic(_) | QuestionOptionalPublic(_) => {
              let info = command.args
                .iter()
                .find_first(arg => arg.source_param == pair.0)
              if info.flat_map(arg => arg.scope) is Some("positional" | "tail") {
                tool_fail(
                  "Subtree parameter '" +
                  pair.0 +
                  "' must project to a global option or flag",
                )
              }
            }
            Hidden(_) | LabelledHidden(_) =>
              tool_fail(
                "Subtree methods cannot declare hidden stream or principal parameters",
              )
          }
        }
        if command.is_root {
          tool_fail(
            "The implicit root command cannot also be a subtree command",
          )
        }
      }
    }
  }
  result
}
