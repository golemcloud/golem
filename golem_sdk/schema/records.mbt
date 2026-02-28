///|
pub fn make_record_schema(
  fields : Array[(String, @common.ElementSchema)],
) -> @common.ElementSchema {
  let nodes : Array[@types.NamedWitTypeNode] = [
    { name: None, owner: None, type_: @types.WitTypeNode::RecordType([]) },
  ]
  let field_entries : Array[(String, Int)] = []
  for i in 0..<fields.length() {
    let (name, schema) = fields[i]
    guard schema is @common.ElementSchema::ComponentModel(inner_wit_type) else {
      abort("make_record_schema: field \{name} is not a ComponentModel schema")
    }
    let root = merge_wit_type(nodes, inner_wit_type)
    field_entries.push((name, root))
  }
  nodes[0] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::RecordType(field_entries),
  }
  @common.ElementSchema::ComponentModel({ nodes, })
}

///|
pub fn make_record_value(
  fields : Array[@common.ElementValue],
) -> @common.ElementValue raise @common.AgentError {
  let wit_values : Array[@types.WitValue] = []
  for i in 0..<fields.length() {
    guard fields[i] is @common.ElementValue::ComponentModel(inner_wit) else {
      raise @common.AgentError::InvalidInput(
        "Expected ComponentModel for record field at index " + i.to_string(),
      )
    }
    wit_values.push(inner_wit)
  }
  let wit_value = @builder.Builder::new().record(item => {
    for v in wit_values {
      item.wit_value(v)
    }
  }) catch {
    @builder.BuilderError(msg) => raise @common.AgentError::InvalidInput(msg)
  }
  @common.ElementValue::ComponentModel(wit_value)
}

///|
pub fn[T : FromExtractor] extract_field(
  e : &@extractor.Extractor,
  idx : Int,
) -> T raise @common.AgentError {
  guard e.field(idx) is Some(f) else {
    raise @common.AgentError::InvalidInput(
      "Expected field at index " + idx.to_string(),
    )
  }
  T::from_extractor(f)
}

///|
pub fn make_enum_schema(cases : Array[String]) -> @common.ElementSchema {
  @common.ElementSchema::ComponentModel({
    nodes: [
      { name: None, owner: None, type_: @types.WitTypeNode::EnumType(cases) },
    ],
  })
}

///|
pub fn make_enum_value(idx : UInt) -> @common.ElementValue {
  @common.ElementValue::ComponentModel(@builder.Builder::new().enum_value(idx))
}

///|
pub fn extract_enum(e : &@extractor.Extractor) -> UInt raise @common.AgentError {
  guard e.enum_value() is Some(idx) else {
    raise @common.AgentError::InvalidInput("Expected enum value")
  }
  idx
}

///|
pub fn make_variant_schema(
  cases : Array[(String, @common.ElementSchema?)],
) -> @common.ElementSchema {
  let nodes : Array[@types.NamedWitTypeNode] = [
    { name: None, owner: None, type_: @types.WitTypeNode::VariantType([]) },
  ]
  let case_entries : Array[(String, Int?)] = []
  for i in 0..<cases.length() {
    let (name, payload) = cases[i]
    match payload {
      None => case_entries.push((name, None))
      Some(schema) => {
        guard schema is @common.ElementSchema::ComponentModel(inner_wit_type) else {
          abort(
            "make_variant_schema: case \{name} payload is not a ComponentModel schema",
          )
        }
        let root = merge_wit_type(nodes, inner_wit_type)
        case_entries.push((name, Some(root)))
      }
    }
  }
  nodes[0] = {
    name: None,
    owner: None,
    type_: @types.WitTypeNode::VariantType(case_entries),
  }
  @common.ElementSchema::ComponentModel({ nodes, })
}

///|
pub fn make_variant_value(
  case_idx : UInt,
  payload : @common.ElementValue?,
) -> @common.ElementValue raise @common.AgentError {
  match payload {
    None =>
      @common.ElementValue::ComponentModel(
        @builder.Builder::new().variant_unit(case_idx),
      )
    Some(p) => {
      guard p is @common.ElementValue::ComponentModel(inner_wit) else {
        raise @common.AgentError::InvalidInput(
          "Expected ComponentModel for variant payload",
        )
      }
      let wit_value = @builder.Builder::new().variant(case_idx, item => {
        item.wit_value(inner_wit)
      }) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
      @common.ElementValue::ComponentModel(wit_value)
    }
  }
}

///|
pub fn extract_variant(
  e : &@extractor.Extractor,
) -> (UInt, &@extractor.Extractor?) raise @common.AgentError {
  guard e.variant() is Some(result) else {
    raise @common.AgentError::InvalidInput("Expected variant value")
  }
  result
}
