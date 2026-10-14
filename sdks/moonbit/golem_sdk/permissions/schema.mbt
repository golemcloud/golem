// Schema and wire support, so that a `PermissionCard` can be an agent method
// parameter or return value and a field of a derived value type.

///|
/// A card is a first-class capability node in the schema model
/// (`SchemaType::permission-card`). The schema is never polymorphic: a
/// `PermissionCard` carries a concrete card.
pub impl @schema.IntoSchema for PermissionCard with fn type_id() {
  "golem.core.PermissionCard"
}

///|
pub impl @schema.IntoSchema for PermissionCard with fn register_in(_builder) {
  @model.schema_type(PermissionCard({ polymorphic: false, }))
}

///|
/// Shares the take-once holder with the schema value. The owned resource moves
/// out of the holder exactly once, when the value is encoded into a WIT
/// `schema-value-tree`; after that this card is moved. Code outside the SDK
/// cannot take or borrow the raw handle from the shared holder.
pub impl @schema.IntoSchema for PermissionCard with fn to_value(self) {
  PermissionCard(self.handle)
}

///|
/// A `SchemaValue::PermissionCard` becomes a card that shares its holder, so
/// both values see the same take-once state. The SDK creates holders only for
/// handles that host calls return and for handles in decoded wire trees, and
/// decoding rejects a handle that a card already holds.
pub impl @schema.FromSchema for PermissionCard with fn from_value(value) {
  match value {
    PermissionCard(handle) => { handle, }
    other =>
      raise @schema.FromSchemaError::shape_mismatch(
        "permission-card",
        @schema.value_kind(other),
        "PermissionCard",
      )
  }
}

///|
pub impl @schema.FromWire for PermissionCard with fn read_wire(reader, index) {
  { handle: @schema.FromWire::read_wire(reader, index), }
}

///|
pub impl @schema.IntoWire for PermissionCard with fn append_wire(self, encoder) {
  @schema.IntoWire::append_wire(self.handle, encoder)
}

///|
pub impl @schema.IntoWire for PermissionCard with fn release_wire(
  self,
  releaser,
) {
  @schema.IntoWire::release_wire(self.handle, releaser)
}

///|
pub impl @schema.WireSchema for PermissionCard with fn append_wire_type(encoder) {
  encoder.push(@types.PermissionCardType({ polymorphic: false, }))
}
