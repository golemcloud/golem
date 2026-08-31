///|
/// A demand-driven agent stream. The native component reader is retained in
/// the schema value and transferred affinely; no producer task or buffer is
/// introduced by this wrapper.
#warnings("-unused_type_variable")
pub(all) struct AgentStream[T] {
  priv handle : @model.GuestSchemaValueStreamHandle
}

///|
pub fn[T] AgentStream::from_reader(
  reader : @async-core.Stream[@types.SchemaValueTree],
) -> AgentStream[T] {
  { handle: @model.GuestSchemaValueStreamHandle::from_native(reader), }
}

///|
/// Close this stream without reading any remaining items. Dropping an already
/// consumed or closed stream has no effect.
pub fn[T] AgentStream::drop(self : AgentStream[T]) -> Unit {
  @model.SchemaValue::Stream(self.handle).release_resources(
    @model_host.resource_drops(),
  )
}

///|
pub async fn[T : FromSchema] AgentStream::read(
  self : AgentStream[T],
) -> T? raise FromSchemaError {
  let reader = self.handle.take_or_unwrap_native() catch {
    e =>
      raise FromSchemaError::custom(
        "agent stream is no longer readable: \{repr(e)}",
      )
  }
  let chunk = {
    errdefer self.handle.put_native(reader)
    reader.read(1) catch {
      e =>
        raise FromSchemaError::custom("failed reading agent stream: \{repr(e)}")
    }
  }
  // Reassociate the exact same reader after each pull.
  self.handle.put_native(reader)
  match chunk {
    None => {
      self.drop()
      None
    }
    Some(values) if values.length() == 0 => self.read()
    Some(values) => {
      let drops = @model_host.resource_drops()
      let value = @model_host.schema_value_from_wit(values[0]) catch {
        e =>
          raise FromSchemaError::custom("invalid agent stream item: \{repr(e)}")
      }
      errdefer value.release_resources(drops)
      Some(from_value_as(value))
    }
  }
}

///|
pub impl[T : IntoSchema] IntoSchema for AgentStream[T] with fn type_id() {
  type_id_with_args("golem.AgentStream", [T::type_id()])
}

///|
pub impl[T : IntoSchema] IntoSchema for AgentStream[T] with fn register_in(b) {
  @model.schema_type(@model.SchemaTypeBody::Stream(Some(T::register_in(b))))
}

///|
pub impl[T : IntoSchema] IntoSchema for AgentStream[T] with fn to_value(self) {
  @model.SchemaValue::Stream(self.handle)
}

///|
pub impl[T] FromSchema for AgentStream[T] with fn from_value(value) {
  match value {
    @model.SchemaValue::Stream(handle) => { handle, }
    other =>
      raise FromSchemaError::shape_mismatch(
        "stream",
        value_kind(other),
        "AgentStream",
      )
  }
}
