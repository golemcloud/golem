// Schema and wire support, so that a `PermissionCard` or a
// `PolymorphicPermissionCard` can be an agent method parameter or return value
// and a field of a derived value type. Both share the schema value and wire
// node of a permission card; only the schema's `polymorphic` flag differs.

///|
/// A card is a first-class capability node in the schema model
/// (`SchemaType::permission-card`). The schema is never polymorphic: a
/// `PermissionCard` declares a concrete card. The host does not check the kind
/// of a received card.
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
/// decoding rejects a handle that a card already holds. A holder that a
/// `PolymorphicPermissionCard` shares does not decode into a `PermissionCard`.
pub impl @schema.FromSchema for PermissionCard with fn from_value(value) {
  match value {
    PermissionCard(handle) => {
      claim_card(handle, false, "PermissionCard")
      { handle, }
    }
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
  let handle = @schema.FromWire::read_wire(reader, index)
  ignore(@card.claim_kind(handle, false))
  { handle, }
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

///|
/// A template card is a capability node whose schema is polymorphic.
pub impl @schema.IntoSchema for PolymorphicPermissionCard with fn type_id() {
  "golem.core.PolymorphicPermissionCard"
}

///|
pub impl @schema.IntoSchema for PolymorphicPermissionCard with fn register_in(
  _builder,
) {
  @model.schema_type(PermissionCard({ polymorphic: true, }))
}

///|
/// Shares the take-once holder with the schema value, like `PermissionCard`.
pub impl @schema.IntoSchema for PolymorphicPermissionCard with fn to_value(self) {
  PermissionCard(self.handle)
}

///|
/// A `SchemaValue::PermissionCard` becomes a template card that shares its
/// holder. A card received in a wire tree takes the kind that the decoded type
/// declares. A holder that a `PermissionCard` shares does not decode into a
/// `PolymorphicPermissionCard`.
pub impl @schema.FromSchema for PolymorphicPermissionCard with fn from_value(
  value,
) {
  match value {
    PermissionCard(handle) => {
      claim_card(handle, true, "PolymorphicPermissionCard")
      { handle, }
    }
    other =>
      raise @schema.FromSchemaError::shape_mismatch(
        "permission-card",
        @schema.value_kind(other),
        "PolymorphicPermissionCard",
      )
  }
}

///|
pub impl @schema.FromWire for PolymorphicPermissionCard with fn read_wire(
  reader,
  index,
) {
  let handle = @schema.FromWire::read_wire(reader, index)
  ignore(@card.claim_kind(handle, true))
  { handle, }
}

///|
pub impl @schema.IntoWire for PolymorphicPermissionCard with fn append_wire(
  self,
  encoder,
) {
  @schema.IntoWire::append_wire(self.handle, encoder)
}

///|
pub impl @schema.IntoWire for PolymorphicPermissionCard with fn release_wire(
  self,
  releaser,
) {
  @schema.IntoWire::release_wire(self.handle, releaser)
}

///|
pub impl @schema.WireSchema for PolymorphicPermissionCard with fn append_wire_type(
  encoder,
) {
  encoder.push(@types.PermissionCardType({ polymorphic: true, }))
}

///|
fn claim_card(
  handle : @card.GuestPermissionCardHandle,
  polymorphic : Bool,
  context : String,
) -> Unit raise @schema.FromSchemaError {
  guard @card.claim_kind(handle, polymorphic) else {
    raise @schema.FromSchemaError::shape_mismatch(
      "permission-card { polymorphic: \{polymorphic} }",
      "permission-card { polymorphic: \{!polymorphic} }",
      context,
    )
  }
}
