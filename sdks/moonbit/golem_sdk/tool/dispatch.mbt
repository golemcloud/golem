///|
pub fn decode_invocation_input(
  input : @types.TypedSchemaValue,
  arity : Int,
  context : String,
) -> Array[@model.SchemaValue] raise @toolCommon.ToolError {
  let typed = @model.typed_schema_value_from_wit(input) catch {
    e =>
      raise @toolCommon.ToolError::InvalidInput(
        "\{context}: invalid typed schema value input: \{e}",
      )
  }
  validate_invocation_input_graph(typed.graph, arity, context)
  if !value_matches_type(typed.graph, typed.graph.root, typed.value) {
    raise @toolCommon.ToolError::InvalidInput(
      "\{context}: input value does not match input schema graph",
    )
  }
  let fields = @schema.expect_record(typed.value, context) catch {
    e =>
      raise @toolCommon.ToolError::InvalidInput(
        "\{context}: expected a record input: \{e}",
      )
  }
  guard fields.length() == arity else {
    raise @toolCommon.ToolError::InvalidInput(
      "\{context}: expected \{arity} parameter(s), got \{fields.length()}",
    )
  }
  fields
}

///|
fn validate_invocation_input_graph(
  graph : @model.SchemaGraph,
  arity : Int,
  context : String,
) -> Unit raise @toolCommon.ToolError {
  if @model.validate_graph(graph).length() > 0 {
    raise @toolCommon.ToolError::InvalidInput(
      "\{context}: invalid input schema graph",
    )
  }
  match resolved_record_field_count(graph, graph.root, {}) {
    Some(field_count) =>
      if field_count != arity {
        raise @toolCommon.ToolError::InvalidInput(
          "\{context}: input schema record has \{field_count} field(s), expected \{arity}",
        )
      }
    None =>
      raise @toolCommon.ToolError::InvalidInput(
        "\{context}: input schema root must be a record",
      )
  }
}

///|
fn resolved_record_field_count(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
  visited : Map[String, Unit],
) -> Int? {
  match type_.body {
    @model.SchemaTypeBody::Record(fields) => Some(fields.length())
    @model.SchemaTypeBody::Ref(id) =>
      if visited.contains(id) {
        None
      } else {
        visited[id] = ()
        match lookup_type_def(graph, id) {
          Some(def) => resolved_record_field_count(graph, def.body, visited)
          None => None
        }
      }
    _ => None
  }
}

///|
fn lookup_type_def(
  graph : @model.SchemaGraph,
  id : String,
) -> @model.SchemaTypeDef? {
  for def in graph.defs {
    if def.id == id {
      return Some(def)
    }
  }
  None
}

