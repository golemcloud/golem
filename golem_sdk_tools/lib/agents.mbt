///|
pub(all) enum ParamType {
  Simple(String)
  Optional(ParamType)
  List(ParamType)
  ResultType(ParamType, ParamType)
  Tuple(Array[ParamType])
  MultimodalType(ParamType)
  MultimodalCustomType(ParamType)
} derive(Show, Eq)

///|
/// Encodes a qualified (cross-package) type name into the `Simple` string format.
pub fn encode_qualified(pkg : String, id : String) -> ParamType {
  Simple("@\{pkg}.\{id}")
}

///|
/// Tries to decode a `Simple` name as a qualified type.
/// Returns `Some((pkg, id))` for names like `"@pkg.Type"`, `None` otherwise.
pub fn decode_qualified(name : String) -> (String, String)? {
  if name.has_prefix("@") {
    match name.find(".") {
      Some(dot) => {
        let pkg = name.unsafe_substring(start=1, end=dot)
        let id = name.unsafe_substring(start=dot + 1, end=name.length())
        if pkg.length() > 0 && id.length() > 0 {
          Some((pkg, id))
        } else {
          None
        }
      }
      None => None
    }
  } else {
    None
  }
}

///|
pub(all) struct SchemaOptions {
  text_languages : Array[String]
  binary_mime_types : Array[String]
} derive(Show, Eq)

///|
pub(all) struct MethodInfo {
  name : String
  description : String
  prompt_hint : String?
  params : Array[(String, ParamType)]
  return_type : ParamType?
  param_schema_options : Map[String, SchemaOptions]
} derive(Show)

///|
pub(all) enum AgentMode {
  Durable
  Ephemeral
} derive(Show, Eq)

///|
pub(all) struct AgentInfo {
  name : String
  description : String
  mode : AgentMode
  constructor_params : Array[(String, ParamType)]
  constructor_description : String
  constructor_prompt_hint : String?
  methods : Array[MethodInfo]
  has_init : Bool
  constructor_schema_options : Map[String, SchemaOptions]
} derive(Show)

///|
pub suberror AgentParseError {
  AgentParseError(String)
} derive(Show)

///|
pub fn syntax_type_to_param_type(
  ty : @syntax.Type,
) -> ParamType raise AgentParseError {
  match ty {
    Option(ty~, ..) => Optional(syntax_type_to_param_type(ty))
    Tuple(tys~, ..) => {
      let elems : Array[ParamType] = []
      tys.each(fn(t) {
        let pt = syntax_type_to_param_type(t) catch { _ => Simple("Unknown") }
        elems.push(pt)
      })
      Tuple(elems)
    }
    Name(constr_id~, tys~, ..) =>
      match constr_id.id {
        Ident(name~) =>
          if name == "Array" {
            match tys {
              More(inner, ..) => List(syntax_type_to_param_type(inner))
              Empty =>
                raise AgentParseError("Array type requires a type parameter")
            }
          } else if name == "Result" {
            match tys {
              More(ok, tail~) =>
                match tail {
                  More(err, ..) =>
                    ResultType(
                      syntax_type_to_param_type(ok),
                      syntax_type_to_param_type(err),
                    )
                  Empty =>
                    raise AgentParseError(
                      "Result type requires two type parameters",
                    )
                }
              Empty =>
                raise AgentParseError(
                  "Result type requires two type parameters",
                )
            }
          } else if name == "Multimodal" {
            match tys {
              More(inner, ..) =>
                MultimodalType(syntax_type_to_param_type(inner))
              Empty =>
                raise AgentParseError(
                  "Multimodal type requires a type parameter",
                )
            }
          } else if name == "MultimodalCustom" {
            match tys {
              More(inner, ..) =>
                MultimodalCustomType(syntax_type_to_param_type(inner))
              Empty =>
                raise AgentParseError(
                  "MultimodalCustom type requires a type parameter",
                )
            }
          } else if tys.is_empty() {
            Simple(name)
          } else {
            raise AgentParseError("Unsupported parameterized type: \{name}")
          }
        Dot(pkg~, id~) =>
          if id == "Multimodal" && not(tys.is_empty()) {
            match tys {
              More(inner, ..) =>
                MultimodalType(syntax_type_to_param_type(inner))
              Empty =>
                raise AgentParseError(
                  "Multimodal type requires a type parameter",
                )
            }
          } else if id == "MultimodalCustom" && not(tys.is_empty()) {
            match tys {
              More(inner, ..) =>
                MultimodalCustomType(syntax_type_to_param_type(inner))
              Empty =>
                raise AgentParseError(
                  "MultimodalCustom type requires a type parameter",
                )
            }
          } else if tys.is_empty() {
            encode_qualified(pkg, id)
          } else {
            raise AgentParseError(
              "Unsupported qualified parameterized type: @\{pkg}.\{id}",
            )
          }
      }
    _ => raise AgentParseError("Unsupported type syntax")
  }
}

