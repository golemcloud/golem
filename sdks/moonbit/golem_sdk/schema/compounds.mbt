// `IntoSchema` / `FromSchema` impls for MoonBit compound types, mirroring the
// Rust SDK's `Option` / `Vec` / `Result` / `HashMap` / tuple impls in
// `conversion.rs`. Like primitives these are structural (inline): they never
// reserve a named definition, and their `type_id` is built from the argument
// type ids via `type_id_with_args`.

///|
/// The canonical `TypeId` of the unit type, matching the Rust SDK's `()`
/// (`core.tuple.Unit`). Used to detect unit ok/err branches of `Result`.
let unit_type_id : @model.TypeId = "core.tuple.Unit"

// ============================================================
// Unit
// ============================================================

///|
pub impl IntoSchema for Unit with type_id() {
  unit_type_id
}

///|
pub impl IntoSchema for Unit with register_in(_b) {
  @model.schema_type(Tuple([]))
}

///|
pub impl IntoSchema for Unit with to_value(_self) {
  Tuple([])
}

///|
pub impl FromSchema for Unit with from_value(v) {
  match v {
    Tuple(elems) =>
      if elems.is_empty() {
        ()
      } else {
        raise FromSchemaError::shape_mismatch("unit tuple", "tuple", "Unit")
      }
    other =>
      raise FromSchemaError::shape_mismatch(
        "unit tuple",
        value_kind(other),
        "Unit",
      )
  }
}

// ============================================================
// Option[T] -> option
// ============================================================

///|
pub impl[T : IntoSchema] IntoSchema for T? with type_id() {
  type_id_with_args("core.option.Option", [T::type_id()])
}

///|
pub impl[T : IntoSchema] IntoSchema for T? with register_in(b) {
  @model.schema_type(Option(T::register_in(b)))
}

///|
pub impl[T : IntoSchema] IntoSchema for T? with to_value(self) {
  match self {
    Some(x) => Option(Some(T::to_value(x)))
    None => Option(None)
  }
}

///|
pub impl[T : FromSchema] FromSchema for T? with from_value(v) {
  match v {
    Option(Some(x)) => Some(T::from_value(x))
    Option(None) => None
    other =>
      raise FromSchemaError::shape_mismatch(
        "option",
        value_kind(other),
        "Option",
      )
  }
}

// ============================================================
// Array[T] -> list
// ============================================================

///|
pub impl[T : IntoSchema] IntoSchema for Array[T] with type_id() {
  type_id_with_args("alloc.vec.Vec", [T::type_id()])
}

///|
pub impl[T : IntoSchema] IntoSchema for Array[T] with register_in(b) {
  @model.schema_type(List(T::register_in(b)))
}

///|
pub impl[T : IntoSchema] IntoSchema for Array[T] with to_value(self) {
  List(self.map(x => T::to_value(x)))
}

///|
pub impl[T : FromSchema] FromSchema for Array[T] with from_value(v) {
  match v {
    List(items) => {
      let result : Array[T] = []
      for item in items {
        result.push(T::from_value(item))
      }
      result
    }
    other =>
      raise FromSchemaError::shape_mismatch("list", value_kind(other), "Array")
  }
}

// ============================================================
// Result[T, E] -> result (unit ok/err omit the payload)
// ============================================================

///|
pub impl[T : IntoSchema, E : IntoSchema] IntoSchema for Result[T, E] with type_id() {
  type_id_with_args("core.result.Result", [T::type_id(), E::type_id()])
}

///|
pub impl[T : IntoSchema, E : IntoSchema] IntoSchema for Result[T, E] with register_in(
  b,
) {
  let ok = if T::type_id() == unit_type_id {
    None
  } else {
    Some(T::register_in(b))
  }
  let err = if E::type_id() == unit_type_id {
    None
  } else {
    Some(E::register_in(b))
  }
  @model.schema_type(Result(ok, err))
}

///|
pub impl[T : IntoSchema, E : IntoSchema] IntoSchema for Result[T, E] with to_value(
  self,
) {
  match self {
    Ok(x) =>
      ResultOk(
        if T::type_id() == unit_type_id {
          None
        } else {
          Some(T::to_value(x))
        },
      )
    Err(e) =>
      ResultErr(
        if E::type_id() == unit_type_id {
          None
        } else {
          Some(E::to_value(e))
        },
      )
  }
}

///|
pub impl[T : FromSchema, E : FromSchema] FromSchema for Result[T, E] with from_value(
  v,
) {
  let unit_value : @model.SchemaValue = Tuple([])
  match v {
    ResultOk(inner) =>
      Ok(
        T::from_value(
          match inner {
            Some(x) => x
            None => unit_value
          },
        ),
      )
    ResultErr(inner) =>
      Err(
        E::from_value(
          match inner {
            Some(x) => x
            None => unit_value
          },
        ),
      )
    other =>
      raise FromSchemaError::shape_mismatch(
        "result",
        value_kind(other),
        "Result",
      )
  }
}

// ============================================================
// Map[K, V] -> map
// ============================================================

///|
pub impl[K : IntoSchema, V : IntoSchema] IntoSchema for Map[K, V] with type_id() {
  type_id_with_args("moonbitlang.core.Map", [K::type_id(), V::type_id()])
}

///|
pub impl[K : IntoSchema, V : IntoSchema] IntoSchema for Map[K, V] with register_in(
  b,
) {
  @model.schema_type(Map(K::register_in(b), V::register_in(b)))
}

///|
pub impl[K : IntoSchema, V : IntoSchema] IntoSchema for Map[K, V] with to_value(
  self,
) {
  let entries : Array[@model.SchemaMapEntry] = []
  for k, v in self {
    entries.push({ key: K::to_value(k), value: V::to_value(v) })
  }
  Map(entries)
}

///|
pub impl[K : FromSchema + Hash + Eq, V : FromSchema] FromSchema for Map[K, V] with from_value(
  v,
) {
  match v {
    Map(entries) => {
      let m : Map[K, V] = Map::new()
      for e in entries {
        m[K::from_value(e.key)] = V::from_value(e.value)
      }
      m
    }
    other =>
      raise FromSchemaError::shape_mismatch("map", value_kind(other), "Map")
  }
}
