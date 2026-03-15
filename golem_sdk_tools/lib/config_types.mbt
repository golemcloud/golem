///|
/// Parsed information about a config struct annotated with `#derive.config`.
pub(all) struct ConfigTypeInfo {
  name : String
  fields : Array[(String, ParamType)]
} derive(Show, Eq)

///|
/// Parses `#derive.config` annotated struct types from source files.
pub fn parse_config_types(
  files : Array[(String, String)],
) -> Array[ConfigTypeInfo] raise AgentParseError {
  let results : Array[ConfigTypeInfo] = []
  for pair in files {
    let (_filename, content) = pair
    let (impls, _reports) = @parser.parse_string(content)
    let impl_arr = impls.to_array()
    for impl_ in impl_arr {
      match impl_ {
        TopTypeDef(type_decl) => {
          let mut is_config = false
          let attr_arr = type_decl.attrs.to_array()
          for attr in attr_arr {
            if attr.raw == "#derive.config" {
              is_config = true
            }
          }
          if is_config {
            let name = type_decl.tycon
            if not(type_decl.params.is_empty()) {
              raise AgentParseError(
                "#derive.config does not support generic types, but '\{name}' has type parameters",
              )
            }
            match type_decl.components {
              Record(fields~, ..) => {
                let result : Array[(String, ParamType)] = []
                let field_arr = fields.to_array()
                for field in field_arr {
                  let field_name = field.name.label
                  let field_type = syntax_type_to_param_type(field.ty)
                  result.push((field_name, field_type))
                }
                results.push({ name, fields: result })
              }
              _ =>
                raise AgentParseError(
                  "#derive.config can only be applied to struct types, but '\{name}' is not a struct",
                )
            }
          }
        }
        _ => ()
      }
    }
  }
  results
}

///|
/// Validates that config types referenced by agents exist and are valid.
pub fn validate_config_types(
  agents : Array[AgentInfo],
  config_types : Array[ConfigTypeInfo],
) -> Unit raise AgentParseError {
  let config_type_names : Map[String, ConfigTypeInfo] = {}
  for ct in config_types {
    config_type_names[ct.name] = ct
  }
  for agent in agents {
    match agent.config_type {
      Some(type_name) =>
        if config_type_names.get(type_name) is None {
          raise AgentParseError(
            "Agent '\{agent.name}' references config type '\{type_name}' which is not annotated with #derive.config",
          )
        }
      None => ()
    }
  }
  for ct in config_types {
    validate_no_config_cycles(ct.name, ct, config_type_names, [])
  }
}

///|
fn validate_no_config_cycles(
  root_name : String,
  current : ConfigTypeInfo,
  all_types : Map[String, ConfigTypeInfo],
  visited : Array[String],
) -> Unit raise AgentParseError {
  for field in current.fields {
    let (field_name, pt) = field
    match pt {
      Simple(name) =>
        if name == root_name {
          raise AgentParseError(
            "Config type '\{root_name}' has a cycle through field '\{field_name}'",
          )
        } else {
          match all_types.get(name) {
            Some(nested_ct) => {
              if visited.contains(name) {
                raise AgentParseError(
                  "Config type '\{root_name}' has a cycle through '\{name}'",
                )
              }
              let new_visited = visited.copy()
              new_visited.push(name)
              validate_no_config_cycles(
                root_name, nested_ct, all_types, new_visited,
              )
            }
            None => ()
          }
        }
      _ => validate_no_nested_config_in_container(field_name, pt, all_types)
    }
  }
}

///|
fn validate_no_nested_config_in_container(
  field_name : String,
  pt : ParamType,
  all_types : Map[String, ConfigTypeInfo],
) -> Unit raise AgentParseError {
  match pt {
    Simple(name) => if all_types.get(name) is Some(_) { () }
    Optional(inner) =>
      reject_nested_config("Option", field_name, inner, all_types)
    List(inner) => reject_nested_config("Array", field_name, inner, all_types)
    ResultType(ok, err) => {
      reject_nested_config("Result", field_name, ok, all_types)
      reject_nested_config("Result", field_name, err, all_types)
    }
    Tuple(elems) =>
      for elem in elems {
        reject_nested_config("Tuple", field_name, elem, all_types)
      }
    Parameterized(name, params) =>
      if is_secret_type(pt) {
        match unwrap_secret_type(pt) {
          Some(Simple(inner_name)) =>
            if all_types.get(inner_name) is Some(_) {
              raise AgentParseError(
                "Secret[\{inner_name}] in field '\{field_name}' — Secret cannot wrap a #derive.config type",
              )
            }
          Some(other) =>
            reject_nested_config("Secret", field_name, other, all_types)
          _ => ()
        }
      } else {
        for p in params {
          reject_nested_config(name, field_name, p, all_types)
        }
      }
    _ => ()
  }
}

///|
fn reject_nested_config(
  container : String,
  field_name : String,
  inner : ParamType,
  all_types : Map[String, ConfigTypeInfo],
) -> Unit raise AgentParseError {
  match inner {
    Simple(name) =>
      if all_types.get(name) is Some(_) {
        raise AgentParseError(
          "\{container}[\{name}] in field '\{field_name}' — #derive.config types cannot be nested inside \{container}",
        )
      }
    Optional(inner2) =>
      reject_nested_config(container, field_name, inner2, all_types)
    List(inner2) =>
      reject_nested_config(container, field_name, inner2, all_types)
    ResultType(ok, err) => {
      reject_nested_config(container, field_name, ok, all_types)
      reject_nested_config(container, field_name, err, all_types)
    }
    Tuple(elems) =>
      for elem in elems {
        reject_nested_config(container, field_name, elem, all_types)
      }
    Parameterized(_, params) =>
      for p in params {
        reject_nested_config(container, field_name, p, all_types)
      }
    _ => ()
  }
}
