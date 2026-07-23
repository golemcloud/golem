// `IntoSchema` / `FromSchema` impls for MoonBit tuples (arities 2..8), mirroring
// the Rust SDK's `impl_tuple!` expansions. Tuples are structural: `register_in`
// builds a `Tuple` schema inline and `type_id` is `core.tuple.TupleN<args...>`.

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema] IntoSchema for (T0, T1) with fn type_id() {
  type_id_with_args("core.tuple.Tuple2", [T0::type_id(), T1::type_id()])
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema] IntoSchema for (T0, T1) with fn register_in(
  b,
) {
  @model.schema_type(Tuple([T0::register_in(b), T1::register_in(b)]))
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema] IntoSchema for (T0, T1) with fn to_value(
  self,
) {
  Tuple([T0::to_value(self.0), T1::to_value(self.1)])
}

///|
pub impl[T0 : FromSchema, T1 : FromSchema] FromSchema for (T0, T1) with fn from_value(
  v,
) {
  match v {
    Tuple(elems) => {
      if elems.length() != 2 {
        raise FromSchemaError::custom(
          "tuple length mismatch: expected 2, got " + elems.length().to_string(),
        )
      }
      (T0::from_value(elems[0]), T1::from_value(elems[1]))
    }
    other =>
      raise FromSchemaError::shape_mismatch("tuple", value_kind(other), "tuple")
  }
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema, T2 : IntoSchema] IntoSchema for (
  T0,
  T1,
  T2,
) with fn type_id() {
  type_id_with_args("core.tuple.Tuple3", [
    T0::type_id(),
    T1::type_id(),
    T2::type_id(),
  ])
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema, T2 : IntoSchema] IntoSchema for (
  T0,
  T1,
  T2,
) with fn register_in(b) {
  @model.schema_type(
    Tuple([T0::register_in(b), T1::register_in(b), T2::register_in(b)]),
  )
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema, T2 : IntoSchema] IntoSchema for (
  T0,
  T1,
  T2,
) with fn to_value(self) {
  Tuple([T0::to_value(self.0), T1::to_value(self.1), T2::to_value(self.2)])
}

///|
pub impl[T0 : FromSchema, T1 : FromSchema, T2 : FromSchema] FromSchema for (
  T0,
  T1,
  T2,
) with fn from_value(v) {
  match v {
    Tuple(elems) => {
      if elems.length() != 3 {
        raise FromSchemaError::custom(
          "tuple length mismatch: expected 3, got " + elems.length().to_string(),
        )
      }
      (
        T0::from_value(elems[0]),
        T1::from_value(elems[1]),
        T2::from_value(elems[2]),
      )
    }
    other =>
      raise FromSchemaError::shape_mismatch("tuple", value_kind(other), "tuple")
  }
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema, T2 : IntoSchema, T3 : IntoSchema] IntoSchema for (
  T0,
  T1,
  T2,
  T3,
) with fn type_id() {
  type_id_with_args("core.tuple.Tuple4", [
    T0::type_id(),
    T1::type_id(),
    T2::type_id(),
    T3::type_id(),
  ])
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema, T2 : IntoSchema, T3 : IntoSchema] IntoSchema for (
  T0,
  T1,
  T2,
  T3,
) with fn register_in(b) {
  @model.schema_type(
    Tuple([
      T0::register_in(b),
      T1::register_in(b),
      T2::register_in(b),
      T3::register_in(b),
    ]),
  )
}

///|
pub impl[T0 : IntoSchema, T1 : IntoSchema, T2 : IntoSchema, T3 : IntoSchema] IntoSchema for (
  T0,
  T1,
  T2,
  T3,
) with fn to_value(self) {
  Tuple([
    T0::to_value(self.0),
    T1::to_value(self.1),
    T2::to_value(self.2),
    T3::to_value(self.3),
  ])
}