///|
fn extract_doc_string(doc : @syntax.DocString) -> String {
  let sb = StringBuilder::new()
  let mut first = true
  doc.content.each(fn(line) {
    if first {
      first = false
    } else {
      sb.write_char('\n')
    }
    sb.write_string(line)
  })
  sb.to_string().trim().to_string()
}

///|
fn extract_agent_mode(
  attrs : @list.List[@attribute.Attribute],
) -> AgentMode raise AgentParseError {
  let mut result = Durable
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "agent" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) =>
              if s == "ephemeral" {
                result = Ephemeral
              } else if s == "durable" {
                result = Durable
              }
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
fn extract_prompt_hint(attrs : @list.List[@attribute.Attribute]) -> String? {
  let mut result : String? = None
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "prompt_hint" }, props)) =>
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => result = Some(s)
            _ => ()
          }
        })
      _ => ()
    }
  })
  result
}

///|
let unstructured_type_names : Array[String] = [
  "UnstructuredText", "UnstructuredBinary",
]

///|
fn is_unstructured_type_name(name : String) -> Bool {
  for uname in unstructured_type_names {
    if name == uname || name.has_suffix(".\{uname}") {
      return true
    }
  }
  false
}

///|
pub fn is_multimodal_type(pt : ParamType) -> Bool {
  match pt {
    MultimodalType(_) | MultimodalCustomType(_) => true
    _ => false
  }
}

///|
pub fn validate_no_nested_unstructured(
  param_name : String,
  pt : ParamType,
) -> Unit raise AgentParseError {
  match pt {
    Simple(name) => {
      let _ = name
    }
    MultimodalType(_) | MultimodalCustomType(_) => ()
    Optional(inner) => {
      validate_no_unstructured_inside("Option", param_name, inner)
      validate_no_multimodal_inside("Option", param_name, inner)
    }
    List(inner) => {
      validate_no_unstructured_inside("Array", param_name, inner)
      validate_no_multimodal_inside("Array", param_name, inner)
    }
    ResultType(ok, err) => {
      validate_no_unstructured_inside("Result", param_name, ok)
      validate_no_unstructured_inside("Result", param_name, err)
      validate_no_multimodal_inside("Result", param_name, ok)
      validate_no_multimodal_inside("Result", param_name, err)
    }
    Tuple(elems) =>
      for elem in elems {
        validate_no_unstructured_inside("Tuple", param_name, elem)
        validate_no_multimodal_inside("Tuple", param_name, elem)
      }
  }
}

///|
fn validate_no_unstructured_inside(
  wrapper : String,
  param_name : String,
  pt : ParamType,
) -> Unit raise AgentParseError {
  match pt {
    Simple(name) =>
      if is_unstructured_type_name(name) {
        raise AgentParseError(
          "\{name} cannot be nested inside \{wrapper} for parameter '\{param_name}'; it must be a direct parameter or return type",
        )
      }
    MultimodalType(_) | MultimodalCustomType(_) => ()
    Optional(inner) =>
      validate_no_unstructured_inside(wrapper, param_name, inner)
    List(inner) => validate_no_unstructured_inside(wrapper, param_name, inner)
    ResultType(ok, err) => {
      validate_no_unstructured_inside(wrapper, param_name, ok)
      validate_no_unstructured_inside(wrapper, param_name, err)
    }
    Tuple(elems) =>
      for elem in elems {
        validate_no_unstructured_inside(wrapper, param_name, elem)
      }
  }
}

