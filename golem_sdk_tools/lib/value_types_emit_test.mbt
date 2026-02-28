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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for TodoItem with element_schema() {
      #|  @schema.make_record_schema([
      #|    (
      #|      "title",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),
      #|    ),
      #|    (
      #|      "completed",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[Bool])),
      #|    ),
      #|  ])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for TodoItem with from_extractor(e) {
      #|  let title :String = @schema.extract_field(e, 0)
      #|  let completed :Bool = @schema.extract_field(e, 1)
      #|  { title, completed }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for TodoItem with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for TodoItem with to_element_value(self) {
      #|  @schema.make_record_value([
      #|    @schema.to_element_value_as(self.title),
      #|    @schema.to_element_value_as(self.completed),
      #|  ])
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for Priority with element_schema() {
      #|  @schema.make_enum_schema(["Low", "Medium", "High"])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for Priority with from_extractor(e) {
      #|  let idx = @schema.extract_enum(e)
      #|  match idx {
      #|    0 => Low
      #|    1 => Medium
      #|    2 => High
      #|    _ => {
      #|      raise  @common.AgentError::InvalidInput(
      #|        "Unknown Priority case: " + idx.to_string(),
      #|      )
      #|    }
      #|  }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for Priority with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for Priority with to_element_value(self) {
      #|  match self {
      #|    Low => @schema.make_enum_value(0U)
      #|    Medium => @schema.make_enum_value(1U)
      #|    High => @schema.make_enum_value(2U)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for Shape with element_schema() {
      #|  @schema.make_variant_schema([
      #|    (
      #|      "Circle",
      #|      Some(
      #|        @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[Double])),
      #|      ),
      #|    ),
      #|    (
      #|      "Rectangle",
      #|      Some(
      #|        @schema.make_record_schema([
      #|          (
      #|            "field0",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[Double]),
      #|            ),
      #|          ),
      #|          (
      #|            "field1",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[Double]),
      #|            ),
      #|          ),
      #|        ]),
      #|      ),
      #|    ),
      #|    ("Point", None),
      #|  ])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for Shape with from_extractor(e) {
      #|  let (idx, payload) = @schema.extract_variant(e)
      #|  match idx {
      #|    0 => {
      #|      guard payload is Some(p) else {
      #|        raise  
      #|          @common.AgentError::InvalidInput("Expected payload for Shape::Circle")
      #|      }
      #|      let field0 :Double = @schema.from_extractor_as(p)
      #|      Circle(field0)
      #|    }
      #|    1 => {
      #|      guard payload is Some(p) else {
      #|        raise  @common.AgentError::InvalidInput(
      #|          "Expected payload for Shape::Rectangle",
      #|        )
      #|      }
      #|      let field0 :Double = @schema.extract_field(p, 0)
      #|      let field1 :Double = @schema.extract_field(p, 1)
      #|      Rectangle(field0, field1)
      #|    }
      #|    2 => Point
      #|    _ => raise  @common.AgentError::InvalidInput("Unknown Shape case")
      #|  }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for Shape with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for Shape with to_element_value(self) {
      #|  match self {
      #|    Circle(field0) => 
      #|      @schema.make_variant_value(0U, Some(@schema.to_element_value_as(field0)))
      #|    Rectangle(field0, field1) => {
      #|      @schema.make_variant_value(
      #|        1U,
      #|        Some(
      #|          @schema.make_record_value([
      #|            @schema.to_element_value_as(field0),
      #|            @schema.to_element_value_as(field1),
      #|          ]),
      #|        ),
      #|      )
      #|    }
      #|    Point => @schema.make_variant_value(2U, None)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for Event with element_schema() {
      #|  @schema.make_variant_schema([
      #|    (
      #|      "Created",
      #|      Some(
      #|        @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),
      #|      ),
      #|    ),
      #|    (
      #|      "Updated",
      #|      Some(
      #|        @schema.make_record_schema([
      #|          (
      #|            "title",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|            ),
      #|          ),
      #|          (
      #|            "priority",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[Int]),
      #|            ),
      #|          ),
      #|        ]),
      #|      ),
      #|    ),
      #|    ("Deleted", None),
      #|  ])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for Event with from_extractor(e) {
      #|  let (idx, payload) = @schema.extract_variant(e)
      #|  match idx {
      #|    0 => {
      #|      guard payload is Some(p) else {
      #|        raise  @common.AgentError::InvalidInput(
      #|          "Expected payload for Event::Created",
      #|        )
      #|      }
      #|      let field0 :String = @schema.from_extractor_as(p)
      #|      Created(field0)
      #|    }
      #|    1 => {
      #|      guard payload is Some(p) else {
      #|        raise  @common.AgentError::InvalidInput(
      #|          "Expected payload for Event::Updated",
      #|        )
      #|      }
      #|      let title :String = @schema.extract_field(p, 0)
      #|      let priority :Int = @schema.extract_field(p, 1)
      #|      Updated(title~, priority~)
      #|    }
      #|    2 => Deleted
      #|    _ => raise  @common.AgentError::InvalidInput("Unknown Event case")
      #|  }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for Event with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for Event with to_element_value(self) {
      #|  match self {
      #|    Created(field0) => 
      #|      @schema.make_variant_value(0U, Some(@schema.to_element_value_as(field0)))
      #|    Updated(title=title, priority=priority) => {
      #|      @schema.make_variant_value(
      #|        1U,
      #|        Some(
      #|          @schema.make_record_value([
      #|            @schema.to_element_value_as(title),
      #|            @schema.to_element_value_as(priority),
      #|          ]),
      #|        ),
      #|      )
      #|    }
      #|    Deleted => @schema.make_variant_value(2U, None)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for Order with element_schema() {
      #|  @schema.make_record_schema([
      #|    (
      #|      "id",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[UInt64])),
      #|    ),
      #|    (
      #|      "status",
      #|      @schema.schema_of_tag(
      #|        (@schema.TypeTag::{  } : @schema.TypeTag[@types.OrderStatus]),
      #|      ),
      #|    ),
      #|    (
      #|      "items",
      #|      @schema.schema_of_tag(
      #|        (@schema.TypeTag::{  } : @schema.TypeTag[Array[@types.Item]]),
      #|      ),
      #|    ),
      #|    (
      #|      "note",
      #|      @schema.schema_of_tag(
      #|        (@schema.TypeTag::{  } : @schema.TypeTag[@types.Note?]),
      #|      ),
      #|    ),
      #|  ])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for Order with from_extractor(e) {
      #|  let id :UInt64 = @schema.extract_field(e, 0)
      #|  let status :@types.OrderStatus = @schema.extract_field(e, 1)
      #|  let items :Array[@types.Item] = @schema.extract_field(e, 2)
      #|  let note :@types.Note? = @schema.extract_field(e, 3)
      #|  { id, status, items, note }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for Order with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for Order with to_element_value(self) {
      #|  @schema.make_record_value([
      #|    @schema.to_element_value_as(self.id),
      #|    @schema.to_element_value_as(self.status),
      #|    @schema.to_element_value_as(self.items),
      #|    @schema.to_element_value_as(self.note),
      #|  ])
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for Event with element_schema() {
      #|  @schema.make_variant_schema([
      #|    (
      #|      "Created",
      #|      Some(
      #|        @schema.schema_of_tag(
      #|          (@schema.TypeTag::{  } : @schema.TypeTag[@types.Info]),
      #|        ),
      #|      ),
      #|    ),
      #|    ("Deleted", None),
      #|  ])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for Event with from_extractor(e) {
      #|  let (idx, payload) = @schema.extract_variant(e)
      #|  match idx {
      #|    0 => {
      #|      guard payload is Some(p) else {
      #|        raise  @common.AgentError::InvalidInput(
      #|          "Expected payload for Event::Created",
      #|        )
      #|      }
      #|      let field0 :@types.Info = @schema.from_extractor_as(p)
      #|      Created(field0)
      #|    }
      #|    1 => Deleted
      #|    _ => raise  @common.AgentError::InvalidInput("Unknown Event case")
      #|  }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for Event with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for Event with to_element_value(self) {
      #|  match self {
      #|    Created(field0) => 
      #|      @schema.make_variant_value(0U, Some(@schema.to_element_value_as(field0)))
      #|    Deleted => @schema.make_variant_value(1U, None)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for UserRecord with element_schema() {
      #|  @schema.make_record_schema([
      #|    (
      #|      "id",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[UserId])),
      #|    ),
      #|    (
      #|      "name",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[UserName])),
      #|    ),
      #|    (
      #|      "tags",
      #|      @schema.schema_of_tag(
      #|        (@schema.TypeTag::{  } : @schema.TypeTag[Array[Tag]]),
      #|      ),
      #|    ),
      #|  ])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for UserRecord with from_extractor(e) {
      #|  let id :UserId = @schema.extract_field(e, 0)
      #|  let name :UserName = @schema.extract_field(e, 1)
      #|  let tags :Array[Tag] = @schema.extract_field(e, 2)
      #|  { id, name, tags }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for UserRecord with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for UserRecord with to_element_value(self) {
      #|  @schema.make_record_value([
      #|    @schema.to_element_value_as(self.id),
      #|    @schema.to_element_value_as(self.name),
      #|    @schema.to_element_value_as(self.tags),
      #|  ])
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @types.MultimodalModality for TextOrImage with multimodal_schema() {
      #|  [
      #|    (
      #|      "Text",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),
      #|    ),
      #|    (
      #|      "Image",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[Bytes])),
      #|    ),
      #|  ]
      #|}
      #|
      #|///|
      #|impl @types.MultimodalModality for TextOrImage with to_named_element(self) {
      #|  match self {
      #|    Text(v) => ("Text", @schema.to_element_value_as(v))
      #|    Image(v) => ("Image", @schema.to_element_value_as(v))
      #|  }
      #|}
      #|
      #|///|
      #|impl @types.MultimodalModality for TextOrImage with from_named_element(
      #|  name,
      #|  value,
      #|) {
      #|  match name {
      #|    "Text" => Text(@schema.from_element_value_as(value))
      #|    "Image" => Image(@schema.from_element_value_as(value))
      #|    _ => raise  @common.AgentError::InvalidInput("Unknown modality: " + name)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|///|
      #|impl @schema.HasElementSchema for Config with element_schema() {
      #|  @schema.make_record_schema([
      #|    (
      #|      "name",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String])),
      #|    ),
      #|    (
      #|      "tags",
      #|      @schema.schema_of_tag(
      #|        (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),
      #|      ),
      #|    ),
      #|    (
      #|      "description",
      #|      @schema.schema_of_tag((@schema.TypeTag::{  } : @schema.TypeTag[String?])),
      #|    ),
      #|  ])
      #|}
      #|
      #|///|
      #|impl @schema.FromExtractor for Config with from_extractor(e) {
      #|  let name :String = @schema.extract_field(e, 0)
      #|  let tags :Array[String] = @schema.extract_field(e, 1)
      #|  let description :String? = @schema.extract_field(e, 2)
      #|  { name, tags, description }
      #|}
      #|
      #|///|
      #|impl @schema.FromElementValue for Config with from_element_value(v) {
      #|  @schema.from_extractor_as(@extractor.extract_component_model_value(v))
      #|}
      #|
      #|///|
      #|impl @schema.ToElementValue for Config with to_element_value(self) {
      #|  @schema.make_record_value([
      #|    @schema.to_element_value_as(self.name),
      #|    @schema.to_element_value_as(self.tags),
      #|    @schema.to_element_value_as(self.description),
      #|  ])
      #|}
      #|
      #|
    ),
  )
}
