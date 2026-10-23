///|
pub(all) struct Path {
  value : String
} derive(Debug, Eq)

///|
pub fn Path::new(value : String) -> Path {
  { value, }
}

///|
pub(all) struct Url {
  value : String
} derive(Debug, Eq)

///|
pub fn Url::new(value : String) -> Url {
  { value, }
}

///|
pub(open) trait TextSpec {
  fn type_id() -> String
  fn languages() -> Array[String]
  fn min_length() -> UInt?
  fn max_length() -> UInt?
  fn regex() -> String?
}

///|
#warnings("-unused_type_variable")
pub struct Text[S] {
  value : String
  language : String?
} derive(Debug, Eq)

///|
pub fn[S] Text::new(value : String, language? : String) -> Text[S] {
  { value, language, }
}

///|
pub fn[S] Text::value(self : Text[S]) -> String {
  self.value
}

///|
pub fn[S] Text::language(self : Text[S]) -> String? {
  self.language
}

///|
pub(all) struct Duration {
  nanoseconds : Int64
} derive(Debug, Eq)

///|
pub fn Duration::new(nanoseconds : Int64) -> Duration {
  { nanoseconds, }
}

///|
/// A universally unique identifier (RFC 9562), stored as its 128 bits. `Show`
/// and `Uuid::to_string` print the lowercase hyphenated form.
pub struct Uuid {
  priv high_bits : UInt64
  priv low_bits : UInt64
} derive(Eq, Compare, Hash)

///|
pub suberror UuidParseError {
  UuidParseError(String)
} derive(Debug, Eq)

///|
pub fn Uuid::from_bits(high_bits : UInt64, low_bits : UInt64) -> Uuid {
  { high_bits, low_bits, }
}

///|
pub fn Uuid::high_bits(self : Uuid) -> UInt64 {
  self.high_bits
}

///|
pub fn Uuid::low_bits(self : Uuid) -> UInt64 {
  self.low_bits
}

///|
pub fn Uuid::from_wit(uuid : @types.Uuid) -> Uuid {
  { high_bits: uuid.high_bits, low_bits: uuid.low_bits, }
}

///|
pub fn Uuid::to_wit(self : Uuid) -> @types.Uuid {
  { high_bits: self.high_bits, low_bits: self.low_bits, }
}

///|
/// Generates a random (version 4) UUID from `wasi:random/random`. Golem
/// records the random bits in the oplog, so the value is stable on replay.
pub fn Uuid::random() -> Uuid {
  let high = @random.get_random_u64()
  let low = @random.get_random_u64()
  {
    high_bits: (high & 0xFFFFFFFFFFFF0FFFUL) | 0x0000000000004000UL,
    low_bits: (low & 0x3FFFFFFFFFFFFFFFUL) | 0x8000000000000000UL,
  }
}

///|
fn uuid_hex_digit(c : UInt16) -> UInt64? {
  let c = c.to_int()
  if c >= '0' && c <= '9' {
    Some((c - '0').to_uint64())
  } else if c >= 'a' && c <= 'f' {
    Some((c - 'a' + 10).to_uint64())
  } else if c >= 'A' && c <= 'F' {
    Some((c - 'A' + 10).to_uint64())
  } else {
    None
  }
}

///|
/// Parses the hyphenated form `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`. Hex
/// digits may be upper or lower case.
pub fn Uuid::parse(value : String) -> Uuid raise UuidParseError {
  if value.length() != 36 {
    raise UuidParseError("invalid UUID '\{value}': expected 36 characters")
  }
  let mut high = 0UL
  let mut low = 0UL
  let mut digits = 0
  for i in 0..<36 {
    let c = value.code_unit_at(i)
    if i == 8 || i == 13 || i == 18 || i == 23 {
      if c.to_int() != '-' {
        raise UuidParseError("invalid UUID '\{value}': expected '-' at \{i}")
      }
      continue
    }
    guard uuid_hex_digit(c) is Some(digit) else {
      raise UuidParseError("invalid UUID '\{value}': invalid hex digit at \{i}")
    }
    if digits < 16 {
      high = (high << 4) | digit
    } else {
      low = (low << 4) | digit
    }
    digits += 1
  }
  { high_bits: high, low_bits: low, }
}

///|
fn uuid_write_hex(logger : &Logger, value : UInt64, digits : Int) -> Unit {
  for i = digits - 1; i >= 0; i = i - 1 {
    let nibble = ((value >> (i * 4)) & 0xFUL).to_int()
    logger.write_char("0123456789abcdef".unsafe_get(nibble).unsafe_to_char())
  }
}

///|
pub impl Show for Uuid with fn output(self, logger) {
  uuid_write_hex(logger, self.high_bits >> 32, 8)
  logger.write_char('-')
  uuid_write_hex(logger, self.high_bits >> 16, 4)
  logger.write_char('-')
  uuid_write_hex(logger, self.high_bits, 4)
  logger.write_char('-')
  uuid_write_hex(logger, self.low_bits >> 48, 4)
  logger.write_char('-')
  uuid_write_hex(logger, self.low_bits, 12)
}

