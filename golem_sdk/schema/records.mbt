///|
pub fn make_record_schema(
  fields : Array[(String, @common.ElementSchema)],
) -> @common.ElementSchema {
  let wit_fields : Array[(String, @types.WitType)] = []
  for i in 0..<fields.length() {
    let (name, schema) = fields[i]
    guard schema is @common.ElementSchema::ComponentModel(inner_wit_type) else {
      abort("make_record_schema: field \{name} is not a ComponentModel schema")
    }
    wit_fields.push((name, inner_wit_type))
  }
  @common.ElementSchema::ComponentModel(
    @builder.TypeBuilder::new().record_type(wit_fields),
  )
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
  @common.ElementSchema::ComponentModel(
    @builder.TypeBuilder::new().enum_type(cases),
  )
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
  let wit_cases : Array[(String, @types.WitType?)] = []
  for i in 0..<cases.length() {
    let (name, payload) = cases[i]
    match payload {
      None => wit_cases.push((name, None))
      Some(schema) => {
        guard schema is @common.ElementSchema::ComponentModel(inner_wit_type) else {
          abort(
            "make_variant_schema: case \{name} payload is not a ComponentModel schema",
          )
        }
        wit_cases.push((name, Some(inner_wit_type)))
      }
    }
  }
  @common.ElementSchema::ComponentModel(
    @builder.TypeBuilder::new().variant_type(wit_cases),
  )
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
