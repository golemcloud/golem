///|
/// Render the canonical JSON carrier of a reflected schema as JSON Schema.
/// Named definitions remain references, so recursive graphs are finite.
pub fn SchemaRef::to_json_schema(
  self : SchemaRef,
  include_draft_marker? : Bool = true,
) -> Json {
  let root = schema_json(self.graph, self.root)
  let fields : Map[String, Json] = match root {
    Object(value) => value
    _ => {}
  }
  if include_draft_marker {
    fields["$schema"] = Json::string(
      "https://json-schema.org/draft/2020-12/schema",
    )
  }
  if !self.graph.defs.is_empty() {
    let defs : Map[String, Json] = Map([])
    for definition in self.graph.defs {
      defs[definition.id] = schema_json(self.graph, definition.body)
    }
    fields["$defs"] = Json::object(defs)
  }
  Json::object(fields)
}

///|
fn schema_type(name : String) -> Json {
  Json::object({ "type": Json::string(name) })
}

///|
fn schema_number(value : Int) -> Json {
  Json::number(value.to_double())
}

///|
fn schema_strings(values : Array[String]) -> Json {
  let result : Array[Json] = []
  for value in values {
    result.push(Json::string(value))
  }
  Json::array(result)
}

///|
fn schema_array(values : Array[Json]) -> Json {
  Json::array(values)
}

///|
fn schema_integer(
  minimum : Double,
  maximum : Double,
  restrictions : @types.NumericRestrictions?,
) -> Json {
  let fields : Map[String, Json] = {
    "type": Json::string("integer"),
    "minimum": Json::number(minimum),
    "maximum": Json::number(maximum),
  }
  match restrictions {
    Some(value) => {
      match value.min {
        Some(bound) =>
          fields["minimum"] = Json::number(
            minimum.max(schema_bound_number(bound)),
          )
        None => ()
      }
      match value.max {
        Some(bound) =>
          fields["maximum"] = Json::number(
            maximum.min(schema_bound_number(bound)),
          )
        None => ()
      }
    }
    None => ()
  }
  Json::object(fields)
}

///|
fn schema_bound_number(bound : @types.NumericBound) -> Double {
  match bound {
    Signed(value) => value.to_double()
    Unsigned(value) => value.to_double()
    FloatBits(bits) => bits.reinterpret_as_double()
  }
}

///|
fn schema_decimal(restrictions : @types.NumericRestrictions?) -> Json {
  let fields : Map[String, Json] = { "type": Json::string("number") }
  match restrictions {
    Some(value) => {
      match value.min {
        Some(bound) =>
          fields["minimum"] = Json::number(schema_bound_number(bound))
        None => ()
      }
      match value.max {
        Some(bound) =>
          fields["maximum"] = Json::number(schema_bound_number(bound))
        None => ()
      }
    }
    None => ()
  }
  Json::object(fields)
}

///|
fn schema_bound_text(bound : @types.NumericBound) -> String {
  match bound {
    Signed(value) => value.to_string()
    Unsigned(value) => value.to_string()
    FloatBits(bits) => bits.reinterpret_as_double().to_string()
  }
}

///|
fn schema_wide_integer(
  signed : Bool,
  restrictions : @types.NumericRestrictions?,
) -> Json {
  let fields : Map[String, Json] = {
    "type": Json::string("string"),
    "format": Json::string(if signed { "int64" } else { "uint64" }),
    "pattern": Json::string(
      if signed {
        "^(?:0|-[1-9][0-9]*|[1-9][0-9]*)$"
      } else {
        "^(?:0|[1-9][0-9]*)$"
      },
    ),
    "x-golem-minimum": Json::string(
      if signed {
        "-9223372036854775808"
      } else {
        "0"
      },
    ),
    "x-golem-maximum": Json::string(
      if signed {
        "9223372036854775807"
      } else {
        "18446744073709551615"
      },
    ),
  }
  match restrictions {
    Some(value) => {
      match value.min {
        Some(bound) =>
          fields["x-golem-minimum"] = Json::string(schema_bound_text(bound))
        None => ()
      }
      match value.max {
        Some(bound) =>
          fields["x-golem-maximum"] = Json::string(schema_bound_text(bound))
        None => ()
      }
    }
    None => ()
  }
  Json::object(fields)
}

///|
fn schema_resolves_to_option(
  graph : @model.SchemaGraph,
  schema : @model.SchemaType,
  visited : Map[String, Unit],
) -> Bool {
  match schema.body {
    Option(_) => true
    Ref(id) =>
      if visited.contains(id) {
        false
      } else {
        visited[id] = ()
        match graph.defs.iter().find_first(definition => definition.id == id) {
          Some(definition) =>
            schema_resolves_to_option(graph, definition.body, visited)
          None => false
        }
      }
    _ => false
  }
}

