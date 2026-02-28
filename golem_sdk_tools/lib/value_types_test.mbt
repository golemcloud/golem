///|
test "parse struct with primitive fields" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) struct Point {
    #|  x : Double
    #|  y : Double
    #|}
  let types = parse_value_types([("point.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Point")
  inspect(
    types[0].kind,
    content="Record([(\"x\", Simple(\"Double\")), (\"y\", Simple(\"Double\"))])",
  )
}

///|
test "parse struct with compound fields" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) struct Config {
    #|  name : String
    #|  tags : Array[String]
    #|  description : String?
    #|}
  let types = parse_value_types([("config.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Config")
  inspect(
    types[0].kind,
    content="Record([(\"name\", Simple(\"String\")), (\"tags\", List(Simple(\"String\"))), (\"description\", Optional(Simple(\"String\")))])",
  )
}

///|
test "parse simple enum (all unit cases)" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) enum Priority {
    #|  Low
    #|  Medium
    #|  High
    #|}
  let types = parse_value_types([("priority.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Priority")
  inspect(types[0].kind, content="SimpleEnum([\"Low\", \"Medium\", \"High\"])")
}

///|
test "parse enum with mixed unit and payload cases" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) enum Shape {
    #|  Circle(Double)
    #|  Square(Double)
    #|  None_
    #|}
  let types = parse_value_types([("shape.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Shape")
  inspect(
    types[0].kind,
    content="VariantEnum([(\"Circle\", Single(Simple(\"Double\"))), (\"Square\", Single(Simple(\"Double\"))), (\"None_\", None)])",
  )
}

///|
test "parse enum with multi-field positional case" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) enum Shape {
    #|  Circle(Double)
    #|  Rectangle(Double, Double)
    #|}
  let types = parse_value_types([("shape.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Shape")
  inspect(
    types[0].kind,
    content=(
      #|VariantEnum([("Circle", Single(Simple("Double"))), ("Rectangle", Multi([("field0", Simple("Double")), ("field1", Simple("Double"))], false))])
    ),
  )
}

///|
test "parse enum with labelled fields" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) enum Shape {
    #|  Circle(radius~ : Double)
    #|  Rect(width~ : Double, height~ : Double)
    #|}
  let types = parse_value_types([("shape.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Shape")
  inspect(
    types[0].kind,
    content=(
      #|VariantEnum([("Circle", Multi([("radius", Simple("Double"))], true)), ("Rect", Multi([("width", Simple("Double")), ("height", Simple("Double"))], true))])
    ),
  )
}

///|
test "multiple types in one file" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) struct Point {
    #|  x : Double
    #|  y : Double
    #|}
    #|
    #|///|
    #|#derive.golem_schema
    #|pub(all) enum Color {
    #|  Red
    #|  Green
    #|  Blue
    #|}
  let types = parse_value_types([("types.mbt", content)])
  inspect(types.length(), content="2")
  inspect(types[0].name, content="Point")
  inspect(
    types[0].kind,
    content="Record([(\"x\", Simple(\"Double\")), (\"y\", Simple(\"Double\"))])",
  )
  inspect(types[1].name, content="Color")
  inspect(types[1].kind, content="SimpleEnum([\"Red\", \"Green\", \"Blue\"])")
}

///|
test "types across multiple files" {
  let file1 =
    #|///|
    #|#derive.golem_schema
    #|pub(all) struct Point {
    #|  x : Double
    #|  y : Double
    #|}
  let file2 =
    #|///|
    #|#derive.golem_schema
    #|pub(all) enum Direction {
    #|  North
    #|  South
    #|  East
    #|  West
    #|}
  let types = parse_value_types([("point.mbt", file1), ("direction.mbt", file2)])
  inspect(types.length(), content="2")
  inspect(types[0].name, content="Point")
  inspect(
    types[0].kind,
    content="Record([(\"x\", Simple(\"Double\")), (\"y\", Simple(\"Double\"))])",
  )
  inspect(types[1].name, content="Direction")
  inspect(
    types[1].kind,
    content="SimpleEnum([\"North\", \"South\", \"East\", \"West\"])",
  )
}

///|
test "parse struct with tuple field" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) struct PairHolder {
    #|  pair : (String, Int)
    #|  triple : (String, Int, Bool)
    #|}
  let types = parse_value_types([("pair.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="PairHolder")
  inspect(
    types[0].kind,
    content=(
      #|Record([("pair", Tuple([Simple("String"), Simple("Int")])), ("triple", Tuple([Simple("String"), Simple("Int"), Simple("Bool")]))])
    ),
  )
}

///|
test "parse struct with cross-package type field" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) struct Order {
    #|  id : UInt64
    #|  status : @types.OrderStatus
    #|  items : Array[@types.Item]
    #|  note : @types.Note?
    #|}
  let types = parse_value_types([("order.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Order")
  inspect(
    types[0].kind,
    content="Record([(\"id\", Simple(\"UInt64\")), (\"status\", Simple(\"@types.OrderStatus\")), (\"items\", List(Simple(\"@types.Item\"))), (\"note\", Optional(Simple(\"@types.Note\")))])",
  )
}

///|
test "parse variant enum with cross-package payload" {
  let content =
    #|///|
    #|#derive.golem_schema
    #|pub(all) enum Event {
    #|  Created(@types.Info)
    #|  Updated(@types.Info, @types.Metadata)
    #|  Deleted
    #|}
  let types = parse_value_types([("event.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Event")
  inspect(
    types[0].kind,
    content=(
      #|VariantEnum([("Created", Single(Simple("@types.Info"))), ("Updated", Multi([("field0", Simple("@types.Info")), ("field1", Simple("@types.Metadata"))], false)), ("Deleted", None)])
    ),
  )
}

///|
test "non-annotated types are skipped" {
  let content =
    #|///|
    #|pub(all) struct Internal {
    #|  x : Int
    #|}
    #|
    #|///|
    #|#derive.golem_schema
    #|pub(all) struct Public {
    #|  name : String
    #|}
    #|
    #|///|
    #|pub(all) enum Status {
    #|  Active
    #|  Inactive
    #|}
  let types = parse_value_types([("mixed.mbt", content)])
  inspect(types.length(), content="1")
  inspect(types[0].name, content="Public")
  inspect(types[0].kind, content="Record([(\"name\", Simple(\"String\"))])")
}
