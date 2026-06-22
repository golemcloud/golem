// `IntoSchema` / `FromSchema` impls for MoonBit primitive types, mirroring the
// Rust SDK's `impl_primitive!` macro expansions in `conversion.rs`.
//
// Primitives are structural (inline): `register_in` returns the leaf
// `SchemaType` directly, never reserving a named definition. `type_id` is kept
// aligned with the Rust SDK's identifiers so a generic nominal type that closes
// over a primitive produces the same cross-SDK definition id.

// ============================================================
// String
// ============================================================

///|
pub impl IntoSchema for String with type_id() {
  "alloc.string.String"
}

///|
pub impl IntoSchema for String with register_in(_b) {
  @model.schema_type(String)
}

///|
pub impl IntoSchema for String with to_value(self) {
  String(self)
}

///|
pub impl FromSchema for String with from_value(v) {
  match v {
    String(s) => s
    other =>
      raise FromSchemaError::shape_mismatch(
        "string",
        value_kind(other),
        "String",
      )
  }
}

// ============================================================
// Bool
// ============================================================

///|
pub impl IntoSchema for Bool with type_id() {
  "bool"
}

///|
pub impl IntoSchema for Bool with register_in(_b) {
  @model.schema_type(Bool)
}

///|
pub impl IntoSchema for Bool with to_value(self) {
  Bool(self)
}

///|
pub impl FromSchema for Bool with from_value(v) {
  match v {
    Bool(b) => b
    other =>
      raise FromSchemaError::shape_mismatch("bool", value_kind(other), "Bool")
  }
}

// ============================================================
// Int -> s32
// ============================================================

///|
pub impl IntoSchema for Int with type_id() {
  "i32"
}

///|
pub impl IntoSchema for Int with register_in(_b) {
  @model.schema_type(S32)
}

///|
pub impl IntoSchema for Int with to_value(self) {
  S32(self)
}

///|
pub impl FromSchema for Int with from_value(v) {
  match v {
    S32(x) => x
    other =>
      raise FromSchemaError::shape_mismatch("s32", value_kind(other), "Int")
  }
}

// ============================================================
// UInt -> u32
// ============================================================

///|
pub impl IntoSchema for UInt with type_id() {
  "u32"
}

///|
pub impl IntoSchema for UInt with register_in(_b) {
  @model.schema_type(U32)
}

///|
pub impl IntoSchema for UInt with to_value(self) {
  U32(self)
}

///|
pub impl FromSchema for UInt with from_value(v) {
  match v {
    U32(x) => x
    other =>
      raise FromSchemaError::shape_mismatch("u32", value_kind(other), "UInt")
  }
}

// ============================================================
// Int64 -> s64
// ============================================================

///|
pub impl IntoSchema for Int64 with type_id() {
  "i64"
}

///|
pub impl IntoSchema for Int64 with register_in(_b) {
  @model.schema_type(S64)
}

///|
pub impl IntoSchema for Int64 with to_value(self) {
  S64(self)
}

///|
pub impl FromSchema for Int64 with from_value(v) {
  match v {
    S64(x) => x
    other =>
      raise FromSchemaError::shape_mismatch("s64", value_kind(other), "Int64")
  }
}

// ============================================================
// UInt64 -> u64
// ============================================================

///|
pub impl IntoSchema for UInt64 with type_id() {
  "u64"
}

///|
pub impl IntoSchema for UInt64 with register_in(_b) {
  @model.schema_type(U64)
}

///|
pub impl IntoSchema for UInt64 with to_value(self) {
  U64(self)
}

///|
pub impl FromSchema for UInt64 with from_value(v) {
  match v {
    U64(x) => x
    other =>
      raise FromSchemaError::shape_mismatch("u64", value_kind(other), "UInt64")
  }
}

// ============================================================
// Float -> f32
// ============================================================

///|
pub impl IntoSchema for Float with type_id() {
  "f32"
}

///|
pub impl IntoSchema for Float with register_in(_b) {
  @model.schema_type(F32)
}

///|
pub impl IntoSchema for Float with to_value(self) {
  F32(self)
}

///|
pub impl FromSchema for Float with from_value(v) {
  match v {
    F32(x) => x
    other =>
      raise FromSchemaError::shape_mismatch("f32", value_kind(other), "Float")
  }
}

// ============================================================
// Double -> f64
// ============================================================

///|
pub impl IntoSchema for Double with type_id() {
  "f64"
}

///|
pub impl IntoSchema for Double with register_in(_b) {
  @model.schema_type(F64)
}

///|
pub impl IntoSchema for Double with to_value(self) {
  F64(self)
}

///|
pub impl FromSchema for Double with from_value(v) {
  match v {
    F64(x) => x
    other =>
      raise FromSchemaError::shape_mismatch("f64", value_kind(other), "Double")
  }
}

// ============================================================
// Byte -> u8
// ============================================================

///|
pub impl IntoSchema for Byte with type_id() {
  "u8"
}

///|
pub impl IntoSchema for Byte with register_in(_b) {
  @model.schema_type(U8)
}

///|
pub impl IntoSchema for Byte with to_value(self) {
  U8(self)
}

///|
pub impl FromSchema for Byte with from_value(v) {
  match v {
    U8(x) => x
    other =>
      raise FromSchemaError::shape_mismatch("u8", value_kind(other), "Byte")
  }
}

// ============================================================
// Char
// ============================================================

///|
pub impl IntoSchema for Char with type_id() {
  "char"
}

///|
pub impl IntoSchema for Char with register_in(_b) {
  @model.schema_type(Char)
}

///|
pub impl IntoSchema for Char with to_value(self) {
  Char(self)
}

///|
pub impl FromSchema for Char with from_value(v) {
  match v {
    Char(c) => c
    other =>
      raise FromSchemaError::shape_mismatch("char", value_kind(other), "Char")
  }
}

// ============================================================
// Bytes -> binary (mirrors Rust `bytes::Bytes` -> SchemaType::binary)
// ============================================================

///|
pub impl IntoSchema for Bytes with type_id() {
  "bytes.Bytes"
}

///|
pub impl IntoSchema for Bytes with register_in(_b) {
  @model.schema_type(
    Binary({ mime_types: None, min_bytes: None, max_bytes: None }),
  )
}

///|
pub impl IntoSchema for Bytes with to_value(self) {
  let bytes : FixedArray[Byte] = FixedArray::makei(self.length(), i => self[i])
  Binary(bytes, None)
}

///|
pub impl FromSchema for Bytes with from_value(v) {
  match v {
    Binary(bytes, _) => {
      let arr : Array[Byte] = []
      for i in 0..<bytes.length() {
        arr.push(bytes[i])
      }
      Bytes::from_array(arr)
    }
    other =>
      raise FromSchemaError::shape_mismatch(
        "binary",
        value_kind(other),
        "Bytes",
      )
  }
}
