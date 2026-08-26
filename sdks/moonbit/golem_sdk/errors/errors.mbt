// Construction and decoding of `AgentError`s over the new schema model.
//
// Mirrors the Rust SDK `agentic::errors` (`custom_error` / `internal_error` /
// `invalid_input_error` / `invalid_method_error`) and the TS SDK
// `agentError.ts` (`createCustomError` / `invalidInput` / `invalidMethod` / …).
// `AgentError::CustomError` carries a self-describing `@types.TypedSchemaValue`
// (schema graph + value tree); these helpers build it from, and decode it back
// to, ordinary MoonBit values via the `@schema` conversion layer.

///|
/// Build an `AgentError::CustomError` from any value whose type implements
/// `IntoSchema`, encoding it as a self-describing `TypedSchemaValue`. Follows
/// the same schema conversion path as the Rust SDK
/// (`value.into_typed_schema_value()` → `encode_typed_schema_value`), exposed
/// here as a typed helper. Raises if the value's schema is malformed or the WIT
/// encoding fails.
pub async fn[T : @schema.IntoSchema] custom_error_of(
  value : T,
) -> @common.AgentError {
  let typed = @schema.try_into_typed_schema_value(value)
  let wire = @model.typed_schema_value_to_wit_async(typed) catch {
    error => {
      typed.value.release_resources(@model_host.resource_drops())
      raise error
    }
  }
  @common.AgentError::CustomError(wire)
}

///|
/// Build an `AgentError::CustomError` carrying a plain string message. Mirrors
/// the Rust `custom_error(impl ToString)` / TS `createCustomError(string)` —
/// infallible, because a bare string always produces a well-formed schema.
pub fn custom_error(message : String) -> @common.AgentError {
  let typed = try @schema.try_into_typed_schema_value(message) catch {
    // Unreachable: a `String` schema graph is always well-formed and encodable.
    e => abort("failed to encode custom agent error: \{e}")
  } noraise {
    value => value
  }
  try
    @common.AgentError::CustomError(@model.typed_schema_value_to_wit(typed))
  catch {
    e => abort("failed to encode custom agent error: \{e}")
  } noraise {
    error => error
  }
}

///|
/// Build an `AgentError::CustomError` prefixed with `"Internal error: "`.
/// Mirrors the Rust `internal_error`.
pub fn internal_error(message : String) -> @common.AgentError {
  custom_error("Internal error: \{message}")
}

///|
/// Build an `AgentError::InvalidInput`. Mirrors the Rust `invalid_input_error`.
pub fn invalid_input_error(message : String) -> @common.AgentError {
  @common.AgentError::InvalidInput(message)
}

///|
/// Build an `AgentError::InvalidMethod`. Mirrors the Rust `invalid_method_error`.
pub fn invalid_method_error(method_name : String) -> @common.AgentError {
  @common.AgentError::InvalidMethod(method_name)
}

///|
/// Build an `AgentError::InvalidType`. Mirrors the TS `invalidType`.
pub fn invalid_type_error(message : String) -> @common.AgentError {
  @common.AgentError::InvalidType(message)
}

///|
/// Decode the `TypedSchemaValue` carried by an `AgentError::CustomError` back
/// into a value whose type implements `FromSchema`. Returns `None` for any
/// other `AgentError` variant. Raises if the carried value does not match `T`.
///
/// This is the "surface on decode" counterpart to `custom_error_of`: an agent
/// or RPC client that receives a `CustomError` can recover the original typed
/// payload (or render it).
pub fn[T : @schema.FromSchema] decode_custom_error(
  error : @common.AgentError,
) -> T? raise {
  decode_custom_error_with_resource_drops(error, @model_host.resource_drops())
}

///|
fn[T : @schema.FromSchema] decode_custom_error_with_resource_drops(
  error : @common.AgentError,
  drops : @model.SchemaValueResourceDrops,
) -> T? raise {
  match error {
    CustomError(typed) => {
      let decoded = @model.typed_schema_value_from_wit(typed, drops)
      try Some(@schema.from_value_as(decoded.value)) catch {
        error => {
          decoded.value.release_resources(drops)
          raise error
        }
      } noraise {
        value => value
      }
    }
    _ => None
  }
}
