///|
pub fn resource_drops() -> @model.SchemaValueResourceDrops {
  {
    drop_secret: fn(value) { value.drop() },
    drop_quota_token: fn(value) { value.drop() },
    drop_stream: fn(value) { value.drop() },
    drop_native_stream: fn(reader) { reader.drop_sync() },
  }
}

///|
pub fn schema_value_from_wit(
  wit : @types.SchemaValueTree,
) -> @model.SchemaValue raise @model.SchemaModelError {
  @model.schema_value_from_wit(wit, resource_drops())
}

///|
pub fn typed_schema_value_from_wit(
  wit : @types.TypedSchemaValue,
) -> @model.TypedSchemaValue raise @model.SchemaModelError {
  @model.typed_schema_value_from_wit(wit, resource_drops())
}