///|
pub impl Debug for Uuid with fn to_repr(self) {
  Repr::ctor("Uuid", [(None, Repr::string(self.to_string()))])
}

///|
pub(open) trait QuantityUnit {
  fn type_id() -> String
  fn base_unit() -> String
  fn allowed_suffixes() -> Array[String]
}

///|
#warnings("-unused_type_variable")
pub struct Quantity[U] {
  mantissa : Int64
  scale : Int
  unit : String
} derive(Debug, Eq)

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
    { mantissa, scale, unit, }
  } else {
    raise FromSchemaError::custom(
      "quantity unit '\{unit}' is not allowed for \{U::base_unit()}",
    )
  }
}

///|
pub fn[U] Quantity::value(self : Quantity[U]) -> @types.QuantityValue {
  { mantissa: self.mantissa, scale: self.scale, unit: self.unit, }
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
  { allowed_schemes: None, allowed_hosts: None, }
}

///|
fn[S : TextSpec] text_restrictions(
  _tag : TypeTag[S],
) -> @types.TextRestrictions {
  {
    languages: if S::languages().is_empty() {
      None
    } else {
      Some(S::languages())
    },
    min_length: S::min_length(),
    max_length: S::max_length(),
    regex: S::regex(),
  }
}

///|
pub impl IntoSchema for Path with fn type_id() {
  "golem.core.Path"
}

///|
pub impl IntoSchema for Path with fn register_in(_b) {
  @model.schema_type(Path(default_path_spec()))
}

///|
pub impl IntoSchema for Path with fn to_value(self) {
  Path(self.value)
}

///|
pub impl FromSchema for Path with fn from_value(v) {
  match v {
    Path(value) => { value, }
    other =>
      raise FromSchemaError::shape_mismatch("path", value_kind(other), "Path")
  }
}

///|
pub impl IntoSchema for Url with fn type_id() {
  "golem.core.Url"
}

///|
pub impl IntoSchema for Url with fn register_in(_b) {
  @model.schema_type(Url(default_url_restrictions()))
}

///|
pub impl IntoSchema for Url with fn to_value(self) {
  Url(self.value)
}

///|
pub impl FromSchema for Url with fn from_value(v) {
  match v {
    Url(value) => { value, }
    other =>
      raise FromSchemaError::shape_mismatch("url", value_kind(other), "Url")
  }
}

///|
pub impl[S : TextSpec] IntoSchema for Text[S] with fn type_id() {
  type_id_with_args("golem.core.Text", [S::type_id()])
}

///|
pub impl[S : TextSpec] IntoSchema for Text[S] with fn register_in(_b) {
  @model.schema_type(Text(text_restrictions((type_tag() : TypeTag[S]))))
}

///|
pub impl[S : TextSpec] IntoSchema for Text[S] with fn to_value(self) {
  Text(self.value, self.language)
}

///|
pub impl[S] FromSchema for Text[S] with fn from_value(v) {
  match v {
    Text(value, language) => { value, language, }
    other =>
      raise FromSchemaError::shape_mismatch("text", value_kind(other), "Text")
  }
}

///|
pub impl IntoSchema for Uuid with fn type_id() {
  "golem.core.Uuid"
}

///|
pub impl IntoSchema for Uuid with fn register_in(_b) {
  @model.schema_type(Uuid)
}

///|
pub impl IntoSchema for Uuid with fn to_value(self) {
  Uuid(self.to_wit())
}

///|
pub impl FromSchema for Uuid with fn from_value(v) {
  match v {
    Uuid(value) => Uuid::from_wit(value)
    other =>
      raise FromSchemaError::shape_mismatch("uuid", value_kind(other), "Uuid")
  }
}

///|
pub impl IntoSchema for @types.Datetime with fn type_id() {
  "golem.core.Datetime"
}

///|
pub impl IntoSchema for @types.Datetime with fn register_in(_b) {
  @model.schema_type(Datetime)
}

///|
pub impl IntoSchema for @types.Datetime with fn to_value(self) {
  Datetime(self)
}

