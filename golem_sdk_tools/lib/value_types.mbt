///|
pub(all) enum VariantPayload {
  None
  Single(ParamType)
  Multi(Array[(String, ParamType)], Bool)
} derive(Show, Eq)

///|
pub(all) enum ValueTypeKind {
  Record(Array[(String, ParamType)])
  SimpleEnum(Array[String])
  VariantEnum(Array[(String, VariantPayload)])
} derive(Show, Eq)

///|
pub(all) struct ValueTypeInfo {
  name : String
  kind : ValueTypeKind
} derive(Show, Eq)

///|
pub(all) struct MultimodalEnumInfo {
  name : String
  variants : Array[(String, ParamType)]
} derive(Show)

///|
pub fn parse_multimodal_enums(
  files : Array[(String, String)],
) -> Array[MultimodalEnumInfo] raise AgentParseError {
  let results : Array[MultimodalEnumInfo] = []
  for pair in files {
    let (_filename, content) = pair
    let (impls, _reports) = @parser.parse_string(content)
    let impl_arr = impls.to_array()
    for impl_ in impl_arr {
      match impl_ {
        TopTypeDef(type_decl) => {
          let mut is_multimodal = false
          let attr_arr = type_decl.attrs.to_array()
          for attr in attr_arr {
            if attr.raw == "#derive.multimodal" {
              is_multimodal = true
            }
          }
          if is_multimodal {
            let name = type_decl.tycon
            match type_decl.components {
              Variant(constrs) => {
                let variants : Array[(String, ParamType)] = []
                let constr_arr = constrs.to_array()
                for constr in constr_arr {
                  let case_name = constr.name.name
                  match constr.args {
                    Some(params) => {
                      let param_arr = params.to_array()
                      if param_arr.length() == 1 {
                        let pt = syntax_type_to_param_type(param_arr[0].ty)
                        variants.push((case_name, pt))
                      } else {
                        raise AgentParseError(
                          "Multimodal enum variant '\{case_name}' must have exactly one payload type",
                        )
                      }
                    }
                    None =>
                      raise AgentParseError(
                        "Multimodal enum variant '\{case_name}' must have a payload type",
                      )
                  }
                }
                results.push({ name, variants })
              }
              _ =>
                raise AgentParseError(
                  "#derive.multimodal can only be applied to enum types, but '\{name}' is not an enum",
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
pub fn parse_value_types(
  files : Array[(String, String)],
) -> Array[ValueTypeInfo] raise AgentParseError {
  let results : Array[ValueTypeInfo] = []
  for pair in files {
    let (_filename, content) = pair
    let (impls, _reports) = @parser.parse_string(content)
    let impl_arr = impls.to_array()
    for impl_ in impl_arr {
      match impl_ {
        TopTypeDef(type_decl) => {
          let mut is_schema = false
          let attr_arr = type_decl.attrs.to_array()
          for attr in attr_arr {
            if attr.raw == "#derive.golem_schema" {
              is_schema = true
            }
          }
          if is_schema {
            let name = type_decl.tycon
            let kind = parse_type_desc(name, type_decl.components)
            results.push({ name, kind })
          }
        }
        _ => ()
      }
    }
  }
  results
}

///|
fn parse_type_desc(
  name : String,
  desc : @syntax.TypeDesc,
) -> ValueTypeKind raise AgentParseError {
  match desc {
    Record(fields) => {
      let result : Array[(String, ParamType)] = []
      let field_arr = fields.to_array()
      for field in field_arr {
        let field_name = field.name.label
        let field_type = syntax_type_to_param_type(field.ty)
        result.push((field_name, field_type))
      }
      Record(result)
    }
    Variant(constrs) => {
      let cases : Array[(String, VariantPayload)] = []
      let constr_arr = constrs.to_array()
      for constr in constr_arr {
        let case_name = constr.name.name
        let payload = parse_constr_payload(case_name, constr.args)
        cases.push((case_name, payload))
      }
      let mut all_unit = true
      for pair in cases {
        match pair.1 {
          None => ()
          _ => {
            all_unit = false
            break
          }
        }
      }
      if all_unit {
        let names : Array[String] = []
        for pair in cases {
          names.push(pair.0)
        }
        SimpleEnum(names)
      } else {
        VariantEnum(cases)
      }
    }
    _ =>
      raise AgentParseError(
        "Unsupported type description for '\{name}': expected struct or enum",
      )
  }
}

///|
fn parse_constr_payload(
  _case_name : String,
  args : @list.List[@syntax.ConstrParam]?,
) -> VariantPayload raise AgentParseError {
  match args {
    None => VariantPayload::None
    Some(params) => {
      let items : Array[(String, ParamType)] = []
      let param_arr = params.to_array()
      let mut any_labelled = false
      for idx, param in param_arr {
        let param_type = syntax_type_to_param_type(param.ty)
        let field_name = match param.label {
          Some(label) => {
            any_labelled = true
            label.name
          }
          None => "field\{idx}"
        }
        items.push((field_name, param_type))
      }
      if items.length() == 0 {
        VariantPayload::None
      } else if items.length() == 1 && not(any_labelled) {
        Single(items[0].1)
      } else {
        Multi(items, any_labelled)
      }
    }
  }
}
