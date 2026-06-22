///|
test "generate impls for simple struct" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "TodoItem",
      kind: Record([("title", Simple("String")), ("completed", Simple("Bool"))]),
    },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for TodoItem with type_id() {\n  \"TodoItem\"\n}\n\n///|\npub impl @schema.IntoSchema for TodoItem with register_in(b) {\n  b.register(\n    \"TodoItem\",\n    () => {\n      @model.schema_type(\n        Record([\n          @model.field(\n            \"title\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n          @model.field(\n            \"completed\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[Bool]),\n            ),\n          ),\n        ]),\n      )\n    },\n    Some(\"TodoItem\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for TodoItem with to_value(self) {\n  Record([@schema.to_value_as(self.title), @schema.to_value_as(self.completed)])\n}\n\n///|\npub impl @schema.FromSchema for TodoItem with from_value(v) {\n  let fields = @schema.expect_record(v, \"TodoItem\")\n  let title :String = \n    @schema.from_value_as(@schema.record_field(fields, 0, \"TodoItem\"))\n  let completed :Bool = \n    @schema.from_value_as(@schema.record_field(fields, 1, \"TodoItem\"))\n  { title, completed }\n}\n\n"
    ),
  )
}

///|
test "generate impls for simple enum" {
  let types : Array[ValueTypeInfo] = [
    { name: "Priority", kind: SimpleEnum(["Low", "Medium", "High"]) },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for Priority with type_id() {\n  \"Priority\"\n}\n\n///|\npub impl @schema.IntoSchema for Priority with register_in(b) {\n  b.register(\n    \"Priority\",\n    () => @model.schema_type(Enum([\"Low\", \"Medium\", \"High\"])),\n    Some(\"Priority\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for Priority with to_value(self) {\n  match self {\n    Low => Enum(0)\n    Medium => Enum(1)\n    High => Enum(2)\n  }\n}\n\n///|\npub impl @schema.FromSchema for Priority with from_value(v) {\n  let case = @schema.expect_enum(v, \"Priority\")\n  @schema.check_case_index(case, 3, \"Priority\")\n  match case {\n    0 => Low\n    1 => Medium\n    2 => High\n    _ => {\n      let err = \n        @schema.FromSchemaError::custom(\"unreachable enum case for Priority\")\n      raise  err\n    }\n  }\n}\n\n"
    ),
  )
}

///|
test "generate impls for variant enum" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "Shape",
      kind: VariantEnum([
        ("Circle", Single(Simple("Double"))),
        (
          "Rectangle",
          Multi(
            [("field0", Simple("Double")), ("field1", Simple("Double"))],
            false,
          ),
        ),
        ("Point", VariantPayload::None),
      ]),
    },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for Shape with type_id() {\n  \"Shape\"\n}\n\n///|\npub impl @schema.IntoSchema for Shape with register_in(b) {\n  b.register(\n    \"Shape\",\n    () => {\n      @model.schema_type(\n        Variant([\n          @model.variant_case(\n            \"Circle\",\n            payload=@schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[Double]),\n            ),\n          ),\n          @model.variant_case(\n            \"Rectangle\",\n            payload=@schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[(Double, Double)]),\n            ),\n          ),\n          @model.variant_case(\"Point\"),\n        ]),\n      )\n    },\n    Some(\"Shape\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for Shape with to_value(self) {\n  match self {\n    Circle(field0) => Variant(0, Some(@schema.to_value_as(field0)))\n    Rectangle(field0, field1) => \n      Variant(1, Some(@schema.to_value_as((field0, field1))))\n    Point => Variant(2, None)\n  }\n}\n\n///|\npub impl @schema.FromSchema for Shape with from_value(v) {\n  let (case, payload) = @schema.expect_variant(v, \"Shape\")\n  @schema.check_case_index(case, 3, \"Shape\")\n  match case {\n    0 => {\n      let p :Double = @schema.from_value_as(\n        @schema.expect_variant_payload(payload, \"Shape.Circle\"),\n      )\n      Circle(p)\n    }\n    1 => {\n      let (field0, field1) :(Double, Double) = @schema.from_value_as(\n        @schema.expect_variant_payload(payload, \"Shape.Rectangle\"),\n      )\n      Rectangle(field0, field1)\n    }\n    2 => {\n      @schema.expect_no_variant_payload(payload, \"Shape.Point\")\n      Point\n    }\n    _ => {\n      let err = \n        @schema.FromSchemaError::custom(\"unreachable variant case for Shape\")\n      raise  err\n    }\n  }\n}\n\n"
    ),
  )
}

