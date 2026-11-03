///|
/// The status of a completed command, as a shell sees it. A command that
/// returns it publishes the named record `golem.tool.ProcessExitStatus` with
/// the field `exit_code : u8`, which every SDK shares.
pub(all) struct ProcessExitStatus {
  exit_code : Byte
} derive(Debug, Eq)

///|
let process_exit_status_id : String = "golem.tool.ProcessExitStatus"

///|
pub impl @schema.IntoSchema for ProcessExitStatus with fn type_id() {
  process_exit_status_id
}

///|
pub impl @schema.IntoSchema for ProcessExitStatus with fn register_in(b) {
  b.register(
    process_exit_status_id,
    () => {
      @model.schema_type(
        Record([
          @model.field(
            "exit_code",
            @schema.register_in_with(
              b,
              (@schema.type_tag() : @schema.TypeTag[Byte]),
            ),
          ),
        ]),
      )
    },
    Some("ProcessExitStatus"),
  )
}

///|
pub impl @schema.IntoSchema for ProcessExitStatus with fn to_value(self) {
  Record([@schema.to_value_as(self.exit_code)])
}

///|
pub impl @schema.FromSchema for ProcessExitStatus with fn from_value(v) {
  let fields = @schema.expect_record(v, process_exit_status_id)
  if fields.length() != 1 {
    raise @schema.FromSchemaError::custom(
      "\{process_exit_status_id} expects 1 field, got \{fields.length()}",
    )
  }
  { exit_code: @schema.from_value_as(fields[0]), }
}

///|
pub impl @schema.IntoWire for ProcessExitStatus with fn append_wire(
  self,
  encoder,
) {
  let fields = [@schema.IntoWire::append_wire(self.exit_code, encoder)]
  encoder.push(@types.SchemaValueNode::RecordValue(fields))
}

///|
pub impl @schema.FromWire for ProcessExitStatus with fn read_wire(tree, index) {
  let fields = @schema.wire_record(tree, index, 1, process_exit_status_id)
  { exit_code: @schema.FromWire::read_wire(tree, fields[0]), }
}

///|
pub impl @schema.WireSchema for ProcessExitStatus with fn append_wire_type(
  encoder,
) {
  encoder.named(
    process_exit_status_id,
    () => {
      encoder.push(
        @types.SchemaTypeBody::RecordType([
          {
            name: "exit_code",
            body: @schema.append_wire_type_with(
              encoder,
              (@schema.type_tag() : @schema.TypeTag[Byte]),
            ),
            metadata: @schema.empty_wire_metadata(),
          },
        ]),
      )
    },
    name=Some("ProcessExitStatus"),
  )
}