///|
fn validate_no_multimodal_inside(
  wrapper : String,
  param_name : String,
  pt : ParamType,
) -> Unit raise AgentParseError {
  match pt {
    MultimodalType(_) | MultimodalCustomType(_) =>
      raise AgentParseError(
        "Multimodal cannot be nested inside \{wrapper} for parameter '\{param_name}'; it must be a direct parameter or return type",
      )
    Simple(_) => ()
    Optional(inner) => validate_no_multimodal_inside(wrapper, param_name, inner)
    List(inner) => validate_no_multimodal_inside(wrapper, param_name, inner)
    ResultType(ok, err) => {
      validate_no_multimodal_inside(wrapper, param_name, ok)
      validate_no_multimodal_inside(wrapper, param_name, err)
    }
    Tuple(elems) =>
      for elem in elems {
        validate_no_multimodal_inside(wrapper, param_name, elem)
      }
  }
}

///|
fn extract_schema_restrictions(
  attrs : @list.List[@attribute.Attribute],
) -> Map[String, SchemaOptions] {
  let result : Map[String, SchemaOptions] = {}
  attrs.each(fn(attr) {
    match attr.parsed {
      Some(Apply({ qual: Some("derive"), name: "text_languages" }, props)) => {
        let strings : Array[String] = []
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => strings.push(s)
            _ => ()
          }
        })
        if strings.length() >= 2 {
          let param_name = strings[0]
          let codes : Array[String] = []
          for i in 1..<strings.length() {
            codes.push(strings[i])
          }
          match result.get(param_name) {
            Some(existing) =>
              result[param_name] = {
                text_languages: existing.text_languages + codes,
                binary_mime_types: existing.binary_mime_types,
              }
            None =>
              result[param_name] = {
                text_languages: codes,
                binary_mime_types: [],
              }
          }
        }
      }
      Some(Apply({ qual: Some("derive"), name: "mime_types" }, props)) => {
        let strings : Array[String] = []
        props.each(fn(prop) {
          match prop {
            Expr(String(s)) => strings.push(s)
            _ => ()
          }
        })
        if strings.length() >= 2 {
          let param_name = strings[0]
          let mimes : Array[String] = []
          for i in 1..<strings.length() {
            mimes.push(strings[i])
          }
          match result.get(param_name) {
            Some(existing) =>
              result[param_name] = {
                text_languages: existing.text_languages,
                binary_mime_types: existing.binary_mime_types + mimes,
              }
            None =>
              result[param_name] = {
                text_languages: [],
                binary_mime_types: mimes,
              }
          }
        }
      }
      _ => ()
    }
  })
  result
}

///|
fn extract_method_params(
  fun_decl : @syntax.FunDecl,
) -> Array[(String, ParamType)] raise AgentParseError {
  let params : Array[(String, ParamType)] = []
  match fun_decl.decl_params {
    Some(ps) =>
      ps.each(fn(p) {
        match p {
          Positional(binder~, ty=Some(ty)) | Labelled(binder~, ty=Some(ty)) => {
            let param_name = binder.name
            let param_type = syntax_type_to_param_type(ty) catch {
              _ => Simple("Unknown")
            }
            params.push((param_name, param_type))
          }
          _ => ()
        }
      })
    None => ()
  }
  params
}

///|
fn extract_return_type(
  fun_decl : @syntax.FunDecl,
) -> ParamType? raise AgentParseError {
  match fun_decl.return_type {
    Some(ty) => {
      let pt = syntax_type_to_param_type(ty)
      if pt == Simple("Unit") {
        None
      } else {
        Some(pt)
      }
    }
    None => None
  }
}

