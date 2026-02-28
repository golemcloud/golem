///|
pub(all) enum ParamType {
  Simple(String)
  Optional(ParamType)
  List(ParamType)
  ResultType(ParamType, ParamType)
} derive(Show, Eq)

///|
pub(all) struct MethodInfo {
  name : String
  description : String
  prompt_hint : String?
  params : Array[(String, ParamType)]
  return_type : ParamType?
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
} derive(Show)

///|
pub suberror AgentParseError {
  AgentParseError(String)
}

///|
pub fn syntax_type_to_param_type(
  ty : @syntax.Type,
) -> ParamType raise AgentParseError {
  match ty {
    Option(ty~, ..) => Optional(syntax_type_to_param_type(ty))
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
          } else if tys.is_empty() {
            Simple(name)
          } else {
            raise AgentParseError("Unsupported parameterized type: \{name}")
          }
        Dot(pkg~, id~) =>
          if tys.is_empty() {
            Simple("@\{pkg}.\{id}")
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
    (Array[(String, ParamType)], String, String?),
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
                  agent_constructors[name] = (params, desc, hint)
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
                    match agent_methods.get(name) {
                      Some(methods) =>
                        methods.push({
                          name: method_name,
                          description: desc,
                          prompt_hint: hint,
                          params,
                          return_type,
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
      Some((params, ctor_desc, ctor_hint)) =>
        agents.push({
          name,
          description,
          mode: agent_modes.get(name).unwrap_or(Durable),
          constructor_params: params,
          constructor_description: ctor_desc,
          constructor_prompt_hint: ctor_hint,
          methods: agent_methods.get(name).unwrap_or([]),
          has_init,
        })
      None => raise AgentParseError("Agent '\{name}' has no ::new constructor")
    }
  }
  agents
}
