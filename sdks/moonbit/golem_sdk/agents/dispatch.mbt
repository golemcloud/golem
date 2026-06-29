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
/// Decodes an invocation/constructor input wire value into its ordered
/// parameter values, validating that it is a record of exactly `arity` fields.
/// Decode and shape errors are mapped to `AgentError::InvalidInput`.
pub fn decode_invocation_input(
  input : @types.SchemaValueTree,
  arity : Int,
  context : String,
) -> Array[@model.SchemaValue] raise @common.AgentError {
  let value = @model.schema_value_from_wit(input) catch {
    e =>
      raise @common.AgentError::InvalidInput(
        "\{context}: invalid schema value input: \{e}",
      )
  }
  let fields = @schema.expect_record(value, context) catch {
    e =>
      raise @common.AgentError::InvalidInput(
        "\{context}: expected a record input: \{e}",
      )
  }
  guard fields.length() == arity else {
    raise @common.AgentError::InvalidInput(
      "\{context}: expected \{arity} parameter(s), got \{fields.length()}",
    )
  }
  fields
}

///|
/// Decodes a single parameter at `index` from a previously decoded input record.
/// Decode errors are mapped to `AgentError::InvalidInput`.
pub fn[T : @schema.FromSchema] decode_param(
  fields : Array[@model.SchemaValue],
  index : Int,
  context : String,
) -> T raise @common.AgentError {
  let field = @schema.record_field(fields, index, context) catch {
    e =>
      raise @common.AgentError::InvalidInput(
        "\{context}: missing parameter \{index}: \{e}",
      )
  }
  @schema.from_value_as(field) catch {
    e =>
      raise @common.AgentError::InvalidInput(
        "\{context}: failed decoding parameter \{index}: \{e}",
      )
  }
}

///|
/// Encodes a method/constructor return value into a wire output value.
/// Serialization errors are mapped to `AgentError::InvalidType`.
pub fn[T : @schema.IntoSchema] encode_output(
  value : T,
) -> @types.SchemaValueTree raise @common.AgentError {
  @model.schema_value_to_wit(@schema.to_value_as(value)) catch {
    e =>
      raise @common.AgentError::InvalidType(
        "failed serializing return value: \{e}",
      )
  }
}

///|
/// Encodes the ordered parameter values of an invocation into a wire input
/// record (RPC client side). Serialization errors are mapped to
/// `AgentError::InvalidInput`.
pub fn encode_invocation_input(
  values : Array[@model.SchemaValue],
) -> @types.SchemaValueTree raise @common.AgentError {
  @model.schema_value_to_wit(@model.SchemaValue::Record(values)) catch {
    e =>
      raise @common.AgentError::InvalidInput(
        "failed encoding invocation input: \{e}",
      )
  }
}

///|
/// Decodes the single output value of an RPC invocation result.
/// Decode errors are mapped to `AgentError::InvalidType`.
pub fn[T : @schema.FromSchema] decode_output(
  output : @types.SchemaValueTree,
  context : String,
) -> T raise @common.AgentError {
  let value = @model.schema_value_from_wit(output) catch {
    e =>
      raise @common.AgentError::InvalidType(
        "\{context}: invalid schema value output: \{e}",
      )
  }
  @schema.from_value_as(value) catch {
    e =>
      raise @common.AgentError::InvalidType(
        "\{context}: failed decoding return value: \{e}",
      )
  }
}
