///|
pub(all) enum ParamType {
  Simple(String)
  Optional(ParamType)
  List(ParamType)
  ResultType(ParamType, ParamType)
} derive(Show, Eq)

///|
pub(all) struct AgentInfo {
  name : String
  description : String
  constructor_params : Array[(String, ParamType)]
  constructor_description : String
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
pub fn parse_agents(
  files : Array[(String, String)],
) -> Array[AgentInfo] raise AgentParseError {
  let agent_names : Map[String, String] = {}
  let agent_constructors : Map[String, (Array[(String, ParamType)], String)] = {}
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
            if attr.raw == "#derive.agent" {
              is_agent = true
            }
          })
          if is_agent {
            let name = type_decl.tycon
            let description = extract_doc_string(type_decl.doc)
            let desc = if description == "" { name } else { description }
            agent_names[name] = desc
          }
        }
        _ => ()
      }
    })
    // Second pass: find ::new constructors and fn init
    impls.each(fn(impl_) {
      match impl_ {
        TopFuncDef(fun_decl~, ..) => {
          // Check for fn init
          if fun_decl.type_name is None && fun_decl.name.name == "init" {
            has_init = true
          }
          // Check for Agent::new
          match fun_decl.type_name {
            Some({ name: Ident(name~), .. }) =>
              if agent_names.contains(name) && fun_decl.name.name == "new" {
                let params : Array[(String, ParamType)] = []
                match fun_decl.decl_params {
                  Some(ps) =>
                    ps.each(fn(p) {
                      match p {
                        Positional(binder~, ty=Some(ty))
                        | Labelled(binder~, ty=Some(ty)) => {
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
                let doc = extract_doc_string(fun_decl.doc)
                let desc = if doc == "" { "Creates a new \{name}" } else { doc }
                agent_constructors[name] = (params, desc)
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
      Some((params, ctor_desc)) =>
        agents.push({
          name,
          description,
          constructor_params: params,
          constructor_description: ctor_desc,
          has_init,
        })
      None => raise AgentParseError("Agent '\{name}' has no ::new constructor")
    }
  }
  agents
}