///|
pub impl[T0 : FromSchema, T1 : FromSchema, T2 : FromSchema, T3 : FromSchema] FromSchema for (
  T0,
  T1,
  T2,
  T3,
) with fn from_value(v) {
  match v {
    Tuple(elems) => {
      if elems.length() != 4 {
        raise FromSchemaError::custom(
          "tuple length mismatch: expected 4, got " + elems.length().to_string(),
        )
      }
      (
        T0::from_value(elems[0]),
        T1::from_value(elems[1]),
        T2::from_value(elems[2]),
        T3::from_value(elems[3]),
      )
    }
    other =>
      raise FromSchemaError::shape_mismatch("tuple", value_kind(other), "tuple")
  }
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4) with fn type_id() {
  type_id_with_args("core.tuple.Tuple5", [
    T0::type_id(),
    T1::type_id(),
    T2::type_id(),
    T3::type_id(),
    T4::type_id(),
  ])
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4) with fn register_in(b) {
  @model.schema_type(
    Tuple([
      T0::register_in(b),
      T1::register_in(b),
      T2::register_in(b),
      T3::register_in(b),
      T4::register_in(b),
    ]),
  )
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4) with fn to_value(self) {
  Tuple([
    T0::to_value(self.0),
    T1::to_value(self.1),
    T2::to_value(self.2),
    T3::to_value(self.3),
    T4::to_value(self.4),
  ])
}

///|
pub impl[
  T0 : FromSchema,
  T1 : FromSchema,
  T2 : FromSchema,
  T3 : FromSchema,
  T4 : FromSchema,
] FromSchema for (T0, T1, T2, T3, T4) with fn from_value(v) {
  match v {
    Tuple(elems) => {
      if elems.length() != 5 {
        raise FromSchemaError::custom(
          "tuple length mismatch: expected 5, got " + elems.length().to_string(),
        )
      }
      (
        T0::from_value(elems[0]),
        T1::from_value(elems[1]),
        T2::from_value(elems[2]),
        T3::from_value(elems[3]),
        T4::from_value(elems[4]),
      )
    }
    other =>
      raise FromSchemaError::shape_mismatch("tuple", value_kind(other), "tuple")
  }
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5) with fn type_id() {
  type_id_with_args("core.tuple.Tuple6", [
    T0::type_id(),
    T1::type_id(),
    T2::type_id(),
    T3::type_id(),
    T4::type_id(),
    T5::type_id(),
  ])
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5) with fn register_in(b) {
  @model.schema_type(
    Tuple([
      T0::register_in(b),
      T1::register_in(b),
      T2::register_in(b),
      T3::register_in(b),
      T4::register_in(b),
      T5::register_in(b),
    ]),
  )
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5) with fn to_value(self) {
  Tuple([
    T0::to_value(self.0),
    T1::to_value(self.1),
    T2::to_value(self.2),
    T3::to_value(self.3),
    T4::to_value(self.4),
    T5::to_value(self.5),
  ])
}

///|
pub impl[
  T0 : FromSchema,
  T1 : FromSchema,
  T2 : FromSchema,
  T3 : FromSchema,
  T4 : FromSchema,
  T5 : FromSchema,
] FromSchema for (T0, T1, T2, T3, T4, T5) with fn from_value(v) {
  match v {
    Tuple(elems) => {
      if elems.length() != 6 {
        raise FromSchemaError::custom(
          "tuple length mismatch: expected 6, got " + elems.length().to_string(),
        )
      }
      (
        T0::from_value(elems[0]),
        T1::from_value(elems[1]),
        T2::from_value(elems[2]),
        T3::from_value(elems[3]),
        T4::from_value(elems[4]),
        T5::from_value(elems[5]),
      )
    }
    other =>
      raise FromSchemaError::shape_mismatch("tuple", value_kind(other), "tuple")
  }
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
  T6 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5, T6) with fn type_id() {
  type_id_with_args("core.tuple.Tuple7", [
    T0::type_id(),
    T1::type_id(),
    T2::type_id(),
    T3::type_id(),
    T4::type_id(),
    T5::type_id(),
    T6::type_id(),
  ])
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
  T6 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5, T6) with fn register_in(b) {
  @model.schema_type(
    Tuple([
      T0::register_in(b),
      T1::register_in(b),
      T2::register_in(b),
      T3::register_in(b),
      T4::register_in(b),
      T5::register_in(b),
      T6::register_in(b),
    ]),
  )
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
  T6 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5, T6) with fn to_value(self) {
  Tuple([
    T0::to_value(self.0),
    T1::to_value(self.1),
    T2::to_value(self.2),
    T3::to_value(self.3),
    T4::to_value(self.4),
    T5::to_value(self.5),
    T6::to_value(self.6),
  ])
}

