///|
#derive.golem_schema
pub(all) struct Settings {
  label : String
  count : UInt32
} derive(ToJson)

///|
pub type SettingsAlias = Settings

///|
// A schema-native union uses the existing codec traits. Config derivation must
// preserve it as a named union, not replace it with the enum's variant shape.
pub(all) enum Choice {
  Text(String)
  Other(String)
} derive(ToJson)

///|
pub impl @schema.IntoSchema for Choice with fn type_id() {
  "Choice"
}

///|
pub impl @schema.IntoSchema for Choice with fn register_in(b) {
  b.register(
    "Choice",
    () => {
      @model.schema_type(
        Union([
          {
            tag: "Text",
            body: @model.schema_type(String),
            discriminator: @types.Prefix("selected:"),
            metadata: @model.empty_metadata(),
          },
          {
            tag: "Other",
            body: @model.schema_type(String),
            discriminator: @types.Prefix("other:"),
            metadata: @model.empty_metadata(),
          },
        ]),
      )
    },
    Some("Choice"),
  )
}

///|
pub impl @schema.IntoSchema for Choice with fn to_value(self) {
  match self {
    Text(value) => Union("Text", String(value))
    Other(value) => Union("Other", String(value))
  }
}

///|
pub impl @schema.WireSchema for Choice with fn append_wire_type(encoder) {
  encoder.named(
    "Choice",
    () => {
      let body = @schema.append_wire_type_with(
        encoder,
        (@schema.type_tag() : @schema.TypeTag[String]),
      )
      encoder.push(
        @types.UnionType({
          branches: [
            {
              tag: "Text",
              body,
              discriminator: @types.Prefix("selected:"),
              metadata: @schema.empty_wire_metadata(),
            },
            {
              tag: "Other",
              body,
              discriminator: @types.Prefix("other:"),
              metadata: @schema.empty_wire_metadata(),
            },
          ],
        }),
      )
    },
    name=Some("Choice"),
  )
}

///|
pub impl @schema.IntoWire for Choice with fn append_wire(self, encoder) {
  let (tag, value) = match self {
    Text(value) => ("Text", value)
    Other(value) => ("Other", value)
  }
  let body = @schema.IntoWire::append_wire(value, encoder)
  encoder.push(@types.UnionValue({ tag, body, }))
}

///|
pub impl @schema.FromWire for Choice with fn read_wire(reader, index) {
  match @schema.wire_node(reader, index) {
    @types.UnionValue({ tag, body, }) => {
      let value : String = @schema.FromWire::read_wire(reader, body)
      match tag {
        "Text" => Text(value)
        "Other" => Other(value)
        _ => raise @schema.WireError::Invalid("unknown Choice branch")
      }
    }
    _ => raise @schema.WireError::Invalid("expected Choice union")
  }
}

///|
#derive.config
pub(all) struct StaticDefaults {
  #derive.config(default="{\"label\":\"code-default\",\"count\":31}")
  settings : SettingsAlias
  #derive.config(default="\"selected:value\"")
  choice : Choice
  #derive.config(default="[null,7,\"retained\"]")
  slots : (Int32?, Int32?, String)
  omitted : String?
} derive(ToJson)

///|
#derive.agent
struct StaticDefaultsAgent {
  config : @config.Config[StaticDefaults]
}

///|
fn StaticDefaultsAgent::new(
  _name : String,
  config : @config.Config[StaticDefaults],
) -> StaticDefaultsAgent {
  { config, }
}

///|
pub fn StaticDefaultsAgent::read(self : Self) -> String {
  let c = self.config.value
  let choice : Json = match c.choice {
    Text(value) => { "Text": value.to_json() }
    Other(value) => { "Other": value.to_json() }
  }
  let optional_int = fn(value : Int32?) -> Json {
    match value {
      Some(value) => value.to_json()
      None => Json::null()
    }
  }
  let slots : Json = [
    optional_int(c.slots.0),
    optional_int(c.slots.1),
    c.slots.2.to_json(),
  ]
  let omitted : Json = match c.omitted {
    Some(value) => value.to_json()
    None => Json::null()
  }
  (
    {
      "settings": c.settings.to_json(),
      "choice": choice,
      "slots": slots,
      "omitted": omitted,
    } : Json).stringify()
}

///|
#derive.golem_schema
pub(all) struct Node {
  display_name : String
  next : Node?
} derive(ToJson)

///|
#derive.config
pub(all) struct ShapeConfig {
  #derive.config(name="maxChars", default="4000")
  max_chars : UInt32
  #derive.config(default="{\"displayName\":\"outer\",\"next\":{\"displayName\":\"inner\",\"next\":null}}")
  tree : Node
  #derive.config(default="null")
  cleared : String?
  #derive.config(default="false")
  enabled : Bool
  #derive.config(default="[]")
  values : Array[String]
}

///|
#derive.agent
struct StaticShapeAgent {
  config : @config.Config[ShapeConfig]
}

///|
fn StaticShapeAgent::new(
  config : @config.Config[ShapeConfig],
) -> StaticShapeAgent {
  { config, }
}

///|
pub fn StaticShapeAgent::read(self : Self) -> String {
  let c = self.config.value
  let child : Json = match c.tree.next {
    Some(node) => node.display_name.to_json()
    None => Json::null()
  }
  let cleared : Json = match c.cleared {
    Some(value) => value.to_json()
    None => Json::null()
  }
  (
    {
      "maxChars": c.max_chars.to_json(),
      "root": c.tree.display_name.to_json(),
      "child": child,
      "cleared": cleared,
      "enabled": c.enabled.to_json(),
      "values": c.values.to_json(),
    } : Json).stringify()
}
