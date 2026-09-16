///|
/// The two possible origins of a recursive schema value stream. A received WIT
/// resource remains wrapped until the boundary unwraps it; a native reader is
/// retained verbatim until the boundary wraps it.
priv enum SchemaValueStreamEndpoint {
  Wrapped(@types.SchemaValueStream)
  Native(@async-core.Stream[@types.SchemaValueTree])
}

///|
/// Affine, take-once holder for a recursive schema value stream endpoint.
pub(all) struct GuestSchemaValueStreamHandle {
  priv mut cell : SchemaValueStreamEndpoint?
}

///|
pub fn GuestSchemaValueStreamHandle::from_wrapped(
  value : @types.SchemaValueStream,
) -> GuestSchemaValueStreamHandle {
  { cell: Some(Wrapped(value)), }
}

///|
pub fn GuestSchemaValueStreamHandle::from_native(
  reader : @async-core.Stream[@types.SchemaValueTree],
) -> GuestSchemaValueStreamHandle {
  { cell: Some(Native(reader)), }
}

///|
pub fn GuestSchemaValueStreamHandle::is_present(
  self : GuestSchemaValueStreamHandle,
) -> Bool {
  self.cell is Some(_)
}

///|
fn GuestSchemaValueStreamHandle::same_endpoint(
  self : GuestSchemaValueStreamHandle,
  other : GuestSchemaValueStreamHandle,
) -> Bool {
  match (self.cell, other.cell) {
    (Some(Wrapped(left)), Some(Wrapped(right))) => left == right
    (Some(Native(left)), Some(Native(right))) => physical_equal(left, right)
    _ => false
  }
}

///|
fn GuestSchemaValueStreamHandle::take(
  self : GuestSchemaValueStreamHandle,
) -> SchemaValueStreamEndpoint? {
  let value = self.cell
  self.cell = None
  value
}

///|
fn GuestSchemaValueStreamHandle::release(
  self : GuestSchemaValueStreamHandle,
  drop_wrapped : (@types.SchemaValueStream) -> Unit,
  drop_native : (@async-core.Stream[@types.SchemaValueTree]) -> Unit,
) -> Unit {
  match self.take() {
    Some(Wrapped(value)) => drop_wrapped(value)
    Some(Native(reader)) => drop_native(reader)
    None => ()
  }
}

///|
async fn GuestSchemaValueStreamHandle::prepare_outgoing(
  self : GuestSchemaValueStreamHandle,
) -> Unit {
  match self.take() {
    None =>
      raise EncodeError(
        "schema-value-stream handle was already transferred; an owned stream can only be sent once",
      )
    Some(Wrapped(value)) => self.cell = Some(Wrapped(value))
    Some(Native(reader)) => {
      let wrapped = @types.SchemaValueStream::wrap(reader) catch {
        @async-core.Cancelled::Cancelled as err => raise err
        e => raise EncodeError("failed wrapping schema value stream: \{e}")
      }
      self.cell = Some(Wrapped(wrapped))
    }
  }
}

///|
pub async fn GuestSchemaValueStreamHandle::read_native(
  self : GuestSchemaValueStreamHandle,
) -> FixedArray[@types.SchemaValueTree]? {
  let reader = match self.take() {
    None =>
      raise SchemaValueStreamReadError::Ownership(
        "schema-value-stream handle was already transferred; an owned stream can only be received once",
      )
    Some(Native(reader)) => reader
    Some(Wrapped(value)) =>
      @types.SchemaValueStream::unwrap(value) catch {
        @async-core.Cancelled::Cancelled as err => raise err
        e =>
          raise SchemaValueStreamReadError::Transport(
            "failed unwrapping schema value stream: \{repr(e)}",
          )
      }
  }
  let result = {
    errdefer {
      self.cell = Some(Native(reader))
    }
    reader.read(1) catch {
      @async-core.Cancelled::Cancelled as err => raise err
      e =>
        raise SchemaValueStreamReadError::Transport(
          "failed reading schema value stream: \{repr(e)}",
        )
    }
  }
  self.cell = Some(Native(reader))
  result
}

///|
impl Eq for GuestSchemaValueStreamHandle with fn equal(self, other) {
  physical_equal(self, other)
}

///|
impl Debug for GuestSchemaValueStreamHandle with fn to_repr(self) {
  Repr::literal(
    if self.is_present() {
      "GuestSchemaValueStreamHandle(present)"
    } else {
      "GuestSchemaValueStreamHandle(consumed)"
    },
  )
}
