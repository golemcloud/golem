///|
/// A demand-driven agent stream. The native component reader is retained in
/// the schema value and transferred affinely; no producer task or buffer is
/// introduced by this wrapper. Encoding an unread stream as an invocation
/// input or result transfers its original endpoint exactly once. A partially
/// read stream cannot be forwarded directly and must instead be consumed or
/// dropped.
#warnings("-unused_type_variable")
pub(all) struct AgentStream[T] {
  priv handle : @model.GuestSchemaValueStreamHandle
}

///|
/// The writable endpoint of a typed agent stream.
#warnings("-unused_type_variable")
pub struct AgentStreamWriter[T] {
  priv sink : @async-core.Sink[@types.SchemaValueTree]
}

///|
/// Result of transferring one or more values to an agent stream.
pub(all) enum AgentStreamWriteOutcome {
  /// The transport accepted every value in the write.
  Accepted
  /// The readable endpoint was dropped before every value was accepted.
  PeerDropped
} derive(Debug, Eq)

///|
/// Failures specific to consuming a typed agent stream. Canonical operation
/// cancellation is propagated unchanged rather than wrapped in this error.
pub suberror AgentStreamReadError {
  /// The affine readable endpoint was already transferred or dropped.
  Ownership(String)
  /// The component-model endpoint could not be acquired or read.
  Transport(String)
  /// The transport returned a malformed schema-value carrier.
  WireDecode(@model.SchemaModelError)
  /// A valid schema value could not be decoded as the declared item type.
  ItemDecode(FromSchemaError)
} derive(Debug, Eq)

///|
fn[T] AgentStream::from_reader(
  reader : @async-core.Stream[@types.SchemaValueTree],
) -> AgentStream[T] {
  { handle: @model.GuestSchemaValueStreamHandle::from_native(reader), }
}

///|
/// Creates an explicit writer and readable stream endpoint. The writer owns
/// every value passed to it, including values rejected after the reader drops.
pub fn[T] AgentStream::new() -> (AgentStreamWriter[T], AgentStream[T]) {
  let (reader, sink) = @async-core.Stream::new_with_cleanup(
    @model.drop_wit_capabilities,
  )
  ({ sink, }, AgentStream::from_reader(reader))
}

///|
/// Creates a lazy typed stream producer. A normal producer return closes the
/// stream. Producer failure and cancellation keep their operation-level
/// terminal behavior and are not converted to clean EOF. When the producer
/// captures explicitly owned capabilities, `on_unstarted_drop` must release
/// them if the readable endpoint is dropped before the producer first runs.
pub fn[T] AgentStream::produce(
  producer : async (AgentStreamWriter[T]) -> Unit,
  on_unstarted_drop? : () -> Unit,
) -> AgentStream[T] {
  let reader = @async-core.Stream::produce(
    async fn(sink) {
      let writer : AgentStreamWriter[T] = { sink, }
      producer(writer)
      writer.close()
    },
    cleanup=@model.drop_wit_capabilities,
    on_unstarted_drop?,
  )
  AgentStream::from_reader(reader)
}

///|
async fn[T : IntoSchema] encode_stream_items(
  items : Array[T],
) -> Array[@types.SchemaValueTree] {
  let drops = @model_host.resource_drops()
  let values = items.map(to_value_as)
  let encoded : Array[@types.SchemaValueTree] = []
  errdefer {
    for value in encoded {
      @model.drop_wit_capabilities(value)
    }
    for value in values {
      value.release_resources(drops)
    }
  }
  for value in values {
    encoded.push(@model.schema_value_to_wit_async(value))
  }
  encoded
}

///|
/// Writes one value and waits until the transport accepts it or the reader has
/// dropped. In both cases ownership of `value` is consumed.
pub async fn[T : IntoSchema] AgentStreamWriter::write_one(
  self : AgentStreamWriter[T],
  value : T,
) -> AgentStreamWriteOutcome {
  self.write_all([value])
}

///|
/// Writes all values in order. On `PeerDropped`, failure, or cancellation,
/// every value not accepted by the peer is recursively released exactly once;
/// callers must not retry a suffix.
pub async fn[T : IntoSchema] AgentStreamWriter::write_all(
  self : AgentStreamWriter[T],
  values : Array[T],
) -> AgentStreamWriteOutcome {
  let encoded = encode_stream_items(values)
  if self.sink.write_all(encoded[:]) {
    Accepted
  } else {
    PeerDropped
  }
}

///|
/// Closes the writable endpoint. Repeated closes have no effect.
pub async fn[T] AgentStreamWriter::close(self : AgentStreamWriter[T]) -> Unit {
  self.sink.close()
}

///|
/// Closes this stream without reading any remaining items and recursively
/// releases their owned capabilities. Dropping an already consumed or closed
/// stream has no effect. A producer observes the drop cooperatively when a
/// subsequent write returns `PeerDropped`.
pub fn[T] AgentStream::drop(self : AgentStream[T]) -> Unit {
  @model.SchemaValue::Stream(self.handle).release_resources(
    @model_host.resource_drops(),
  )
}

///|
/// Reads one item, returning `None` only after clean EOF. Ownership, transport,
/// wire-decode, and item-decode failures remain distinguishable through
/// `AgentStreamReadError`; canonical operation cancellation is propagated
/// unchanged. Use `AgentStream[Result[T, E]]` when individual items need
/// recoverable application errors.
pub async fn[T : FromSchema] AgentStream::read(self : AgentStream[T]) -> T? {
  for ;; {
    let chunk = self.handle.read_native() catch {
      @async-core.Cancelled::Cancelled as err => raise err
      @model.SchemaValueStreamReadError::Ownership(message) =>
        raise AgentStreamReadError::Ownership(message)
      @model.SchemaValueStreamReadError::Transport(message) =>
        raise AgentStreamReadError::Transport(message)
      error =>
        raise AgentStreamReadError::Transport(
          "unexpected schema value stream failure: \{repr(error)}",
        )
    }
    match chunk {
      None => {
        self.drop()
        return None
      }
      Some(values) if values.length() == 0 => continue
      Some(values) => {
        let drops = @model_host.resource_drops()
        let value = @model_host.schema_value_from_wit(values[0]) catch {
          e => raise AgentStreamReadError::WireDecode(e)
        }
        errdefer value.release_resources(drops)
        return Some(
          from_value_as(value) catch {
            e => raise AgentStreamReadError::ItemDecode(e)
          },
        )
      }
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