///|
pub impl[
  T0 : FromSchema,
  T1 : FromSchema,
  T2 : FromSchema,
  T3 : FromSchema,
  T4 : FromSchema,
  T5 : FromSchema,
  T6 : FromSchema,
] FromSchema for (T0, T1, T2, T3, T4, T5, T6) with fn from_value(v) {
  match v {
    Tuple(elems) => {
      if elems.length() != 7 {
        raise FromSchemaError::custom(
          "tuple length mismatch: expected 7, got " + elems.length().to_string(),
        )
      }
      (
        T0::from_value(elems[0]),
        T1::from_value(elems[1]),
        T2::from_value(elems[2]),
        T3::from_value(elems[3]),
        T4::from_value(elems[4]),
        T5::from_value(elems[5]),
        T6::from_value(elems[6]),
      )
    }
    other =>
      raise FromSchemaError::shape_mismatch("tuple", value_kind(other), "tuple")
  }
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
  T6 : IntoSchema,
  T7 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5, T6, T7) with fn type_id() {
  type_id_with_args("core.tuple.Tuple8", [
    T0::type_id(),
    T1::type_id(),
    T2::type_id(),
    T3::type_id(),
    T4::type_id(),
    T5::type_id(),
    T6::type_id(),
    T7::type_id(),
  ])
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
  T6 : IntoSchema,
  T7 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5, T6, T7) with fn register_in(b) {
  @model.schema_type(
    Tuple([
      T0::register_in(b),
      T1::register_in(b),
      T2::register_in(b),
      T3::register_in(b),
      T4::register_in(b),
      T5::register_in(b),
      T6::register_in(b),
      T7::register_in(b),
    ]),
  )
}

///|
pub impl[
  T0 : IntoSchema,
  T1 : IntoSchema,
  T2 : IntoSchema,
  T3 : IntoSchema,
  T4 : IntoSchema,
  T5 : IntoSchema,
  T6 : IntoSchema,
  T7 : IntoSchema,
] IntoSchema for (T0, T1, T2, T3, T4, T5, T6, T7) with fn to_value(self) {
  Tuple([
    T0::to_value(self.0),
    T1::to_value(self.1),
    T2::to_value(self.2),
    T3::to_value(self.3),
    T4::to_value(self.4),
    T5::to_value(self.5),
    T6::to_value(self.6),
    T7::to_value(self.7),
  ])
}

///|
pub impl[
  T0 : FromSchema,
  T1 : FromSchema,
  T2 : FromSchema,
  T3 : FromSchema,
  T4 : FromSchema,
  T5 : FromSchema,
  T6 : FromSchema,
  T7 : FromSchema,
] FromSchema for (T0, T1, T2, T3, T4, T5, T6, T7) with fn from_value(v) {
  match v {
    Tuple(elems) => {
      if elems.length() != 8 {
        raise FromSchemaError::custom(
          "tuple length mismatch: expected 8, got " + elems.length().to_string(),
        )
      }
      (
        T0::from_value(elems[0]),
        T1::from_value(elems[1]),
        T2::from_value(elems[2]),
        T3::from_value(elems[3]),
        T4::from_value(elems[4]),
        T5::from_value(elems[5]),
        T6::from_value(elems[6]),
        T7::from_value(elems[7]),
      )
    }
    other =>
      raise FromSchemaError::shape_mismatch("tuple", value_kind(other), "tuple")
  }
}