///|
test "generate impls for variant enum with labelled fields" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "Event",
      kind: VariantEnum([
        ("Created", Single(Simple("String"))),
        (
          "Updated",
          Multi(
            [("title", Simple("String")), ("priority", Simple("Int"))],
            true,
          ),
        ),
        ("Deleted", VariantPayload::None),
      ]),
    },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for Event with type_id() {\n  \"Event\"\n}\n\n///|\npub impl @schema.IntoSchema for Event with register_in(b) {\n  b.register(\n    \"Event\",\n    () => {\n      @model.schema_type(\n        Variant([\n          @model.variant_case(\n            \"Created\",\n            payload=@schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n          @model.variant_case(\n            \"Updated\",\n            payload=@schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[(String, Int)]),\n            ),\n          ),\n          @model.variant_case(\"Deleted\"),\n        ]),\n      )\n    },\n    Some(\"Event\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for Event with to_value(self) {\n  match self {\n    Created(field0) => Variant(0, Some(@schema.to_value_as(field0)))\n    Updated(title~, priority~) => \n      Variant(1, Some(@schema.to_value_as((title, priority))))\n    Deleted => Variant(2, None)\n  }\n}\n\n///|\npub impl @schema.FromSchema for Event with from_value(v) {\n  let (case, payload) = @schema.expect_variant(v, \"Event\")\n  @schema.check_case_index(case, 3, \"Event\")\n  match case {\n    0 => {\n      let p :String = @schema.from_value_as(\n        @schema.expect_variant_payload(payload, \"Event.Created\"),\n      )\n      Created(p)\n    }\n    1 => {\n      let (title, priority) :(String, Int) = @schema.from_value_as(\n        @schema.expect_variant_payload(payload, \"Event.Updated\"),\n      )\n      Updated(title~, priority~)\n    }\n    2 => {\n      @schema.expect_no_variant_payload(payload, \"Event.Deleted\")\n      Deleted\n    }\n    _ => {\n      let err = \n        @schema.FromSchemaError::custom(\"unreachable variant case for Event\")\n      raise  err\n    }\n  }\n}\n\n"
    ),
  )
}

///|
test "generate impls for struct with cross-package field types" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "Order",
      kind: Record([
        ("id", Simple("UInt64")),
        ("status", Simple("@types.OrderStatus")),
        ("items", List(Simple("@types.Item"))),
        ("note", Optional(Simple("@types.Note"))),
      ]),
    },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for Order with type_id() {\n  \"Order\"\n}\n\n///|\npub impl @schema.IntoSchema for Order with register_in(b) {\n  b.register(\n    \"Order\",\n    () => {\n      @model.schema_type(\n        Record([\n          @model.field(\n            \"id\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          @model.field(\n            \"status\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[@types.OrderStatus]),\n            ),\n          ),\n          @model.field(\n            \"items\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[Array[@types.Item]]),\n            ),\n          ),\n          @model.field(\n            \"note\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[@types.Note?]),\n            ),\n          ),\n        ]),\n      )\n    },\n    Some(\"Order\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for Order with to_value(self) {\n  Record([\n    @schema.to_value_as(self.id),\n    @schema.to_value_as(self.status),\n    @schema.to_value_as(self.items),\n    @schema.to_value_as(self.note),\n  ])\n}\n\n///|\npub impl @schema.FromSchema for Order with from_value(v) {\n  let fields = @schema.expect_record(v, \"Order\")\n  let id :UInt64 = \n    @schema.from_value_as(@schema.record_field(fields, 0, \"Order\"))\n  let status :@types.OrderStatus = \n    @schema.from_value_as(@schema.record_field(fields, 1, \"Order\"))\n  let items :Array[@types.Item] = \n    @schema.from_value_as(@schema.record_field(fields, 2, \"Order\"))\n  let note :@types.Note? = \n    @schema.from_value_as(@schema.record_field(fields, 3, \"Order\"))\n  { id, status, items, note }\n}\n\n"
    ),
  )
}