///|
fn schema_record(
  graph : @model.SchemaGraph,
  fields : Array[@model.NamedFieldType],
) -> Json {
  let properties : Map[String, Json] = Map([])
  let required : Array[Json] = []
  for field in fields {
    properties[field.name] = schema_with_metadata(
      schema_json(graph, field.body),
      field.metadata,
    )
    if !schema_resolves_to_option(graph, field.body, Map([])) {
      required.push(Json::string(field.name))
    }
  }
  Json::object({
    "type": Json::string("object"),
    "properties": Json::object(properties),
    "required": Json::array(required),
    "additionalProperties": Json::boolean(false),
  })
}

///|
fn schema_with_metadata(
  rendered : Json,
  metadata : @types.MetadataEnvelope,
) -> Json {
  let fields = match rendered {
    Object(fields) => fields
    _ => Map([])
  }
  match metadata.doc {
    Some(value) => fields["description"] = Json::string(value)
    None => ()
  }
  if !metadata.aliases.is_empty() {
    fields["x-golem-aliases"] = schema_strings(metadata.aliases)
  }
  if !metadata.examples.is_empty() {
    fields["examples"] = schema_strings(metadata.examples)
  }
  match metadata.deprecated {
    Some(note) => {
      fields["deprecated"] = Json::boolean(true)
      fields["x-golem-deprecation-note"] = Json::string(note)
    }
    None => ()
  }
  Json::object(fields)
}

///|
fn schema_regex_literal(value : String) -> String {
  value
  .replace_all(old="\\", new="\\\\")
  .replace_all(old="^", new="\\^")
  .replace_all(old="$", new="\\$")
  .replace_all(old=".", new="\\.")
  .replace_all(old="*", new="\\*")
  .replace_all(old="+", new="\\+")
  .replace_all(old="?", new="\\?")
  .replace_all(old="(", new="\\(")
  .replace_all(old=")", new="\\)")
  .replace_all(old="[", new="\\[")
  .replace_all(old="]", new="\\]")
  .replace_all(old="{", new="\\{")
  .replace_all(old="}", new="\\}")
  .replace_all(old="|", new="\\|")
}

///|
fn schema_discriminator(rule : @types.DiscriminatorRule) -> Json {
  match rule {
    Prefix(value) =>
      Json::object({
        "pattern": Json::string("^" + schema_regex_literal(value)),
      })
    Suffix(value) =>
      Json::object({
        "pattern": Json::string(schema_regex_literal(value) + "$"),
      })
    Contains(value) =>
      Json::object({ "pattern": Json::string(schema_regex_literal(value)) })
    Regex(value) => Json::object({ "pattern": Json::string(value) })
    FieldEquals(field) => {
      let result : Map[String, Json] = {
        "required": schema_strings([field.field_name]),
      }
      match field.literal {
        Some(literal) => {
          let properties : Map[String, Json] = Map([])
          properties[field.field_name] = Json::object({
            "const": Json::string(literal),
          })
          result["properties"] = Json::object(properties)
        }
        None => ()
      }
      Json::object(result)
    }
    FieldAbsent(name) =>
      Json::object({
        "not": Json::object({ "required": schema_strings([name]) }),
      })
  }
}

///|
fn schema_quantity_bound(value : @types.QuantityValue) -> Json {
  Json::object({
    "mantissa": Json::string(value.mantissa.to_string()),
    "scale": Json::number(value.scale.to_double()),
    "unit": Json::string(value.unit),
  })
}

///|
fn schema_variant(
  graph : @model.SchemaGraph,
  cases : Array[@model.VariantCaseType],
) -> Json {
  let choices : Array[Json] = []
  for entry in cases {
    match entry.payload {
      None => choices.push(Json::object({ "const": Json::string(entry.name) }))
      Some(payload) => {
        let properties : Map[String, Json] = Map([])
        properties[entry.name] = schema_json(graph, payload)
        choices.push(
          Json::object({
            "type": Json::string("object"),
            "properties": Json::object(properties),
            "required": schema_strings([entry.name]),
            "additionalProperties": Json::boolean(false),
          }),
        )
      }
    }
  }
  Json::object({ "oneOf": Json::array(choices) })
}

///|
fn schema_pair(first : Json, second : Json) -> Json {
  Json::object({
    "type": Json::string("array"),
    "prefixItems": schema_array([first, second]),
    "items": Json::boolean(false),
    "minItems": schema_number(2),
    "maxItems": schema_number(2),
  })
}