///|
fn value_matches_type(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
  value : @model.SchemaValue,
) -> Bool {
  match type_.body {
    @model.SchemaTypeBody::Ref(id) =>
      match resolve_ref(graph, id, {}) {
        Some(resolved) => value_matches_type(graph, resolved, value)
        None => false
      }
    @model.SchemaTypeBody::Bool => value is @model.SchemaValue::Bool(_)
    @model.SchemaTypeBody::S8(_) => value is @model.SchemaValue::S8(_)
    @model.SchemaTypeBody::S16(_) => value is @model.SchemaValue::S16(_)
    @model.SchemaTypeBody::S32(_) => value is @model.SchemaValue::S32(_)
    @model.SchemaTypeBody::S64(_) => value is @model.SchemaValue::S64(_)
    @model.SchemaTypeBody::U8(_) => value is @model.SchemaValue::U8(_)
    @model.SchemaTypeBody::U16(_) => value is @model.SchemaValue::U16(_)
    @model.SchemaTypeBody::U32(_) => value is @model.SchemaValue::U32(_)
    @model.SchemaTypeBody::U64(_) => value is @model.SchemaValue::U64(_)
    @model.SchemaTypeBody::F32(_) => value is @model.SchemaValue::F32(_)
    @model.SchemaTypeBody::F64(_) => value is @model.SchemaValue::F64(_)
    @model.SchemaTypeBody::Char => value is @model.SchemaValue::Char(_)
    @model.SchemaTypeBody::String => value is @model.SchemaValue::String(_)
    @model.SchemaTypeBody::Record(fields) =>
      match value {
        @model.SchemaValue::Record(values) =>
          fields.length() == values.length() &&
          record_fields_match(graph, fields, values)
        _ => false
      }
    @model.SchemaTypeBody::Variant(cases) =>
      match value {
        @model.SchemaValue::Variant(case_idx, payload) => {
          let idx = case_idx.reinterpret_as_int()
          idx >= 0 &&
          idx < cases.length() &&
          optional_payload_matches(graph, cases[idx].payload, payload)
        }
        _ => false
      }
    @model.SchemaTypeBody::Enum(cases) =>
      match value {
        @model.SchemaValue::Enum(case_idx) => {
          let idx = case_idx.reinterpret_as_int()
          idx >= 0 && idx < cases.length()
        }
        _ => false
      }
    @model.SchemaTypeBody::Flags(labels) =>
      match value {
        @model.SchemaValue::Flags(values) => labels.length() == values.length()
        _ => false
      }
    @model.SchemaTypeBody::Tuple(elements) =>
      match value {
        @model.SchemaValue::Tuple(values) =>
          elements.length() == values.length() &&
          tuple_elements_match(graph, elements, values)
        _ => false
      }
    @model.SchemaTypeBody::List(element) =>
      match value {
        @model.SchemaValue::List(values) =>
          repeated_values_match(graph, element, values)
        _ => false
      }
    @model.SchemaTypeBody::FixedList(element, length) =>
      match value {
        @model.SchemaValue::FixedList(values) =>
          values.length() == length.reinterpret_as_int() &&
          repeated_values_match(graph, element, values)
        _ => false
      }
    @model.SchemaTypeBody::Map(key_type, value_type) =>
      match value {
        @model.SchemaValue::Map(entries) =>
          map_entries_match(graph, key_type, value_type, entries)
        _ => false
      }
    @model.SchemaTypeBody::Option(inner) =>
      match value {
        @model.SchemaValue::Option(payload) =>
          optional_payload_matches(graph, Some(inner), payload)
        _ => false
      }
    @model.SchemaTypeBody::Result(ok_type, err_type) =>
      match value {
        @model.SchemaValue::ResultOk(payload) =>
          optional_payload_matches(graph, ok_type, payload)
        @model.SchemaValue::ResultErr(payload) =>
          optional_payload_matches(graph, err_type, payload)
        _ => false
      }
    @model.SchemaTypeBody::Text(_) => value is @model.SchemaValue::Text(_)
    @model.SchemaTypeBody::Binary(_) => value is @model.SchemaValue::Binary(_)
    @model.SchemaTypeBody::Path(_) => value is @model.SchemaValue::Path(_)
    @model.SchemaTypeBody::Url(_) => value is @model.SchemaValue::Url(_)
    @model.SchemaTypeBody::Datetime => value is @model.SchemaValue::Datetime(_)
    @model.SchemaTypeBody::Duration => value is @model.SchemaValue::Duration(_)
    @model.SchemaTypeBody::Quantity(_) =>
      value is @model.SchemaValue::Quantity(_)
    @model.SchemaTypeBody::Union(branches) =>
      match value {
        @model.SchemaValue::Union(tag, body) =>
          union_branch_matches(graph, branches, tag, body)
        _ => false
      }
    @model.SchemaTypeBody::Secret(_) => value is @model.SchemaValue::Secret(_)
    @model.SchemaTypeBody::QuotaToken(_) =>
      value is @model.SchemaValue::QuotaToken(_)
    @model.SchemaTypeBody::Future(_) => false
    @model.SchemaTypeBody::Stream(_) => false
  }
}

///|
fn resolve_ref(
  graph : @model.SchemaGraph,
  id : String,
  visited : Map[String, Unit],
) -> @model.SchemaType? {
  if visited.contains(id) {
    None
  } else {
    visited[id] = ()
    match lookup_type_def(graph, id) {
      Some(def) =>
        match def.body.body {
          @model.SchemaTypeBody::Ref(next) => resolve_ref(graph, next, visited)
          _ => Some(def.body)
        }
      None => None
    }
  }
}

///|
fn record_fields_match(
  graph : @model.SchemaGraph,
  fields : Array[@model.NamedFieldType],
  values : Array[@model.SchemaValue],
) -> Bool {
  for i = 0; i < fields.length(); i = i + 1 {
    if !value_matches_type(graph, fields[i].body, values[i]) {
      return false
    }
  }
  true
}

///|
fn tuple_elements_match(
  graph : @model.SchemaGraph,
  elements : Array[@model.SchemaType],
  values : Array[@model.SchemaValue],
) -> Bool {
  for i = 0; i < elements.length(); i = i + 1 {
    if !value_matches_type(graph, elements[i], values[i]) {
      return false
    }
  }
  true
}

///|
fn repeated_values_match(
  graph : @model.SchemaGraph,
  element : @model.SchemaType,
  values : Array[@model.SchemaValue],
) -> Bool {
  for value in values {
    if !value_matches_type(graph, element, value) {
      return false
    }
  }
  true
}

///|
fn map_entries_match(
  graph : @model.SchemaGraph,
  key_type : @model.SchemaType,
  value_type : @model.SchemaType,
  entries : Array[@model.SchemaMapEntry],
) -> Bool {
  for entry in entries {
    if !value_matches_type(graph, key_type, entry.key) ||
      !value_matches_type(graph, value_type, entry.value) {
      return false
    }
  }
  true
}

///|
fn optional_payload_matches(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType?,
  value : @model.SchemaValue?,
) -> Bool {
  match (type_, value) {
    (None, None) => true
    (Some(type_), Some(value)) => value_matches_type(graph, type_, value)
    _ => false
  }
}

