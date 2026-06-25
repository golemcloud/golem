///|
pub(all) struct Path {
  value : String
} derive(Show, Eq)

///|
pub fn Path::new(value : String) -> Path {
  { value, }
}

///|
pub(all) struct Url {
  value : String
} derive(Show, Eq)

///|
pub fn Url::new(value : String) -> Url {
  { value, }
}

///|
pub(all) struct Duration {
  nanoseconds : Int64
} derive(Show, Eq)

///|
pub fn Duration::new(nanoseconds : Int64) -> Duration {
  { nanoseconds, }
}

///|
pub(open) trait QuantityUnit {
  type_id() -> String
  base_unit() -> String
  allowed_suffixes() -> Array[String]
}

///|
pub struct Quantity[U] {
  mantissa : Int64
  scale : Int
  unit : String
} derive(Show, Eq)

///|
pub fn[U : QuantityUnit] unit_allowed(_tag : TypeTag[U], unit : String) -> Bool {
  let suffixes = U::allowed_suffixes()
  if suffixes.is_empty() {
    unit == U::base_unit()
  } else {
    suffixes.contains(unit)
  }
}

///|
pub fn[U : QuantityUnit] Quantity::new(
  mantissa : Int64,
  scale : Int,
  unit : String,
) -> Quantity[U] raise FromSchemaError {
  let tag : TypeTag[U] = type_tag()
  if unit_allowed(tag, unit) {
    { mantissa, scale, unit }
  } else {
    raise FromSchemaError::custom(
      "quantity unit '\{unit}' is not allowed for \{U::base_unit()}",
    )
  }
}

///|
pub fn[U] Quantity::value(self : Quantity[U]) -> @types.QuantityValue {
  { mantissa: self.mantissa, scale: self.scale, unit: self.unit }
}

///|
fn default_path_spec() -> @types.PathSpec {
  {
    direction: IN_OUT,
    kind: ANY,
    allowed_mime_types: None,
    allowed_extensions: None,
  }
}

///|
fn default_url_restrictions() -> @types.UrlRestrictions {
  { allowed_schemes: None, allowed_hosts: None }
}

///|
pub impl IntoSchema for Path with type_id() {
  "golem.core.Path"
}

///|
pub impl IntoSchema for Path with register_in(_b) {
  @model.schema_type(Path(default_path_spec()))
}

///|
pub impl IntoSchema for Path with to_value(self) {
  Path(self.value)
}

///|
pub impl FromSchema for Path with from_value(v) {
  match v {
    Path(value) => { value, }
    other =>
      raise FromSchemaError::shape_mismatch("path", value_kind(other), "Path")
  }
}

///|
pub impl IntoSchema for Url with type_id() {
  "golem.core.Url"
}

///|
pub impl IntoSchema for Url with register_in(_b) {
  @model.schema_type(Url(default_url_restrictions()))
}

///|
pub impl IntoSchema for Url with to_value(self) {
  Url(self.value)
}

///|
pub impl FromSchema for Url with from_value(v) {
  match v {
    Url(value) => { value, }
    other =>
      raise FromSchemaError::shape_mismatch("url", value_kind(other), "Url")
  }
}

///|
pub impl IntoSchema for @types.Datetime with type_id() {
  "golem.core.Datetime"
}

///|
pub impl IntoSchema for @types.Datetime with register_in(_b) {
  @model.schema_type(Datetime)
}

///|
pub impl IntoSchema for @types.Datetime with to_value(self) {
  Datetime(self)
}

///|
pub impl FromSchema for @types.Datetime with from_value(v) {
  match v {
    Datetime(value) => value
    other =>
      raise FromSchemaError::shape_mismatch(
        "datetime",
        value_kind(other),
        "Datetime",
      )
  }
}

///|
pub impl IntoSchema for Duration with type_id() {
  "golem.core.Duration"
}

///|
pub impl IntoSchema for Duration with register_in(_b) {
  @model.schema_type(Duration)
}

///|
pub impl IntoSchema for Duration with to_value(self) {
  Duration(self.nanoseconds)
}

///|
pub impl FromSchema for Duration with from_value(v) {
  match v {
    Duration(nanoseconds) => { nanoseconds, }
    other =>
      raise FromSchemaError::shape_mismatch(
        "duration",
        value_kind(other),
        "Duration",
      )
  }
}

///|
pub impl[U : QuantityUnit] IntoSchema for Quantity[U] with type_id() {
  type_id_with_args("golem.core.Quantity", [U::type_id()])
}

///|
pub impl[U : QuantityUnit] IntoSchema for Quantity[U] with register_in(_b) {
  @model.schema_type(
    Quantity({
      base_unit: U::base_unit(),
      allowed_suffixes: U::allowed_suffixes(),
      min: None,
      max: None,
    }),
  )
}

///|
pub impl[U : QuantityUnit] IntoSchema for Quantity[U] with to_value(self) {
  Quantity(self.value())
}

///|
pub impl[U : QuantityUnit] FromSchema for Quantity[U] with from_value(v) {
  match v {
    Quantity(value) => {
      let tag : TypeTag[U] = type_tag()
      if unit_allowed(tag, value.unit) {
        { mantissa: value.mantissa, scale: value.scale, unit: value.unit }
      } else {
        raise FromSchemaError::custom(
          "quantity unit '\{value.unit}' is not allowed for \{U::base_unit()}",
        )
      }
    }
    other =>
      raise FromSchemaError::shape_mismatch(
        "quantity",
        value_kind(other),
        "Quantity",
      )
  }
}
