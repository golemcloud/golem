// Runtime helpers for generated agent dispatch (`golem_agents.mbt`) and RPC
// clients (`golem_clients.mbt`). They centralize the wire <-> schema-value
// conversions and the error mapping to `@common.AgentError`, so the generated
// code stays small and only ever raises `AgentError`.
//
// Invocation/constructor inputs are encoded as a `SchemaValue::Record` whose
// fields are the ordered parameter values (matching the Rust/TS SDKs). A
// method's output is `None` for a unit return and `Some(value)` for a single
// returned value.

///|
/// Decodes a wire input record into its ordered fields.
fn decode_input_record(
  input : @types.SchemaValueTree,
  context : String,
) -> Array[@model.SchemaValue] raise @common.AgentError {
  let drops = @model_host.resource_drops()
  let value = @model_host.schema_value_from_wit(input) catch {
    e =>
      raise @common.AgentError::InvalidInput(
        "\{context}: invalid schema value input: \{repr(e)}",
      )
  }
  let fields = @schema.expect_record(value, context) catch {
    e => {
      value.release_resources(drops)
      raise @common.AgentError::InvalidInput(
        "\{context}: expected a record input: \{repr(e)}",
      )
    }
  }
  fields
}

///|
/// Decodes an invocation/constructor input wire value into its ordered
/// parameter values, validating that it is a record of exactly `arity` fields.
/// Decode and shape errors are mapped to `AgentError::InvalidInput`.
#warnings("-unused_async")
pub async fn decode_invocation_input(
  input : @types.SchemaValueTree,
  arity : Int,
  context : String,
) -> Array[@model.SchemaValue] raise @common.AgentError {
  let fields = decode_input_record(input, context)
  guard fields.length() == arity else {
    @model.SchemaValue::Record(fields).release_resources(
      @model_host.resource_drops(),
    )
    raise @common.AgentError::InvalidInput(
      "\{context}: expected \{arity} parameter(s), got \{fields.length()}",
    )
  }
  fields
}

///|
/// Decodes the identity record from a parsed agent ID for snapshot restoration.
#warnings("-unused_async")
pub async fn decode_snapshot_identity(
  input : @types.SchemaValueTree,
) -> Array[@model.SchemaValue] raise @common.AgentError {
  decode_input_record(input, "snapshot restoration")
}

///|
/// Decodes a single parameter at `index` from a previously decoded input record.
/// Decode errors are mapped to `AgentError::InvalidInput`.
pub fn[T : @schema.FromSchema] decode_param(
  fields : Array[@model.SchemaValue],
  index : Int,
  context : String,
) -> T raise @common.AgentError {
  decode_param_with_resource_drops(
    fields,
    index,
    context,
    @model_host.resource_drops(),
  )
}

///|
fn[T : @schema.FromSchema] decode_param_with_resource_drops(
  fields : Array[@model.SchemaValue],
  index : Int,
  context : String,
  drops : @model.SchemaValueResourceDrops,
) -> T raise @common.AgentError {
  let field = @schema.record_field(fields, index, context) catch {
    e => {
      @model.SchemaValue::Record(fields).release_resources(drops)
      raise @common.AgentError::InvalidInput(
        "\{context}: missing parameter \{index}: \{repr(e)}",
      )
    }
  }
  @schema.from_value_as(field) catch {
    e => {
      @model.SchemaValue::Record(fields).release_resources(drops)
      raise @common.AgentError::InvalidInput(
        "\{context}: failed decoding parameter \{index}: \{repr(e)}",
      )
    }
  }
}

///|
/// Encodes a method/constructor return value into a wire output value.
/// Serialization errors are mapped to `AgentError::InvalidType`.
pub async fn[T : @schema.IntoSchema] encode_output(
  value : T,
) -> @types.SchemaValueTree raise @common.AgentError {
  let model_value = @schema.to_value_as(value)
  @model.schema_value_to_wit_async(model_value) catch {
    e => {
      model_value.release_resources(@model_host.resource_drops())
      raise @common.AgentError::InvalidType(
        "failed serializing return value: \{repr(e)}",
      )
    }
  }
}

///|
/// Encodes the ordered parameter values of an invocation into a wire input
/// record (RPC client side). Serialization errors are mapped to
/// `AgentError::InvalidInput`.
pub fn encode_invocation_input(
  values : Array[@model.SchemaValue],
) -> @types.SchemaValueTree raise @common.AgentError {
  let model_value = @model.SchemaValue::Record(values)
  @model.schema_value_to_wit(model_value) catch {
    e => {
      model_value.release_resources(@model_host.resource_drops())
      raise @common.AgentError::InvalidInput(
        "failed encoding invocation input: \{repr(e)}",
      )
    }
  }
}

///|
/// Asynchronously encodes the ordered parameter values of an awaited
/// invocation, including native streams nested anywhere in the input record.
pub async fn encode_invocation_input_async(
  values : Array[@model.SchemaValue],
) -> @types.SchemaValueTree raise @common.AgentError {
  let model_value = @model.SchemaValue::Record(values)
  @model.schema_value_to_wit_async(model_value) catch {
    e => {
      model_value.release_resources(@model_host.resource_drops())
      raise @common.AgentError::InvalidInput(
        "failed encoding invocation input: \{repr(e)}",
      )
    }
  }
}

///|
/// Decodes the single output value of an RPC invocation result.
/// Decode errors are mapped to `AgentError::InvalidType`.
#warnings("-unused_async")
pub async fn[T : @schema.FromSchema] decode_output(
  output : @types.SchemaValueTree,
  context : String,
) -> T raise @common.AgentError {
  let drops = @model_host.resource_drops()
  let value = @model_host.schema_value_from_wit(output) catch {
    e =>
      raise @common.AgentError::InvalidType(
        "\{context}: invalid schema value output: \{repr(e)}",
      )
  }
  @schema.from_value_as(value) catch {
    e => {
      value.release_resources(drops)
      raise @common.AgentError::InvalidType(
        "\{context}: failed decoding return value: \{repr(e)}",
      )
    }
  }
}

///|
/// Releases a wire value that cannot be delivered to an agent method.
pub fn discard_wire_value(value : @types.SchemaValueTree) -> Unit {
  let drops = @model_host.resource_drops()
  try @model_host.schema_value_from_wit(value) catch {
    _ => ()
  } noraise {
    decoded => decoded.release_resources(drops)
  }
}

///|
/// Releases an optional invocation result that the caller does not consume.
pub fn discard_optional_wire_value(value : @types.SchemaValueTree?) -> Unit {
  match value {
    Some(tree) => discard_wire_value(tree)
    None => ()
  }
}
