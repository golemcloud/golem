// Phase 3d tests for the schema-carrier config layer.
//
// The `load`/`get` paths call the WASM host import `get_config_value` and so
// cannot run in a unit test. The host-free surface is config-declaration
// assembly (`collect_entries_of` → `@agents.AgentConfigDef`), which these tests
// cover: source kind (LOCAL vs SECRET), path threading, and a non-empty schema
// graph for the declared value type.

///|
extern "wasm" fn reset_resource_drop_counts() = "golem:test" "reset-resource-drop-counts"

///|
extern "wasm" fn resource_drop_count(kind : Int32) -> Int32 = "golem:test" "resource-drop-count"

///|
test "opaque secret capability is borrowed and dropped without reveal" {
  reset_resource_drop_counts()
  let seen = with_owned_handle(@types.Secret::Secret(77), fn(handle) {
    let @types.Secret::Secret(raw) = handle
    raw
  })
  assert_eq(seen, 77)
  assert_eq(resource_drop_count(0), 1)
}

///|
test "collect_entries_of - primitive String is a LOCAL declaration" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{ } : @schema.TypeTag[String]),
    ["db", "url"],
  )
  debug_inspect(entries.length(), content="1")
  debug_inspect(entries[0].source, content="LOCAL")
  assert_eq(entries[0].default_value, None)
  debug_inspect(
    entries[0].path,
    content=(
      #|["db", "url"]
    ),
  )
  debug_inspect(
    entries[0].value_type.root,
    content=(
      #|{
      #|  body: String,
      #|  metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|}
    ),
  )
}

///|
test "collect_entries_of - Secret is a SECRET declaration over the inner type" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{ } : @schema.TypeTag[Secret[String]]),
    ["api", "key"],
  )
  debug_inspect(entries.length(), content="1")
  debug_inspect(entries[0].source, content="SECRET")
  assert_eq(entries[0].default_value, None)
  debug_inspect(
    entries[0].value_type.root,
    content=(
      #|{
      #|  body: Secret(
      #|    {
      #|      inner: {
      #|        body: String,
      #|        metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|      },
      #|      category: None,
      #|    },
      #|  ),
      #|  metadata: { doc: None, aliases: [], examples: [], deprecated: None, role: None },
      #|}
    ),
  )
}

///|
test "collect_entries_of - Option field is a LOCAL declaration" {
  let entries = collect_entries_of(
    (@schema.TypeTag::{ } : @schema.TypeTag[Int32?]),
    ["timeout"],
  )
  debug_inspect(entries.length(), content="1")
  debug_inspect(entries[0].source, content="LOCAL")
  let defaulted = local_entry(
    (@schema.type_tag() : @schema.TypeTag[Int32?]),
    ["timeout"],
    default_value="null",
  )
  assert_eq(entries[0].default_value, None)
  assert_eq(defaulted[0].default_value, Some(@model.Option(None)))
  assert_eq(defaulted[0].path, entries[0].path)
  assert_eq(defaulted[0].value_type, entries[0].value_type)
}

///|
test "wire configuration declarations share one type pool and preserve secret wrappers" {
  let encoder = @schema.WireTypeEncoder::new()
  let local_entries = append_wire_entries_with(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[UInt64?]),
    ["limits", "max"],
  )
  let secret = append_wire_entries_with(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[Secret[String]]),
    ["api", "key"],
  )
  assert_eq(local_entries[0].source, @common.AgentConfigSource::LOCAL)
  assert_eq(local_entries[0].default_value, None)
  assert_eq(local_entries[0].path, ["limits", "max"])
  assert_eq(local_entries[0].value_type, 1)
  assert_eq(encoder.nodes[0].body, @types.U64Type(None))
  assert_eq(encoder.nodes[1].body, @types.OptionType(0))
  assert_eq(secret[0].source, @common.AgentConfigSource::SECRET)
  assert_eq(secret[0].default_value, None)
  assert_eq(secret[0].path, ["api", "key"])
  assert_eq(secret[0].value_type, 3)
  assert_eq(encoder.nodes[2].body, @types.StringType)
  assert_eq(
    encoder.nodes[3].body,
    @types.SecretType({ inner: 2, category: None, }),
  )
  let defaulted = wire_local_entry(
    encoder,
    (@schema.type_tag() : @schema.TypeTag[UInt64?]),
    ["limits", "cleared"],
    default_value="null",
  )
  let expected_default : @model.SchemaValue? = Some(Option(None))
  assert_eq(
    defaulted[0].default_value.map(@model_host.schema_value_from_wit),
    expected_default,
  )
  assert_eq(defaulted[0].path, ["limits", "cleared"])
  guard encoder.nodes[defaulted[0].value_type].body is @types.OptionType(inner) else {
    fail("defaulted optional declaration lost its option type")
  }
  assert_eq(encoder.nodes[inner].body, @types.U64Type(None))
}

///|
priv struct NamingFixture {}

