///|
pub impl[T : HasElementSchema] HasElementSchema for T? with element_schema() {
  let inner = T::element_schema()
  guard inner is @types.ElementSchema::ComponentModel(inner_wit_type) else {
    inner
  }
  @types.ElementSchema::ComponentModel(
    @builder.TypeBuilder::new().option_type(inner_wit_type),
  )
}

///|
pub impl[T : FromExtractor] FromExtractor for T? with from_extractor(e) {
  match e.option() {
    Some(None) => None
    Some(Some(inner)) => Some(T::from_extractor(inner))
    None => raise @common.AgentError::InvalidInput("Expected Option")
  }
}

///|
pub impl[T : FromExtractor] FromElementValue for T? with from_element_value(v) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[T : ToElementValue] ToElementValue for T? with to_element_value(self) {
  match self {
    None =>
      @types.ElementValue::ComponentModel(@builder.Builder::new().option_none())
    Some(v) => {
      let inner = T::to_element_value(v)
      guard inner is @types.ElementValue::ComponentModel(inner_wit) else {
        raise @common.AgentError::InvalidInput(
          "Expected ComponentModel for Option inner value",
        )
      }
      let wit_value = @builder.Builder::new().option_some(item => {
        item.wit_value(inner_wit)
      }) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
      @types.ElementValue::ComponentModel(wit_value)
    }
  }
}

///|
pub impl[T : HasElementSchema] HasElementSchema for Array[T] with element_schema() {
  let inner = T::element_schema()
  guard inner is @types.ElementSchema::ComponentModel(inner_wit_type) else {
    inner
  }
  @types.ElementSchema::ComponentModel(
    @builder.TypeBuilder::new().list_type(inner_wit_type),
  )
}

///|
pub impl[T : FromExtractor] FromExtractor for Array[T] with from_extractor(e) {
  guard e.list_elements() is Some(items) else {
    raise @common.AgentError::InvalidInput("Expected List")
  }
  let result : Array[T] = []
  for item in items {
    result.push(T::from_extractor(item))
  }
  result
}

///|
pub impl[T : FromExtractor] FromElementValue for Array[T] with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[T : ToElementValue] ToElementValue for Array[T] with to_element_value(
  self,
) {
  let inner_values : Array[@types.WitValue] = []
  for item in self {
    let inner = T::to_element_value(item)
    guard inner is @types.ElementValue::ComponentModel(inner_wit) else {
      raise @common.AgentError::InvalidInput(
        "Expected ComponentModel for Array element",
      )
    }
    inner_values.push(inner_wit)
  }
  let wit_value = @builder.Builder::new().list(item => {
    for v in inner_values {
      item.wit_value(v)
    }
  }) catch {
    @builder.BuilderError(msg) => raise @common.AgentError::InvalidInput(msg)
  }
  @types.ElementValue::ComponentModel(wit_value)
}

///|
pub impl[T : HasElementSchema, E : HasElementSchema] HasElementSchema for Result[
  T,
  E,
] with element_schema() {
  let ok_schema = T::element_schema()
  let err_schema = E::element_schema()
  guard ok_schema is @types.ElementSchema::ComponentModel(ok_wit_type) else {
    ok_schema
  }
  guard err_schema is @types.ElementSchema::ComponentModel(err_wit_type) else {
    err_schema
  }
  @types.ElementSchema::ComponentModel(
    @builder.TypeBuilder::new().result_type(ok_wit_type, err_wit_type),
  )
}

///|
pub impl[T : FromExtractor, E : FromExtractor] FromExtractor for Result[T, E] with from_extractor(
  e,
) {
  guard e.result() is Some(r) else {
    raise @common.AgentError::InvalidInput("Expected Result")
  }
  match r {
    Ok(Some(ok_inner)) => Ok(T::from_extractor(ok_inner))
    Ok(None) =>
      raise @common.AgentError::InvalidInput("Expected Result Ok value")
    Err(Some(err_inner)) => Err(E::from_extractor(err_inner))
    Err(None) =>
      raise @common.AgentError::InvalidInput("Expected Result Err value")
  }
}

///|
pub impl[T : FromExtractor, E : FromExtractor] FromElementValue for Result[T, E] with from_element_value(
  v,
) {
  FromExtractor::from_extractor(@extractor.extract_component_model_value(v))
}

///|
pub impl[T : ToElementValue, E : ToElementValue] ToElementValue for Result[T, E] with to_element_value(
  self,
) {
  match self {
    Ok(v) => {
      let inner = T::to_element_value(v)
      guard inner is @types.ElementValue::ComponentModel(inner_wit) else {
        raise @common.AgentError::InvalidInput(
          "Expected ComponentModel for Result Ok value",
        )
      }
      let wit_value = @builder.Builder::new().result_ok(item => {
        item.wit_value(inner_wit)
      }) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
      @types.ElementValue::ComponentModel(wit_value)
    }
    Err(e) => {
      let inner = E::to_element_value(e)
      guard inner is @types.ElementValue::ComponentModel(inner_wit) else {
        raise @common.AgentError::InvalidInput(
          "Expected ComponentModel for Result Err value",
        )
      }
      let wit_value = @builder.Builder::new().result_err(item => {
        item.wit_value(inner_wit)
      }) catch {
        @builder.BuilderError(msg) =>
          raise @common.AgentError::InvalidInput(msg)
      }
      @types.ElementValue::ComponentModel(wit_value)
    }
  }
}