///|
fn schema_result_side(
  graph : @model.SchemaGraph,
  name : String,
  payload : @model.SchemaType?,
) -> Json {
  let properties : Map[String, Json] = Map([])
  properties[name] = match payload {
    Some(value) => schema_json(graph, value)
    None => schema_type("null")
  }
  Json::object({
    "type": Json::string("object"),
    "properties": Json::object(properties),
    "required": schema_strings([name]),
    "additionalProperties": Json::boolean(false),
  })
}

///|
fn schema_json(graph : @model.SchemaGraph, schema : @model.SchemaType) -> Json {
  let rendered = match schema.body {
    Ref(id) =>
      Json::object({
        "$ref": Json::string(
          "#/$defs/" +
          id.replace_all(old="~", new="~0").replace_all(old="/", new="~1"),
        ),
      })
    Bool => schema_type("boolean")
    S8(r) => schema_integer(-128.0, 127.0, r)
    S16(r) => schema_integer(-32768.0, 32767.0, r)
    S32(r) => schema_integer(-2147483648.0, 2147483647.0, r)
    S64(r) => schema_wide_integer(true, r)
    U8(r) => schema_integer(0.0, 255.0, r)
    U16(r) => schema_integer(0.0, 65535.0, r)
    U32(r) => schema_integer(0.0, 4294967295.0, r)
    U64(r) => schema_wide_integer(false, r)
    F32(r) | F64(r) => schema_decimal(r)
    Char =>
      Json::object({
        "type": Json::string("string"),
        "minLength": schema_number(1),
        "maxLength": schema_number(1),
      })
    String => schema_type("string")
    Record(fields) => schema_record(graph, fields)
    Variant(cases) => schema_variant(graph, cases)
    Enum(cases) =>
      Json::object({
        "type": Json::string("string"),
        "enum": schema_strings(cases),
      })
    Flags(names) =>
      Json::object({
        "type": Json::string("array"),
        "items": Json::object({
          "type": Json::string("string"),
          "enum": schema_strings(names),
        }),
        "uniqueItems": Json::boolean(true),
      })
    Tuple(elements) => {
      let items : Array[Json] = []
      for element in elements {
        items.push(schema_json(graph, element))
      }
      Json::object({
        "type": Json::string("array"),
        "prefixItems": Json::array(items),
        "items": Json::boolean(false),
        "minItems": schema_number(elements.length()),
        "maxItems": schema_number(elements.length()),
      })
    }
    List(element) =>
      Json::object({
        "type": Json::string("array"),
        "items": schema_json(graph, element),
      })
    FixedList(element, length) =>
      Json::object({
        "type": Json::string("array"),
        "items": schema_json(graph, element),
        "minItems": Json::number(length.to_double()),
        "maxItems": Json::number(length.to_double()),
      })
    Map(key, value) =>
      Json::object({
        "type": Json::string("array"),
        "items": schema_pair(schema_json(graph, key), schema_json(graph, value)),
      })
    Option(element) =>
      Json::object({
        "oneOf": schema_array([schema_type("null"), schema_json(graph, element)]),
      })
    Result(ok, err) =>
      Json::object({
        "oneOf": schema_array([
          schema_result_side(graph, "ok", ok),
          schema_result_side(graph, "err", err),
        ]),
      })
    Text(restrictions) => {
      let text : Map[String, Json] = { "type": Json::string("string") }
      match restrictions.min_length {
        Some(value) => text["minLength"] = Json::number(value.to_double())
        None => ()
      }
      match restrictions.max_length {
        Some(value) => text["maxLength"] = Json::number(value.to_double())
        None => ()
      }
      match restrictions.regex {
        Some(value) => text["pattern"] = Json::string(value)
        None => ()
      }
      let language : Map[String, Json] = { "type": Json::string("string") }
      match restrictions.languages {
        Some(values) => language["enum"] = schema_strings(values)
        None => ()
      }
      Json::object({
        "type": Json::string("object"),
        "properties": Json::object({
          "text": Json::object(text),
          "language": Json::object(language),
        }),
        "required": schema_strings(["text"]),
        "additionalProperties": Json::boolean(false),
      })
    }
    Binary(restrictions) => {
      let bytes : Map[String, Json] = {
        "type": Json::string("string"),
        "contentEncoding": Json::string("base64url"),
        "pattern": Json::string(
          "^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-][AQgw]|[A-Za-z0-9_-]{2}[AEIMQUYcgkosw048])?$",
        ),
      }
      match restrictions.min_bytes {
        Some(value) =>
          bytes["minLength"] = Json::number(
            ((value.to_uint64() * 4UL + 2UL) / 3UL).to_double(),
          )
        None => ()
      }
      match restrictions.max_bytes {
        Some(value) =>
          bytes["maxLength"] = Json::number(
            ((value.to_uint64() * 4UL + 2UL) / 3UL).to_double(),
          )
        None => ()
      }
      let mime : Map[String, Json] = {
        "type": Json::string("string"),
        "pattern": Json::string(
          "^[A-Za-z0-9!#$&^_.+\\-]+\\/[A-Za-z0-9!#$&^_.+\\-]+$",
        ),
      }
      match restrictions.mime_types {
        Some(values) => mime["enum"] = schema_strings(values)
        None => ()
      }
      Json::object({
        "type": Json::string("object"),
        "properties": Json::object({
          "bytes": Json::object(bytes),
          "mimeType": Json::object(mime),
        }),
        "required": schema_strings(["bytes"]),
        "additionalProperties": Json::boolean(false),
      })
    }
    Path(spec) => {
      let fields : Map[String, Json] = {
        "type": Json::string("string"),
        "format": Json::string("file-path"),
      }
      let direction = match spec.direction {
        INPUT => "input"
        OUTPUT => "output"
        IN_OUT => "input/output"
      }
      let kind = match spec.kind {
        FILE => "file"
        DIRECTORY => "directory"
        ANY => "any"
      }
      fields["title"] = Json::string(direction + " " + kind + " path")
      let descriptions : Array[String] = []
      match spec.allowed_extensions {
        Some(values) =>
          descriptions.push("Allowed extensions: " + values.join(", "))
        None => ()
      }
      match spec.allowed_mime_types {
        Some(values) =>
          descriptions.push("Allowed MIME types: " + values.join(", "))
        None => ()
      }
      if !descriptions.is_empty() {
        fields["description"] = Json::string(descriptions.join("; "))
      }
      Json::object(fields)
    }
    Url(restrictions) => {
      let fields : Map[String, Json] = {
        "type": Json::string("string"),
        "format": Json::string("uri"),
        "title": Json::string("URL"),
      }
      let descriptions : Array[String] = []
      match restrictions.allowed_schemes {
        Some(values) =>
          descriptions.push("Allowed schemes: " + values.join(", "))
        None => ()
      }
      match restrictions.allowed_hosts {
        Some(values) => descriptions.push("Allowed hosts: " + values.join(", "))
        None => ()
      }
      if !descriptions.is_empty() {
        fields["description"] = Json::string(descriptions.join("; "))
      }
      Json::object(fields)
    }
    Datetime =>
      Json::object({
        "type": Json::string("string"),
        "format": Json::string("date-time"),
      })
    Duration =>
      Json::object({
        "type": Json::string("object"),
        "properties": Json::object({
          "nanoseconds": schema_wide_integer(true, None),
        }),
        "required": schema_strings(["nanoseconds"]),
        "additionalProperties": Json::boolean(false),
        "title": Json::string("Duration in nanoseconds"),
      })
    Quantity(spec) => {
      let fields : Map[String, Json] = {
        "type": Json::string("object"),
        "properties": Json::object({
          "mantissa": schema_wide_integer(true, None),
          "scale": schema_type("integer"),
          "unit": schema_type("string"),
        }),
        "required": schema_strings(["mantissa", "scale", "unit"]),
        "additionalProperties": Json::boolean(false),
        "title": Json::string("Quantity (" + spec.base_unit + ")"),
      }
      if !spec.allowed_suffixes.is_empty() {
        fields["x-golem-allowed-suffixes"] = schema_strings(
          spec.allowed_suffixes,
        )
      }
      match spec.min {
        Some(value) =>
          fields["x-golem-minimum-quantity"] = schema_quantity_bound(value)
        None => ()
      }
      match spec.max {
        Some(value) =>
          fields["x-golem-maximum-quantity"] = schema_quantity_bound(value)
        None => ()
      }
      Json::object(fields)
    }
    Union(branches) => {
      let choices : Array[Json] = []
      for branch in branches {
        choices.push(
          Json::object({
            "allOf": schema_array([
              schema_with_metadata(
                schema_json(graph, branch.body),
                branch.metadata,
              ),
              schema_discriminator(branch.discriminator),
            ]),
          }),
        )
      }
      Json::object({ "oneOf": Json::array(choices) })
    }
    Secret(_) | QuotaToken(_) | PermissionCard(_) | Future(_) | Stream(_) =>
      Json::object({ "not": Json::object({}) })
  }
  schema_with_metadata(rendered, schema.metadata)
}
