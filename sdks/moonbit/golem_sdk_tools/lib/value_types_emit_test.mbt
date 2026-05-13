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
      #|"///|\nimpl @schema.HasElementSchema for TodoItem with element_schema() {\n  @schema.make_record_schema([\n    (\n      \"title\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),\n    ),\n    (\n      \"completed\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[Bool])),\n    ),\n  ])\n}\n\n///|\nimpl @schema.FromExtractor for TodoItem with from_extractor(e) {\n  let title :String = @schema.extract_field(e, 0)\n  let completed :Bool = @schema.extract_field(e, 1)\n  { title, completed }\n}\n\n///|\nimpl @schema.FromElementValue for TodoItem with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for TodoItem with to_element_value(self) {\n  @schema.make_record_value([\n    @schema.to_element_value_as(self.title),\n    @schema.to_element_value_as(self.completed),\n  ])\n}\n\n"

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
      #|"///|\nimpl @schema.HasElementSchema for Priority with element_schema() {\n  @schema.make_enum_schema([\"Low\", \"Medium\", \"High\"])\n}\n\n///|\nimpl @schema.FromExtractor for Priority with from_extractor(e) {\n  let idx = @schema.extract_enum(e)\n  match idx {\n    0 => Low\n    1 => Medium\n    2 => High\n    _ => {\n      raise  @common.AgentError::InvalidInput(\n        \"Unknown Priority case: \" + idx.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @schema.FromElementValue for Priority with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for Priority with to_element_value(self) {\n  match self {\n    Low => @schema.make_enum_value(0U)\n    Medium => @schema.make_enum_value(1U)\n    High => @schema.make_enum_value(2U)\n  }\n}\n\n"

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
      #|"///|\nimpl @schema.HasElementSchema for Shape with element_schema() {\n  @schema.make_variant_schema([\n    (\n      \"Circle\",\n      Some(\n        @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[Double])),\n      ),\n    ),\n    (\n      \"Rectangle\",\n      Some(\n        @schema.make_record_schema([\n          (\n            \"field0\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Double]),\n            ),\n          ),\n          (\n            \"field1\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Double]),\n            ),\n          ),\n        ]),\n      ),\n    ),\n    (\"Point\", None),\n  ])\n}\n\n///|\nimpl @schema.FromExtractor for Shape with from_extractor(e) {\n  let (idx, payload) = @schema.extract_variant(e)\n  match idx {\n    0 => {\n      guard payload is Some(p) else {\n        raise  \n          @common.AgentError::InvalidInput(\"Expected payload for Shape::Circle\")\n      }\n      let field0 :Double = @schema.from_extractor_as(p)\n      Circle(field0)\n    }\n    1 => {\n      guard payload is Some(p) else {\n        raise  @common.AgentError::InvalidInput(\n          \"Expected payload for Shape::Rectangle\",\n        )\n      }\n      let field0 :Double = @schema.extract_field(p, 0)\n      let field1 :Double = @schema.extract_field(p, 1)\n      Rectangle(field0, field1)\n    }\n    2 => Point\n    _ => raise  @common.AgentError::InvalidInput(\"Unknown Shape case\")\n  }\n}\n\n///|\nimpl @schema.FromElementValue for Shape with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for Shape with to_element_value(self) {\n  match self {\n    Circle(field0) => \n      @schema.make_variant_value(0U, Some(@schema.to_element_value_as(field0)))\n    Rectangle(field0, field1) => {\n      @schema.make_variant_value(\n        1U,\n        Some(\n          @schema.make_record_value([\n            @schema.to_element_value_as(field0),\n            @schema.to_element_value_as(field1),\n          ]),\n        ),\n      )\n    }\n    Point => @schema.make_variant_value(2U, None)\n  }\n}\n\n"

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
      #|"///|\nimpl @schema.HasElementSchema for Event with element_schema() {\n  @schema.make_variant_schema([\n    (\n      \"Created\",\n      Some(\n        @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),\n      ),\n    ),\n    (\n      \"Updated\",\n      Some(\n        @schema.make_record_schema([\n          (\n            \"title\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n          (\n            \"priority\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Int]),\n            ),\n          ),\n        ]),\n      ),\n    ),\n    (\"Deleted\", None),\n  ])\n}\n\n///|\nimpl @schema.FromExtractor for Event with from_extractor(e) {\n  let (idx, payload) = @schema.extract_variant(e)\n  match idx {\n    0 => {\n      guard payload is Some(p) else {\n        raise  @common.AgentError::InvalidInput(\n          \"Expected payload for Event::Created\",\n        )\n      }\n      let field0 :String = @schema.from_extractor_as(p)\n      Created(field0)\n    }\n    1 => {\n      guard payload is Some(p) else {\n        raise  @common.AgentError::InvalidInput(\n          \"Expected payload for Event::Updated\",\n        )\n      }\n      let title :String = @schema.extract_field(p, 0)\n      let priority :Int = @schema.extract_field(p, 1)\n      Updated(title~, priority~)\n    }\n    2 => Deleted\n    _ => raise  @common.AgentError::InvalidInput(\"Unknown Event case\")\n  }\n}\n\n///|\nimpl @schema.FromElementValue for Event with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for Event with to_element_value(self) {\n  match self {\n    Created(field0) => \n      @schema.make_variant_value(0U, Some(@schema.to_element_value_as(field0)))\n    Updated(title=title, priority=priority) => {\n      @schema.make_variant_value(\n        1U,\n        Some(\n          @schema.make_record_value([\n            @schema.to_element_value_as(title),\n            @schema.to_element_value_as(priority),\n          ]),\n        ),\n      )\n    }\n    Deleted => @schema.make_variant_value(2U, None)\n  }\n}\n\n"

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
      #|"///|\nimpl @schema.HasElementSchema for Order with element_schema() {\n  @schema.make_record_schema([\n    (\n      \"id\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[UInt64])),\n    ),\n    (\n      \"status\",\n      @schema.schema_of_tag(\n        (@schema.TypeTag::{  } : @schema.TypeTag[@types.OrderStatus]),\n      ),\n    ),\n    (\n      \"items\",\n      @schema.schema_of_tag(\n        (@schema.TypeTag::{  } : @schema.TypeTag[Array[@types.Item]]),\n      ),\n    ),\n    (\n      \"note\",\n      @schema.schema_of_tag(\n        (@schema.TypeTag::{  } : @schema.TypeTag[@types.Note?]),\n      ),\n    ),\n  ])\n}\n\n///|\nimpl @schema.FromExtractor for Order with from_extractor(e) {\n  let id :UInt64 = @schema.extract_field(e, 0)\n  let status :@types.OrderStatus = @schema.extract_field(e, 1)\n  let items :Array[@types.Item] = @schema.extract_field(e, 2)\n  let note :@types.Note? = @schema.extract_field(e, 3)\n  { id, status, items, note }\n}\n\n///|\nimpl @schema.FromElementValue for Order with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for Order with to_element_value(self) {\n  @schema.make_record_value([\n    @schema.to_element_value_as(self.id),\n    @schema.to_element_value_as(self.status),\n    @schema.to_element_value_as(self.items),\n    @schema.to_element_value_as(self.note),\n  ])\n}\n\n"

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
      #|"///|\nimpl @schema.HasElementSchema for Event with element_schema() {\n  @schema.make_variant_schema([\n    (\n      \"Created\",\n      Some(\n        @schema.schema_of_tag(\n          (@schema.TypeTag::{  } : @schema.TypeTag[@types.Info]),\n        ),\n      ),\n    ),\n    (\"Deleted\", None),\n  ])\n}\n\n///|\nimpl @schema.FromExtractor for Event with from_extractor(e) {\n  let (idx, payload) = @schema.extract_variant(e)\n  match idx {\n    0 => {\n      guard payload is Some(p) else {\n        raise  @common.AgentError::InvalidInput(\n          \"Expected payload for Event::Created\",\n        )\n      }\n      let field0 :@types.Info = @schema.from_extractor_as(p)\n      Created(field0)\n    }\n    1 => Deleted\n    _ => raise  @common.AgentError::InvalidInput(\"Unknown Event case\")\n  }\n}\n\n///|\nimpl @schema.FromElementValue for Event with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for Event with to_element_value(self) {\n  match self {\n    Created(field0) => \n      @schema.make_variant_value(0U, Some(@schema.to_element_value_as(field0)))\n    Deleted => @schema.make_variant_value(1U, None)\n  }\n}\n\n"

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
      #|"///|\nimpl @schema.HasElementSchema for UserRecord with element_schema() {\n  @schema.make_record_schema([\n    (\n      \"id\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[UserId])),\n    ),\n    (\n      \"name\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[UserName])),\n    ),\n    (\n      \"tags\",\n      @schema.schema_of_tag(\n        (@schema.TypeTag::{  } : @schema.TypeTag[Array[Tag]]),\n      ),\n    ),\n  ])\n}\n\n///|\nimpl @schema.FromExtractor for UserRecord with from_extractor(e) {\n  let id :UserId = @schema.extract_field(e, 0)\n  let name :UserName = @schema.extract_field(e, 1)\n  let tags :Array[Tag] = @schema.extract_field(e, 2)\n  { id, name, tags }\n}\n\n///|\nimpl @schema.FromElementValue for UserRecord with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for UserRecord with to_element_value(self) {\n  @schema.make_record_value([\n    @schema.to_element_value_as(self.id),\n    @schema.to_element_value_as(self.name),\n    @schema.to_element_value_as(self.tags),\n  ])\n}\n\n"

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
      #|"///|\nimpl @types.MultimodalModality for TextOrImage with multimodal_schema() {\n  [\n    (\n      \"Text\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),\n    ),\n    (\n      \"Image\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[Bytes])),\n    ),\n  ]\n}\n\n///|\nimpl @types.MultimodalModality for TextOrImage with to_named_element(self) {\n  match self {\n    Text(v) => (\"Text\", @schema.to_element_value_as(v))\n    Image(v) => (\"Image\", @schema.to_element_value_as(v))\n  }\n}\n\n///|\nimpl @types.MultimodalModality for TextOrImage with from_named_element(\n  name,\n  value,\n) {\n  match name {\n    \"Text\" => Text(@schema.from_element_value_as(value))\n    \"Image\" => Image(@schema.from_element_value_as(value))\n    _ => raise  @common.AgentError::InvalidInput(\"Unknown modality: \" + name)\n  }\n}\n\n"

    ),
  )
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
      #|"///|\nimpl @schema.HasElementSchema for Config with element_schema() {\n  @schema.make_record_schema([\n    (\n      \"name\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),\n    ),\n    (\n      \"tags\",\n      @schema.schema_of_tag(\n        (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),\n      ),\n    ),\n    (\n      \"description\",\n      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String?])),\n    ),\n  ])\n}\n\n///|\nimpl @schema.FromExtractor for Config with from_extractor(e) {\n  let name :String = @schema.extract_field(e, 0)\n  let tags :Array[String] = @schema.extract_field(e, 1)\n  let description :String? = @schema.extract_field(e, 2)\n  { name, tags, description }\n}\n\n///|\nimpl @schema.FromElementValue for Config with from_element_value(v) {\n  @schema.from_extractor_as(@extractor.extract_component_model_value(v))\n}\n\n///|\nimpl @schema.ToElementValue for Config with to_element_value(self) {\n  @schema.make_record_value([\n    @schema.to_element_value_as(self.name),\n    @schema.to_element_value_as(self.tags),\n    @schema.to_element_value_as(self.description),\n  ])\n}\n\n"

    ),
  )
}