///|
pub fn parse_agents(
  files : Array[(String, String)],
) -> Array[AgentInfo] raise AgentParseError {
  let agent_names : Map[String, String] = {}
  let agent_modes : Map[String, AgentMode] = {}
  let agent_constructors : Map[
    String,
    (Array[(String, ParamType)], String, String?, Map[String, SchemaOptions]),
  ] = {}
  let agent_methods : Map[String, Array[MethodInfo]] = {}
  let mut has_init = false
  for pair in files {
    let (_filename, content) = pair
    let (impls, _reports) = @parser.parse_string(content)
    // First pass: find #derive.agent annotated structs
    impls.each(fn(impl_) {
      match impl_ {
        TopTypeDef(type_decl) => {
          let mut is_agent = false
          type_decl.attrs.each(fn(attr) {
            if attr.raw == "#derive.agent" ||
              attr.raw.has_prefix("#derive.agent(") {
              is_agent = true
            }
          })
          if is_agent {
            let name = type_decl.tycon
            let description = extract_doc_string(type_decl.doc)
            let desc = if description == "" { name } else { description }
            let mode = extract_agent_mode(type_decl.attrs) catch {
              _ => Durable
            }
            agent_names[name] = desc
            agent_modes[name] = mode
            agent_methods[name] = []
          }
        }
        _ => ()
      }
    })
    // Second pass: find ::new constructors, public methods, and fn init
    impls.each(fn(impl_) {
      match impl_ {
        TopFuncDef(fun_decl~, ..) => {
          // Check for fn init
          if fun_decl.type_name is None && fun_decl.name.name == "init" {
            has_init = true
          }
          match fun_decl.type_name {
            Some({ name: Ident(name~), .. }) =>
              if agent_names.contains(name) {
                if fun_decl.name.name == "new" {
                  // Constructor
                  let params = extract_method_params(fun_decl) catch { _ => [] }
                  let doc = extract_doc_string(fun_decl.doc)
                  let desc = if doc == "" {
                    "Creates a new \{name}"
                  } else {
                    doc
                  }
                  let hint = extract_prompt_hint(fun_decl.attrs)
                  let restrictions = extract_schema_restrictions(fun_decl.attrs)
                  agent_constructors[name] = (params, desc, hint, restrictions)
                } else if fun_decl.vis is Pub(..) {
                  // Public method — check if first param is Self
                  let mut is_method = false
                  match fun_decl.decl_params {
                    Some(ps) => {
                      let mut first = true
                      ps.each(fn(p) {
                        if first {
                          first = false
                          match p {
                            Positional(ty=Some(Name(constr_id~, ..)), ..) =>
                              match constr_id.id {
                                Ident(name~) =>
                                  if name == "Self" {
                                    is_method = true
                                  }
                                _ => ()
                              }
                            _ => ()
                          }
                        }
                      })
                    }
                    None => ()
                  }
                  if is_method {
                    // Extract params (skip self)
                    let all_params = extract_method_params(fun_decl) catch {
                      _ => []
                    }
                    // Remove the first param (self : Self)
                    let params : Array[(String, ParamType)] = []
                    for i in 1..<all_params.length() {
                      params.push(all_params[i])
                    }
                    let return_type = extract_return_type(fun_decl) catch {
                      _ => None
                    }
                    let doc = extract_doc_string(fun_decl.doc)
                    let method_name = fun_decl.name.name
                    let desc = if doc == "" { method_name } else { doc }
                    let hint = extract_prompt_hint(fun_decl.attrs)
                    let restrictions = extract_schema_restrictions(
                      fun_decl.attrs,
                    )
                    match agent_methods.get(name) {
                      Some(methods) =>
                        methods.push({
                          name: method_name,
                          description: desc,
                          prompt_hint: hint,
                          params,
                          return_type,
                          param_schema_options: restrictions,
                        })
                      None => ()
                    }
                  }
                }
              }
            _ => ()
          }
        }
        _ => ()
      }
    })
  }
  // Validate
  let agents : Array[AgentInfo] = []
  for name, description in agent_names {
    match agent_constructors.get(name) {
      Some((params, ctor_desc, ctor_hint, ctor_restrictions)) =>
        agents.push({
          name,
          description,
          mode: agent_modes.get(name).unwrap_or(Durable),
          constructor_params: params,
          constructor_description: ctor_desc,
          constructor_prompt_hint: ctor_hint,
          methods: agent_methods.get(name).unwrap_or([]),
          has_init,
          constructor_schema_options: ctor_restrictions,
        })
      None => raise AgentParseError("Agent '\{name}' has no ::new constructor")
    }
  }
  // Validate nesting and restriction targets
  for agent in agents {
    // Validate constructor params
    for param in agent.constructor_params {
      let (pname, pt) = param
      validate_no_nested_unstructured(pname, pt)
    }
    // Validate multimodal is sole constructor param
    let has_multimodal_ctor = agent.constructor_params
      .iter()
      .find_first(fn(p) { is_multimodal_type(p.1) })
    if has_multimodal_ctor is Some(_) && agent.constructor_params.length() != 1 {
      raise AgentParseError(
        "Multimodal parameter in constructor of agent '\{agent.name}' must be the only parameter",
      )
    }
    for rname, opts in agent.constructor_schema_options {
      let param_type = agent.constructor_params
        .iter()
        .find_first(fn(p) { p.0 == rname })
      match param_type {
        None =>
          raise AgentParseError(
            "#derive.text_languages/#derive.mime_types references unknown constructor parameter '\{rname}'",
          )
        Some((_, pt)) => {
          if opts.text_languages.length() > 0 {
            match pt {
              Simple(name) =>
                if not(
                    name == "UnstructuredText" ||
                    name.has_suffix(".UnstructuredText"),
                  ) {
                  raise AgentParseError(
                    "#derive.text_languages on parameter '\{rname}' of constructor requires the parameter type to be UnstructuredText",
                  )
                }
              _ =>
                raise AgentParseError(
                  "#derive.text_languages on parameter '\{rname}' of constructor requires the parameter type to be UnstructuredText",
                )
            }
          }
          if opts.binary_mime_types.length() > 0 {
            match pt {
              Simple(name) =>
                if not(
                    name == "UnstructuredBinary" ||
                    name.has_suffix(".UnstructuredBinary"),
                  ) {
                  raise AgentParseError(
                    "#derive.mime_types on parameter '\{rname}' of constructor requires the parameter type to be UnstructuredBinary",
                  )
                }
              _ =>
                raise AgentParseError(
                  "#derive.mime_types on parameter '\{rname}' of constructor requires the parameter type to be UnstructuredBinary",
                )
            }
          }
        }
      }
    }
    // Validate methods
    for meth in agent.methods {
      for param in meth.params {
        let (pname, pt) = param
        validate_no_nested_unstructured(pname, pt)
      }
      match meth.return_type {
        Some(rt) => validate_no_nested_unstructured("return", rt)
        None => ()
      }
      // Validate multimodal params
      let has_multimodal_param = meth.params
        .iter()
        .find_first(fn(p) { is_multimodal_type(p.1) })
      if has_multimodal_param is Some(_) && meth.params.length() != 1 {
        raise AgentParseError(
          "Multimodal parameter in method '\{meth.name}' must be the only parameter",
        )
      }
      for rname, opts in meth.param_schema_options {
        let param_type = meth.params.iter().find_first(fn(p) { p.0 == rname })
        match param_type {
          None =>
            raise AgentParseError(
              "#derive.text_languages/#derive.mime_types references unknown parameter '\{rname}' of method '\{meth.name}'",
            )
          Some((_, pt)) => {
            if opts.text_languages.length() > 0 {
              match pt {
                Simple(name) =>
                  if not(
                      name == "UnstructuredText" ||
                      name.has_suffix(".UnstructuredText"),
                    ) {
                    raise AgentParseError(
                      "#derive.text_languages on parameter '\{rname}' of method '\{meth.name}' requires the parameter type to be UnstructuredText",
                    )
                  }
                _ =>
                  raise AgentParseError(
                    "#derive.text_languages on parameter '\{rname}' of method '\{meth.name}' requires the parameter type to be UnstructuredText",
                  )
              }
            }
            if opts.binary_mime_types.length() > 0 {
              match pt {
                Simple(name) =>
                  if not(
                      name == "UnstructuredBinary" ||
                      name.has_suffix(".UnstructuredBinary"),
                    ) {
                    raise AgentParseError(
                      "#derive.mime_types on parameter '\{rname}' of method '\{meth.name}' requires the parameter type to be UnstructuredBinary",
                    )
                  }
                _ =>
                  raise AgentParseError(
                    "#derive.mime_types on parameter '\{rname}' of method '\{meth.name}' requires the parameter type to be UnstructuredBinary",
                  )
              }
            }
          }
        }
      }
    }
  }
  agents
}