///|
pub impl FromSchema for @types.Datetime with fn from_value(v) {
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
pub impl IntoSchema for Duration with fn type_id() {
  "golem.core.Duration"
}

///|
pub impl IntoSchema for Duration with fn register_in(_b) {
  @model.schema_type(Duration)
}

///|
pub impl IntoSchema for Duration with fn to_value(self) {
  Duration(self.nanoseconds)
}

///|
pub impl FromSchema for Duration with fn from_value(v) {
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
pub impl[U : QuantityUnit] IntoSchema for Quantity[U] with fn type_id() {
  type_id_with_args("golem.core.Quantity", [U::type_id()])
}

///|
pub impl[U : QuantityUnit] IntoSchema for Quantity[U] with fn register_in(_b) {
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
pub impl[U : QuantityUnit] IntoSchema for Quantity[U] with fn to_value(self) {
  Quantity(self.value())
}

///|
pub impl[U : QuantityUnit] FromSchema for Quantity[U] with fn from_value(v) {
  match v {
    Quantity(value) => {
      let tag : TypeTag[U] = type_tag()
      if unit_allowed(tag, value.unit) {
        { mantissa: value.mantissa, scale: value.scale, unit: value.unit, }
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

///|
pub impl IntoWire for Url with fn append_wire(self, encoder) {
  encoder.push(@types.UrlValue(wire_text(self.value, "url")))
}

///|
pub impl FromWire for Url with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.UrlValue(value) => Url::new(value)
    _ => raise WireError::Invalid("expected URL value")
  }
}

///|
pub impl WireSchema for Url with fn append_wire_type(encoder) {
  encoder.push(@types.UrlType(default_url_restrictions()))
}

///|
pub impl[S] IntoWire for Text[S] with fn append_wire(self, encoder) {
  let language = match self.language {
    Some(language) => Some(wire_text(language, "text language"))
    None => None
  }
  encoder.push(
    @types.TextValue({ text: wire_text(self.value, "text"), language, }),
  )
}

///|
pub impl[S] FromWire for Text[S] with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.TextValue(value) => { value: value.text, language: value.language, }
    _ => raise WireError::Invalid("expected text value")
  }
}

///|
pub impl[S : TextSpec] WireSchema for Text[S] with fn append_wire_type(encoder) {
  encoder.push(@types.TextType(text_restrictions((type_tag() : TypeTag[S]))))
}

///|
pub impl IntoWire for Duration with fn append_wire(self, encoder) {
  encoder.push(@types.DurationValue({ nanoseconds: self.nanoseconds, }))
}

///|
pub impl FromWire for Duration with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.DurationValue(value) => Duration::new(value.nanoseconds)
    _ => raise WireError::Invalid("expected duration value")
  }
}

///|
pub impl WireSchema for Duration with fn append_wire_type(encoder) {
  encoder.push(@types.DurationType)
}

///|
pub impl[U] IntoWire for Quantity[U] with fn append_wire(self, encoder) {
  let value = self.value()
  ignore(wire_text(value.unit, "quantity unit"))
  encoder.push(@types.QuantityValueNode(value))
}

///|
pub impl[U : QuantityUnit] FromWire for Quantity[U] with fn read_wire(
  reader,
  index,
) {
  match wire_node(reader, index) {
    @types.QuantityValueNode(value) => {
      guard unit_allowed((type_tag() : TypeTag[U]), value.unit) else {
        raise WireError::Invalid("quantity unit is not allowed")
      }
      { mantissa: value.mantissa, scale: value.scale, unit: value.unit, }
    }
    _ => raise WireError::Invalid("expected quantity value")
  }
}

///|
pub impl[U : QuantityUnit] WireSchema for Quantity[U] with fn append_wire_type(
  encoder,
) {
  encoder.push(
    @types.QuantityType({
      base_unit: U::base_unit(),
      allowed_suffixes: U::allowed_suffixes(),
      min: None,
      max: None,
    }),
  )
}

///|
/// The `binary` kind with its optional MIME type. `Bytes` maps to the same
/// kind but drops the MIME type.
pub(all) struct AgentBinary {
  bytes : Bytes
  mime_type : String?
} derive(Debug, Eq)

///|
pub impl IntoSchema for AgentBinary with fn type_id() {
  "golem.core.AgentBinary"
}

///|
pub impl IntoSchema for AgentBinary with fn register_in(_b) {
  @model.schema_type(
    Binary({ mime_types: None, min_bytes: None, max_bytes: None, }),
  )
}

///|
pub impl IntoSchema for AgentBinary with fn to_value(self) {
  Binary(self.bytes.to_fixedarray(), self.mime_type)
}

///|
pub impl FromSchema for AgentBinary with fn from_value(v) {
  match v {
    Binary(bytes, mime_type) =>
      { bytes: Bytes::from_array(bytes[:]), mime_type, }
    other =>
      raise FromSchemaError::shape_mismatch(
        "binary",
        value_kind(other),
        "AgentBinary",
      )
  }
}

///|
pub impl IntoWire for AgentBinary with fn append_wire(self, encoder) {
  let mime_type = match self.mime_type {
    Some(mime_type) => Some(wire_text(mime_type, "MIME type"))
    None => None
  }
  encoder.push(
    @types.SchemaValueNode::BinaryValue({
      bytes: self.bytes.to_fixedarray(),
      mime_type,
    }),
  )
}

///|
pub impl FromWire for AgentBinary with fn read_wire(reader, index) {
  match wire_node(reader, index) {
    @types.SchemaValueNode::BinaryValue(value) =>
      { bytes: Bytes::from_array(value.bytes[:]), mime_type: value.mime_type, }
    _ => raise WireError::Invalid("expected binary value")
  }
}

///|
pub impl WireSchema for AgentBinary with fn append_wire_type(encoder) {
  encoder.push(
    @types.SchemaTypeBody::BinaryType({
      mime_types: None,
      min_bytes: None,
      max_bytes: None,
    }),
  )
}