///|
test "generate impls for variant enum with cross-package payload" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "Event",
      kind: VariantEnum([
        ("Created", Single(Simple("@types.Info"))),
        ("Deleted", VariantPayload::None),
      ]),
    },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for Event with type_id() {\n  \"Event\"\n}\n\n///|\npub impl @schema.IntoSchema for Event with register_in(b) {\n  b.register(\n    \"Event\",\n    () => {\n      @model.schema_type(\n        Variant([\n          @model.variant_case(\n            \"Created\",\n            payload=@schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[@types.Info]),\n            ),\n          ),\n          @model.variant_case(\"Deleted\"),\n        ]),\n      )\n    },\n    Some(\"Event\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for Event with to_value(self) {\n  match self {\n    Created(field0) => Variant(0, Some(@schema.to_value_as(field0)))\n    Deleted => Variant(1, None)\n  }\n}\n\n///|\npub impl @schema.FromSchema for Event with from_value(v) {\n  let (case, payload) = @schema.expect_variant(v, \"Event\")\n  @schema.check_case_index(case, 2, \"Event\")\n  match case {\n    0 => {\n      let p :@types.Info = @schema.from_value_as(\n        @schema.expect_variant_payload(payload, \"Event.Created\"),\n      )\n      Created(p)\n    }\n    1 => {\n      @schema.expect_no_variant_payload(payload, \"Event.Deleted\")\n      Deleted\n    }\n    _ => {\n      let err = \n        @schema.FromSchemaError::custom(\"unreachable variant case for Event\")\n      raise  err\n    }\n  }\n}\n\n"
    ),
  )
}

///|
test "generate impls for struct with type alias fields" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "UserRecord",
      kind: Record([
        ("id", Simple("UserId")),
        ("name", Simple("UserName")),
        ("tags", List(Simple("Tag"))),
      ]),
    },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for UserRecord with type_id() {\n  \"UserRecord\"\n}\n\n///|\npub impl @schema.IntoSchema for UserRecord with register_in(b) {\n  b.register(\n    \"UserRecord\",\n    () => {\n      @model.schema_type(\n        Record([\n          @model.field(\n            \"id\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[UserId]),\n            ),\n          ),\n          @model.field(\n            \"name\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[UserName]),\n            ),\n          ),\n          @model.field(\n            \"tags\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[Array[Tag]]),\n            ),\n          ),\n        ]),\n      )\n    },\n    Some(\"UserRecord\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for UserRecord with to_value(self) {\n  Record([\n    @schema.to_value_as(self.id),\n    @schema.to_value_as(self.name),\n    @schema.to_value_as(self.tags),\n  ])\n}\n\n///|\npub impl @schema.FromSchema for UserRecord with from_value(v) {\n  let fields = @schema.expect_record(v, \"UserRecord\")\n  let id :UserId = \n    @schema.from_value_as(@schema.record_field(fields, 0, \"UserRecord\"))\n  let name :UserName = \n    @schema.from_value_as(@schema.record_field(fields, 1, \"UserRecord\"))\n  let tags :Array[Tag] = \n    @schema.from_value_as(@schema.record_field(fields, 2, \"UserRecord\"))\n  { id, name, tags }\n}\n\n"
    ),
  )
}

///|
test "generate multimodal impls for TextOrImage" {
  let enums : Array[MultimodalEnumInfo] = [
    {
      name: "TextOrImage",
      variants: [("Text", Simple("String")), ("Image", Simple("Bytes"))],
    },
  ]
  let impls = generate_multimodal_impls(enums)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @multimodal.MultimodalModality for TextOrImage with multimodal_type_id(\n  \n) {\n  \"TextOrImage\"\n}\n\n///|\npub impl @multimodal.MultimodalModality for TextOrImage with multimodal_cases(b) {\n  [\n    @model.variant_case(\n      \"Text\",\n      payload=@schema.register_in_with(\n        b,\n        (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n      ),\n    ),\n    @model.variant_case(\n      \"Image\",\n      payload=@schema.register_in_with(\n        b,\n        (@schema.TypeTag::{  } : @schema.TypeTag[Bytes]),\n      ),\n    ),\n  ]\n}\n\n///|\npub impl @multimodal.MultimodalModality for TextOrImage with to_modality_value(\n  self,\n) {\n  match self {\n    Text(v) => (0U, @schema.to_value_as(v))\n    Image(v) => (1U, @schema.to_value_as(v))\n  }\n}\n\n///|\npub impl @multimodal.MultimodalModality for TextOrImage with from_modality_value(\n  case,\n  payload,\n) {\n  match case {\n    0 => Text(@schema.from_value_as(payload))\n    1 => Image(@schema.from_value_as(payload))\n    other => {\n      let err = @schema.FromSchemaError::out_of_range(other, 2U, \"TextOrImage\")\n      raise  err\n    }\n  }\n}\n\n"
    ),
  )
}