///|
fn union_branch_matches(
  graph : @model.SchemaGraph,
  branches : Array[@model.UnionBranch],
  tag : String,
  body : @model.SchemaValue,
) -> Bool {
  for branch in branches {
    if branch.tag == tag {
      return value_matches_type(graph, branch.body, body) &&
        discriminator_matches(graph, branch, body)
    }
  }
  false
}

///|
fn discriminator_matches(
  graph : @model.SchemaGraph,
  branch : @model.UnionBranch,
  value : @model.SchemaValue,
) -> Bool {
  match branch.discriminator {
    @types.DiscriminatorRule::Prefix(prefix) =>
      match discriminator_string(value) {
        Some(value) => value.has_prefix(prefix)
        None => false
      }
    @types.DiscriminatorRule::Suffix(suffix) =>
      match discriminator_string(value) {
        Some(value) => value.has_suffix(suffix)
        None => false
      }
    @types.DiscriminatorRule::Contains(needle) =>
      match discriminator_string(value) {
        Some(value) => value.contains(needle)
        None => false
      }
    @types.DiscriminatorRule::Regex(_) => discriminator_string(value) is Some(_)
    @types.DiscriminatorRule::FieldEquals(field) =>
      match value {
        @model.SchemaValue::Record(values) =>
          match record_field_index(graph, branch.body, field.field_name) {
            Some(idx) =>
              match field.literal {
                Some(literal) =>
                  match discriminator_string(values[idx]) {
                    Some(value) => value == literal
                    None => false
                  }
                None => true
              }
            None => false
          }
        _ => false
      }
    @types.DiscriminatorRule::FieldAbsent(field_name) =>
      match value {
        @model.SchemaValue::Record(_) =>
          match record_field_index(graph, branch.body, field_name) {
            Some(_) => false
            None => true
          }
        _ => false
      }
  }
}

///|
fn discriminator_string(value : @model.SchemaValue) -> String? {
  match value {
    @model.SchemaValue::String(value) => Some(value)
    @model.SchemaValue::Text(value, _) => Some(value)
    @model.SchemaValue::Url(value) => Some(value)
    @model.SchemaValue::Path(value) => Some(value)
    _ => None
  }
}

///|
fn record_field_index(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
  field_name : String,
) -> Int? {
  match resolve_type(graph, type_) {
    Some(type_) =>
      match type_.body {
        @model.SchemaTypeBody::Record(fields) => {
          for i = 0; i < fields.length(); i = i + 1 {
            if fields[i].name == field_name {
              return Some(i)
            }
          }
          None
        }
        _ => None
      }
    None => None
  }
}

///|
fn resolve_type(
  graph : @model.SchemaGraph,
  type_ : @model.SchemaType,
) -> @model.SchemaType? {
  match type_.body {
    @model.SchemaTypeBody::Ref(id) => resolve_ref(graph, id, {})
    _ => Some(type_)
  }
}

///|
pub fn[T : @schema.FromSchema] decode_param(
  fields : Array[@model.SchemaValue],
  index : Int,
  context : String,
) -> T raise @toolCommon.ToolError {
  let field = @schema.record_field(fields, index, context) catch {
    e =>
      raise @toolCommon.ToolError::InvalidInput(
        "\{context}: missing parameter \{index}: \{e}",
      )
  }
  @schema.from_value_as(field) catch {
    e =>
      raise @toolCommon.ToolError::InvalidInput(
        "\{context}: failed decoding parameter \{index}: \{e}",
      )
  }
}

///|
pub fn encode_invocation_input(
  graph : @model.SchemaGraph,
  values : Array[@model.SchemaValue],
) -> @model.TypedSchemaValue {
  { graph, value: @model.SchemaValue::Record(values) }
}

///|
pub fn[T : @schema.IntoSchema] encode_result_value(
  value : T,
) -> @types.TypedSchemaValue raise @toolCommon.ToolError {
  @model.typed_schema_value_to_wit(@schema.try_into_typed_schema_value(value)) catch {
    e =>
      raise @toolCommon.ToolError::InvalidResult(
        "failed serializing tool result: \{e}",
      )
  }
}

///|
pub fn invocation_result_empty() -> @toolCommon.InvocationResult {
  { result: None, stdout: None }
}

///|
pub fn[T : @schema.IntoSchema] invocation_result_value(
  value : T,
) -> @toolCommon.InvocationResult raise @toolCommon.ToolError {
  { result: Some(encode_result_value(value)), stdout: None }
}

///|
pub fn invocation_result_stdout(
  stdout : @streams.OutputStream,
) -> @toolCommon.InvocationResult {
  { result: None, stdout: Some(stdout) }
}

///|
pub fn[T : @schema.IntoSchema] invocation_result_value_with_stdout(
  value : T,
  stdout : @streams.OutputStream,
) -> @toolCommon.InvocationResult raise @toolCommon.ToolError {
  { result: Some(encode_result_value(value)), stdout: Some(stdout) }
}
