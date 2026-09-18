///|
priv struct Transport {
  raw : @agentHost.WasmRpc
}

///|
fn Transport::create(
  type_name : String,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid?,
) -> Transport raise ReflectError {
  Transport::create_with_config(type_name, constructor_value, phantom_id, [])
}

///|
fn Transport::create_with_config(
  type_name : String,
  constructor_value : @model.SchemaValue,
  phantom_id : @types.Uuid?,
  agent_config : Array[@common.TypedAgentConfigValue],
) -> Transport raise ReflectError {
  let constructor_input = @model.schema_value_to_wit(constructor_value) catch {
    error => raise Schema("failed to encode constructor value: \{repr(error)}")
  }
  match
    @agentHost.WasmRpc::create(
      type_name, constructor_input, phantom_id, agent_config,
    ) {
    Ok(raw) => { raw, }
    Err(error) => raise rpc_error(error)
  }
}

///|
fn invocation_metadata(
  metadata : @agentHost.InvocationMetadata,
) -> InvocationMetadata {
  {
    agent_id: { value: metadata.agent_id, },
    idempotency_key: metadata.idempotency_key,
  }
}

///|
async fn Transport::invoke_value(
  self : Transport,
  method_name : String,
  input : @model.SchemaValue,
) -> Invocation[@model.SchemaValue?] {
  let wire = @model.schema_value_to_wit(input) catch {
    error => raise Schema("failed to encode invocation input: \{repr(error)}")
  }
  let pending = self.raw.async_invoke_and_await(method_name, wire, None)
  defer pending.future.drop()
  let result = {
    errdefer pending.future.cancel()
    pending.future.get()
  }
  match result {
    Ok(output) => {
      let value = match output {
        Some(output) => {
          let decoded = @model_host.schema_value_from_wit(output) catch {
            error =>
              raise Schema("failed to decode invocation output: \{repr(error)}")
          }
          Some(decoded)
        }
        None => None
      }
      { metadata: invocation_metadata(pending.metadata), value, }
    }
    Err(error) => raise rpc_error(error)
  }
}

///|
fn Transport::trigger_value(
  self : Transport,
  method_name : String,
  input : @model.SchemaValue,
) -> InvocationMetadata raise ReflectError {
  let wire = @model.schema_value_to_wit(input) catch {
    error => raise Schema("failed to encode invocation input: \{repr(error)}")
  }
  match self.raw.invoke(method_name, wire, None) {
    Ok(metadata) => invocation_metadata(metadata)
    Err(error) => raise rpc_error(error)
  }
}

///|
fn Transport::schedule_value(
  self : Transport,
  scheduled_at : @systemClock.Instant,
  method_name : String,
  input : @model.SchemaValue,
) -> ScheduledInvocation raise ReflectError {
  let wire = @model.schema_value_to_wit(input) catch {
    error => raise Schema("failed to encode invocation input: \{repr(error)}")
  }
  match
    self.raw.schedule_cancelable_invocation(
      scheduled_at,
      method_name,
      wire,
      None,
    ) {
    Ok(receipt) =>
      {
        metadata: invocation_metadata(receipt.metadata),
        cancellation_token: receipt.cancellation_token,
      }
    Err(error) => raise rpc_error(error)
  }
}

///|
fn Transport::drop(self : Transport) -> Unit {
  self.raw.drop()
}
