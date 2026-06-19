// Decode helpers for generated record / enum / variant `FromSchema` impls.
//
// The encode + `register_in` side of generated code constructs schema/value
// nodes directly with the `schema_model` constructors (`schema_type`, `field`,
// `variant_case`, `Record`, `Variant`, `Enum`, ...). The decode side benefits
// from these shared helpers so the generated `from_value` stays small and every
// failure produces a uniform, precise `FromSchemaError`. Mirrors the shape of
// the Rust derive's generated decoders.

///|
/// Expect a record value, returning its positional field values.
pub fn expect_record(
  v : @model.SchemaValue,
  context : String,
) -> Array[@model.SchemaValue] raise FromSchemaError {
  match v {
    Record(fields) => fields
    other =>
      raise FromSchemaError::shape_mismatch(
        "record",
        value_kind(other),
        context,
      )
  }
}

///|
/// Read a record field by position with a bounds check.
pub fn record_field(
  fields : Array[@model.SchemaValue],
  pos : Int,
  context : String,
) -> @model.SchemaValue raise FromSchemaError {
  if pos < 0 || pos >= fields.length() {
    raise FromSchemaError::custom(
      "record field index \{pos} out of bounds in \{context}",
    )
  }
  fields[pos]
}

///|
/// Expect a simple-enum value, returning its case index.
pub fn expect_enum(
  v : @model.SchemaValue,
  context : String,
) -> UInt raise FromSchemaError {
  match v {
    Enum(case) => case
    other =>
      raise FromSchemaError::shape_mismatch("enum", value_kind(other), context)
  }
}

///|
/// Expect a variant value, returning its case index and optional payload.
pub fn expect_variant(
  v : @model.SchemaValue,
  context : String,
) -> (UInt, @model.SchemaValue?) raise FromSchemaError {
  match v {
    Variant(case, payload) => (case, payload)
    other =>
      raise FromSchemaError::shape_mismatch(
        "variant",
        value_kind(other),
        context,
      )
  }
}

///|
/// Raise `OutOfRange` if a decoded case index exceeds the known case count.
pub fn check_case_index(
  case : UInt,
  count : UInt,
  context : String,
) -> Unit raise FromSchemaError {
  if case >= count {
    raise FromSchemaError::out_of_range(case, count, context)
  }
}

///|
/// Unwrap a variant payload that the case schema declared as present.
pub fn expect_variant_payload(
  payload : @model.SchemaValue?,
  context : String,
) -> @model.SchemaValue raise FromSchemaError {
  match payload {
    Some(p) => p
    None =>
      raise FromSchemaError::custom(
        "variant case in \{context} expected a payload but found none",
      )
  }
}

///|
/// Assert a unit variant case carries no payload, mirroring the Rust derive's
/// generated decoder (it rejects a payload on a payload-less case).
pub fn expect_no_variant_payload(
  payload : @model.SchemaValue?,
  context : String,
) -> Unit raise FromSchemaError {
  match payload {
    None => ()
    Some(_) =>
      raise FromSchemaError::custom(
        "variant case in \{context} expected no payload but found one",
      )
  }
}
