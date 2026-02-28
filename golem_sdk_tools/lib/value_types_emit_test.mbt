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
    {
      name: "Priority",
      kind: SimpleEnum(["Low", "Medium", "High"]),
    },
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
          Multi([("field0", Simple("Double")), ("field1", Simple("Double"))]),
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
