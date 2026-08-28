///|
pub(all) struct InvocationResult {
  result : @model.TypedSchemaValue?
  stdout : @async-core.Stream[Byte]?
}

///|
/// Empty custom-error type for tools whose command surface declares no typed
/// custom errors.
pub(all) enum NoToolError {}

///|
pub fn encode_typed_input(
  input : @model.TypedSchemaValue,
) -> Result[@types.TypedSchemaValue, String] {
  try @model.typed_schema_value_to_wit(input) catch {
    error => Err("failed to encode tool input: \{error}")
  } noraise {
    encoded => Ok(encoded)
  }
}

///|
pub fn decode_typed_value(
  value : @types.TypedSchemaValue,
  context : String,
) -> Result[@model.TypedSchemaValue, String] {
  try @model_host.typed_schema_value_from_wit(value) catch {
    error => Err("failed to decode \{context}: \{error}")
  } noraise {
    decoded => Ok(decoded)
  }
}

///|
pub fn decode_invocation_result(
  result : @toolCommon.InvocationResult,
) -> Result[InvocationResult, String] {
  let decoded = match result.result {
    Some(value) =>
      match decode_typed_value(value, "tool result") {
        Ok(decoded) => Some(decoded)
        Err(error) => {
          match result.stdout {
            Some(stdout) => stdout.drop_sync()
            None => ()
          }
          return Err(error)
        }
      }
    None => None
  }
  Ok({ result: decoded, stdout: result.stdout })
}

///|
/// Releases every still-owned capability and stdout stream in a decoded tool
/// result. The operation is safe to repeat because schema capability holders
/// and streams share take/drop state across copies.
#doc(hidden)
pub fn drop_invocation_result(result : InvocationResult) -> Unit {
  match result.result {
    Some(value) => @model.drop_owned_capabilities(value.value)
    None => ()
  }
  match result.stdout {
    Some(stdout) => stdout.drop_sync()
    None => ()
  }
}

///|
pub fn[T : @schema.FromSchema] decode_custom_error(
  value : @model.TypedSchemaValue,
) -> Result[T, String] {
  try @schema.from_value_as(value.value) catch {
    error => {
      @model.drop_owned_capabilities(value.value)
      Err("failed to decode remote tool error: \{error}")
    }
  } noraise {
    decoded => Ok(decoded)
  }
}

///|
pub fn[T : @schema.FromSchema] project_result_with_stdout(
  result : InvocationResult,
) -> Result[(T, @async-core.Stream[Byte]), String] {
  let stdout = expect_stdout(result.stdout)
  let value = decode_expected_value(result.result)
  match (stdout, value) {
    (Ok(stdout), Ok(value)) => Ok((value, stdout))
    (Err(error), _) => {
      drop_invocation_result(result)
      Err(error)
    }
    (_, Err(error)) => {
      drop_invocation_result(result)
      Err(error)
    }
  }
}

///|
pub fn[T : @schema.FromSchema] project_result_value(
  result : InvocationResult,
) -> Result[T, String] {
  match expect_no_stdout(result.stdout) {
    Err(error) => {
      drop_invocation_result(result)
      Err(error)
    }
    Ok(_) =>
      match decode_expected_value(result.result) {
        Ok(value) => Ok(value)
        Err(error) => {
          drop_invocation_result(result)
          Err(error)
        }
      }
  }
}

///|
pub fn project_result_stdout_only(
  result : InvocationResult,
) -> Result[@async-core.Stream[Byte], String] {
  let stdout = expect_stdout(result.stdout)
  match (stdout, expect_no_value(result.result)) {
    (Ok(stdout), Ok(_)) => Ok(stdout)
    (Err(error), _) => {
      drop_invocation_result(result)
      Err(error)
    }
    (_, Err(error)) => {
      drop_invocation_result(result)
      Err(error)
    }
  }
}

///|
pub fn project_result_empty(result : InvocationResult) -> Result[Unit, String] {
  match expect_no_stdout(result.stdout) {
    Err(error) => {
      drop_invocation_result(result)
      Err(error)
    }
    Ok(_) =>
      match expect_no_value(result.result) {
        Ok(_) => Ok(())
        Err(error) => {
          drop_invocation_result(result)
          Err(error)
        }
      }
  }
}

///|
fn expect_stdout(
  stdout : @async-core.Stream[Byte]?,
) -> Result[@async-core.Stream[Byte], String] {
  match stdout {
    Some(stdout) => Ok(stdout)
    None => Err("tool result did not contain declared stdout stream")
  }
}

///|
fn expect_no_stdout(stdout : @async-core.Stream[Byte]?) -> Result[Unit, String] {
  match stdout {
    Some(_) => Err("tool result unexpectedly contained stdout stream")
    None => Ok(())
  }
}

///|
fn expect_value(
  value : @model.TypedSchemaValue?,
) -> Result[@model.TypedSchemaValue, String] {
  match value {
    Some(value) => Ok(value)
    None => Err("tool result did not contain a value")
  }
}

///|
fn expect_no_value(value : @model.TypedSchemaValue?) -> Result[Unit, String] {
  match value {
    Some(_) => Err("tool result unexpectedly contained a value")
    None => Ok(())
  }
}

///|
fn[T : @schema.FromSchema] decode_expected_value(
  value : @model.TypedSchemaValue?,
) -> Result[T, String] {
  let value = match expect_value(value) {
    Ok(value) => value
    Err(error) => return Err(error)
  }
  try @schema.from_value_as(value.value) catch {
    error => Err("failed to decode tool result: \{error}")
  } noraise {
    decoded => Ok(decoded)
  }
}
