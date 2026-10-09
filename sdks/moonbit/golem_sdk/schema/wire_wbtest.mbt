///|
test "native preflight detects distinct holders of the same reader at both boundaries" {
  for at in [1, 49] {
    let encoder = WireEncoder::new()
    let (reader, _) : (
      @async-core.Stream[@types.SchemaValueTree],
      @async-core.Sink[@types.SchemaValueTree],
    ) = @async-core.Stream::new()
    let original = @model.GuestSchemaValueStreamHandle::from_native(reader)
    let duplicate = @model.GuestSchemaValueStreamHandle::from_native(reader)
    encoder.streams.push((0, original))
    for i in 1..<50 {
      let (other, _) : (
        @async-core.Stream[@types.SchemaValueTree],
        @async-core.Sink[@types.SchemaValueTree],
      ) = @async-core.Stream::new()
      encoder.streams.push(
        (
          i,
          if i == at {
            duplicate
          } else {
            @model.GuestSchemaValueStreamHandle::from_native(other)
          },
        ),
      )
    }
    try encoder.preflight() catch {
      WireError::Invalid("owned stream appears more than once") => ()
      error => fail(repr(error))
    } noraise {
      _ => fail("accepted aliased native reader")
    }
    assert_true(encoder.streams.all(entry => entry.1.is_present()))
  }
}