///|
test "multimodal type_id uses package-qualified prefix when provided" {
  let enums : Array[MultimodalEnumInfo] = [
    {
      name: "TextOrImage",
      variants: [("Text", Simple("String")), ("Image", Simple("Bytes"))],
    },
  ]
  let impls = generate_multimodal_impls(
    enums,
    type_id_prefix="golem_moonbit_examples",
  )
  let output = @formatter.impls_to_string(impls)
  // The modality id is package-qualified; the bare name must not leak.
  assert_true(output.contains("\"golem_moonbit_examples.TextOrImage\""))
  assert_false(output.contains("multimodal_type_id() {\n  \"TextOrImage\"\n}"))
}

///|
test "generate impls for struct with compound fields" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "Config",
      kind: Record([
        ("name", Simple("String")),
        ("tags", List(Simple("String"))),
        ("description", Optional(Simple("String"))),
      ]),
    },
  ]
  let impls = generate_value_type_impls(types)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for Config with type_id() {\n  \"Config\"\n}\n\n///|\npub impl @schema.IntoSchema for Config with register_in(b) {\n  b.register(\n    \"Config\",\n    () => {\n      @model.schema_type(\n        Record([\n          @model.field(\n            \"name\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n          @model.field(\n            \"tags\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),\n            ),\n          ),\n          @model.field(\n            \"description\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[String?]),\n            ),\n          ),\n        ]),\n      )\n    },\n    Some(\"Config\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for Config with to_value(self) {\n  Record([\n    @schema.to_value_as(self.name),\n    @schema.to_value_as(self.tags),\n    @schema.to_value_as(self.description),\n  ])\n}\n\n///|\npub impl @schema.FromSchema for Config with from_value(v) {\n  let fields = @schema.expect_record(v, \"Config\")\n  let name :String = \n    @schema.from_value_as(@schema.record_field(fields, 0, \"Config\"))\n  let tags :Array[String] = \n    @schema.from_value_as(@schema.record_field(fields, 1, \"Config\"))\n  let description :String? = \n    @schema.from_value_as(@schema.record_field(fields, 2, \"Config\"))\n  { name, tags, description }\n}\n\n"
    ),
  )
}

///|
test "type_id uses package-qualified prefix when provided" {
  let types : Array[ValueTypeInfo] = [
    {
      name: "TodoItem",
      kind: Record([("title", Simple("String")), ("completed", Simple("Bool"))]),
    },
  ]
  let impls = generate_value_type_impls(
    types,
    type_id_prefix="golem_moonbit_examples",
  )
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\npub impl @schema.IntoSchema for TodoItem with type_id() {\n  \"golem_moonbit_examples.TodoItem\"\n}\n\n///|\npub impl @schema.IntoSchema for TodoItem with register_in(b) {\n  b.register(\n    \"golem_moonbit_examples.TodoItem\",\n    () => {\n      @model.schema_type(\n        Record([\n          @model.field(\n            \"title\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n          @model.field(\n            \"completed\",\n            @schema.register_in_with(\n              b,\n              (@schema.TypeTag::{  } : @schema.TypeTag[Bool]),\n            ),\n          ),\n        ]),\n      )\n    },\n    Some(\"golem_moonbit_examples.TodoItem\"),\n  )\n}\n\n///|\npub impl @schema.IntoSchema for TodoItem with to_value(self) {\n  Record([@schema.to_value_as(self.title), @schema.to_value_as(self.completed)])\n}\n\n///|\npub impl @schema.FromSchema for TodoItem with from_value(v) {\n  let fields = @schema.expect_record(v, \"golem_moonbit_examples.TodoItem\")\n  let title :String = @schema.from_value_as(\n    @schema.record_field(fields, 0, \"golem_moonbit_examples.TodoItem\"),\n  )\n  let completed :Bool = @schema.from_value_as(\n    @schema.record_field(fields, 1, \"golem_moonbit_examples.TodoItem\"),\n  )\n  { title, completed }\n}\n\n"
    ),
  )
}
