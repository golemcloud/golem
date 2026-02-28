///|
pub impl[T : HasElementSchema] HasElementSchema for T? with element_schema() {
  let inner = T::element_schema()
  guard inner is @common.ElementSchema::ComponentModel(inner_wit_type) else {
    inner
  }
  let result_nodes : Array[@types.NamedWitTypeNode] = []
  result_nodes.push({
    name: None,
    owner: None,
    type_: @types.WitTypeNode::OptionType(1),
  })
  for node in inner_wit_type.nodes {
    result_nodes.push(node)
  }
  @common.ElementSchema::ComponentModel({ nodes: result_nodes })
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
      @common.ElementValue::ComponentModel({
        nodes: [@types.WitNode::OptionValue(None)],
      })
    Some(v) => {
      let inner = T::to_element_value(v)
      guard inner is @common.ElementValue::ComponentModel(inner_wit) else {
        raise @common.AgentError::InvalidInput(
          "Expected ComponentModel for Option inner value",
        )
      }
      let nodes : Array[@types.WitNode] = [@types.WitNode::OptionValue(Some(1))]
      for node in inner_wit.nodes {
        nodes.push(node)
      }
      @common.ElementValue::ComponentModel({ nodes, })
    }
  }
}

///|
pub impl[T : HasElementSchema] HasElementSchema for Array[T] with element_schema() {
  let inner = T::element_schema()
  guard inner is @common.ElementSchema::ComponentModel(inner_wit_type) else {
    inner
  }
  let result_nodes : Array[@types.NamedWitTypeNode] = []
  result_nodes.push({
    name: None,
    owner: None,
    type_: @types.WitTypeNode::ListType(1),
  })
  for node in inner_wit_type.nodes {
    result_nodes.push(node)
  }
  @common.ElementSchema::ComponentModel({ nodes: result_nodes })
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
  let child_indices : Array[Int] = []
  let nodes : Array[@types.WitNode] = []
  nodes.push(@types.WitNode::ListValue([]))
  for item in self {
    let inner = T::to_element_value(item)
    guard inner is @common.ElementValue::ComponentModel(inner_wit) else {
      raise @common.AgentError::InvalidInput(
        "Expected ComponentModel for Array element",
      )
    }
    let start_idx = nodes.length()
    child_indices.push(start_idx)
    for node in inner_wit.nodes {
      nodes.push(node)
    }
  }
  nodes[0] = @types.WitNode::ListValue(child_indices)
  @common.ElementValue::ComponentModel({ nodes, })
}

///|
pub impl[T : HasElementSchema, E : HasElementSchema] HasElementSchema for Result[
  T,
  E,
] with element_schema() {
  let ok_schema = T::element_schema()
  let err_schema = E::element_schema()
  guard ok_schema is @common.ElementSchema::ComponentModel(ok_wit_type) else {
    ok_schema
  }
  guard err_schema is @common.ElementSchema::ComponentModel(err_wit_type) else {
    err_schema
  }
  let result_nodes : Array[@types.NamedWitTypeNode] = []
  let ok_root = 1
  let err_root = 1 + ok_wit_type.nodes.length()
  result_nodes.push({
    name: None,
    owner: None,
    type_: @types.WitTypeNode::ResultType((Some(ok_root), Some(err_root))),
  })
  for node in ok_wit_type.nodes {
    result_nodes.push(node)
  }
  for node in err_wit_type.nodes {
    result_nodes.push(node)
  }
  @common.ElementSchema::ComponentModel({ nodes: result_nodes })
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
      guard inner is @common.ElementValue::ComponentModel(inner_wit) else {
        raise @common.AgentError::InvalidInput(
          "Expected ComponentModel for Result Ok value",
        )
      }
      let nodes : Array[@types.WitNode] = [
        @types.WitNode::ResultValue(Ok(Some(1))),
      ]
      for node in inner_wit.nodes {
        nodes.push(node)
      }
      @common.ElementValue::ComponentModel({ nodes, })
    }
    Err(e) => {
      let inner = E::to_element_value(e)
      guard inner is @common.ElementValue::ComponentModel(inner_wit) else {
        raise @common.AgentError::InvalidInput(
          "Expected ComponentModel for Result Err value",
        )
      }
      let nodes : Array[@types.WitNode] = [
        @types.WitNode::ResultValue(Err(Some(1))),
      ]
      for node in inner_wit.nodes {
        nodes.push(node)
      }
      @common.ElementValue::ComponentModel({ nodes, })
    }
  }
}