///|
impl @schema.WireSchema for NamingFixture with fn append_wire_type(encoder) {
  encoder.named("NamingFixture", () => {
    let text = @schema.append_wire_type_with(
      encoder,
      (@schema.type_tag() : @schema.TypeTag[String]),
    )
    let next = @schema.append_wire_type_with(
      encoder,
      (@schema.type_tag() : @schema.TypeTag[NamingFixture?]),
    )
    let record = encoder.push(
      @types.RecordType([
        {
          name: "display_name",
          body: text,
          metadata: @schema.empty_wire_metadata(),
        },
        { name: "next", body: next, metadata: @schema.empty_wire_metadata(), },
      ]),
    )
    encoder.push(
      @types.UnionType({
        branches: [
          {
            tag: "selected",
            body: record,
            discriminator: @types.FieldEquals({
              field_name: "display_name",
              literal: Some("Exact_Value"),
            }),
            metadata: @schema.empty_wire_metadata(),
          },
          {
            tag: "other",
            body: encoder.push(@types.RecordType([])),
            discriminator: @types.FieldAbsent("display_name"),
            metadata: @schema.empty_wire_metadata(),
          },
        ],
      }),
    )
  })
}

///|
impl @schema.IntoSchema for NamingFixture with fn type_id() {
  "NamingFixture"
}

///|
impl @schema.IntoSchema for NamingFixture with fn register_in(builder) {
  builder.register(
    "NamingFixture",
    () => {
      let record = @model.schema_type(
        Record([
          @model.field("display_name", @model.schema_type(String)),
          @model.field(
            "next",
            @model.schema_type(Option(builder.ref_("NamingFixture"))),
          ),
        ]),
      )
      @model.schema_type(
        Union([
          {
            tag: "selected",
            body: record,
            discriminator: @types.FieldEquals({
              field_name: "display_name",
              literal: Some("Exact_Value"),
            }),
            metadata: @model.empty_metadata(),
          },
          {
            tag: "other",
            body: @model.schema_type(Record([])),
            discriminator: @types.FieldAbsent("display_name"),
            metadata: @model.empty_metadata(),
          },
        ]),
      )
    },
    Some("NamingFixture"),
  )
}

///|
impl @schema.IntoSchema for NamingFixture with fn to_value(_) {
  abort("schema-only fixture")
}

///|
test "config names and recursive definitions do not change ordinary schemas" {
  ignore(NamingFixture::{ })
  let tag = (@schema.type_tag() : @schema.TypeTag[NamingFixture])
  let ordinary = @schema.wire_schema(tag)
  let encoder = @schema.WireTypeEncoder::new()
  let method_root = @schema.append_wire_type_with(encoder, tag)
  let first = wire_local_entry(encoder, tag, ["tree"])
  let second = wire_local_entry(encoder, tag, ["other"])
  let config_graph : @types.SchemaGraph = {
    type_nodes: encoder.nodes,
    defs: encoder.defs,
    root: first[0].value_type,
  }
  let config = @model.schema_graph_from_wit(config_graph)
  guard config.defs[0].body.body is Union(ordinary_branches) else {
    fail("ordinary union")
  }
  guard ordinary_branches[0].body.body is Record(ordinary_fields) else {
    fail("ordinary record")
  }
  assert_eq(ordinary_fields[0].name, "display_name")
  guard encoder.nodes[method_root].body is @types.RefType(method_def) else {
    fail("ordinary reference")
  }
  guard encoder.nodes[second[0].value_type].body is @types.RefType(second_def) else {
    fail("second config reference")
  }
  guard encoder.nodes[first[0].value_type].body is @types.RefType(first_def) else {
    fail("first config reference")
  }
  assert_eq(first_def, second_def)
  assert_true(method_def != first_def)
  let config_type = config.defs[first_def].body
  guard config_type.body is Union(branches) else { fail("config union") }
  assert_eq(
    branches[0].discriminator,
    @types.FieldEquals({
      field_name: "displayName",
      literal: Some("Exact_Value"),
    }),
  )
  assert_eq(branches[1].discriminator, @types.FieldAbsent("displayName"))
  guard branches[0].body.body is Record(fields) else { fail("config record") }
  assert_eq(fields[0].name, "displayName")
  assert_eq(
    fields[1].body.body,
    Option(@model.schema_type(Ref(config.defs[first_def].id))),
  )
  let declaration = local_entry(tag, ["tree"])
  assert_eq(declaration[0].value_type.defs[0].body, config_type)
  assert_eq(@schema.wire_schema(tag), ordinary)
}

///|
priv struct CollidingFixture {}

///|
impl @schema.WireSchema for CollidingFixture with fn append_wire_type(encoder) {
  let text = encoder.push(@types.StringType)
  encoder.push(
    @types.RecordType([
      {
        name: "max_chars",
        body: text,
        metadata: @schema.empty_wire_metadata(),
      },
      { name: "maxChars", body: text, metadata: @schema.empty_wire_metadata(), },
    ]),
  )
}

///|
test "panic config projection rejects colliding record names" {
  ignore(CollidingFixture::{ })
  ignore(
    wire_local_entry(
      @schema.WireTypeEncoder::new(),
      (@schema.type_tag() : @schema.TypeTag[CollidingFixture]),
      ["settings"],
    ),
  )
}
