// Round-trip tests for nominal user types (record / simple-enum / variant /
// recursive), with hand-written `IntoSchema` / `FromSchema` impls that mirror
// exactly what the Phase 4 codegen will emit:
//
//   - records use a reserved/committed named definition whose body is a
//     `Record` of `field(...)`s; `to_value` emits a positional `Record`; decode
//     uses `expect_record` + `record_field` + `from_value_as`.
//   - simple enums use an `Enum([names])` body; decode uses `expect_enum` +
//     `check_case_index`.
//   - variants use a `Variant([variant_case(...)])` body; decode uses
//     `expect_variant` + `check_case_index` + `expect_variant_payload`.
//   - generic field/element types are registered via `register_in_with` with a
//     parenthesized `TypeTag` ascription (the codegen-facing pattern).

///|
/// Full encode -> WIT -> decode round-trip (shared with schema_test via copy to
/// keep each test file self-contained as a blackbox).
fn[T : IntoSchema + FromSchema] rt(v : T) -> T raise {
  let typed = try_into_typed_schema_value(v)
  let wit = @model.typed_schema_value_to_wit(typed)
  let back = @model.typed_schema_value_from_wit(wit)
  assert_eq(back.graph, typed.graph)
  T::from_value(back.value)
}

// ============================================================
// Record: Point { x, y }
// ============================================================

///|
struct Point {
  x : Int
  y : Int
} derive(Eq, Show)

///|
pub impl IntoSchema for Point with type_id() {
  "test.Point"
}

///|
pub impl IntoSchema for Point with register_in(b) {
  b.register(
    "test.Point",
    () => {
      @model.schema_type(
        Record([
          @model.field("x", register_in_with(b, (TypeTag::{  } : TypeTag[Int]))),
          @model.field("y", register_in_with(b, (TypeTag::{  } : TypeTag[Int]))),
        ]),
      )
    },
    Some("Point"),
  )
}

///|
pub impl IntoSchema for Point with to_value(self) {
  Record([to_value_as(self.x), to_value_as(self.y)])
}

///|
pub impl FromSchema for Point with from_value(v) {
  let fields = expect_record(v, "Point")
  let x : Int = from_value_as(record_field(fields, 0, "Point"))
  let y : Int = from_value_as(record_field(fields, 1, "Point"))
  { x, y }
}

///|
test "record round-trip + named def" {
  let p = { x: 3, y: -4 }
  assert_eq(rt(p), p)
  let tag : TypeTag[Point] = type_tag()
  let g = into_schema_graph(tag)
  assert_eq(g.defs.length(), 1)
  assert_eq(g.defs[0].id, "test.Point")
  assert_eq(g.defs[0].name, Some("Point"))
}

// ============================================================
// Simple enum: Color { Red, Green, Blue }
// ============================================================

///|
enum Color {
  Red
  Green
  Blue
} derive(Eq, Show)

///|
pub impl IntoSchema for Color with type_id() {
  "test.Color"
}

///|
pub impl IntoSchema for Color with register_in(b) {
  b.register(
    "test.Color",
    () => @model.schema_type(Enum(["Red", "Green", "Blue"])),
    Some("Color"),
  )
}

///|
pub impl IntoSchema for Color with to_value(self) {
  match self {
    Red => Enum(0)
    Green => Enum(1)
    Blue => Enum(2)
  }
}

///|
pub impl FromSchema for Color with from_value(v) {
  let case = expect_enum(v, "Color")
  check_case_index(case, 3, "Color")
  match case {
    0 => Red
    1 => Green
    2 => Blue
    _ => raise FromSchemaError::custom("unreachable enum case for Color")
  }
}

///|
test "simple enum round-trip" {
  assert_eq(rt(Red), Red)
  assert_eq(rt(Green), Green)
  assert_eq(rt(Blue), Blue)
}

///|
test "enum out-of-range decode raises OutOfRange" {
  let v : @model.SchemaValue = Enum(9)
  let r : Result[Color, FromSchemaError] = try? from_value_as(v)
  guard r is Err(OutOfRange(index~, len~, context~)) else {
    fail("expected OutOfRange, got \{r}")
  }
  assert_eq(index, 9)
  assert_eq(len, 3)
  assert_eq(context, "Color")
}

// ============================================================
// Variant: Shape { Empty, Circle(Double), Rect(Int, String) }
// ============================================================

///|
enum Shape {
  Empty
  Circle(Double)
  Rect(Int, String)
} derive(Eq, Show)

///|
pub impl IntoSchema for Shape with type_id() {
  "test.Shape"
}

///|
pub impl IntoSchema for Shape with register_in(b) {
  b.register(
    "test.Shape",
    () => {
      @model.schema_type(
        Variant([
          @model.variant_case("Empty"),
          @model.variant_case(
            "Circle",
            payload=register_in_with(b, (TypeTag::{  } : TypeTag[Double])),
          ),
          @model.variant_case(
            "Rect",
            payload=register_in_with(
              b,
              (TypeTag::{  } : TypeTag[(Int, String)]),
            ),
          ),
        ]),
      )
    },
    Some("Shape"),
  )
}

///|
pub impl IntoSchema for Shape with to_value(self) {
  match self {
    Empty => Variant(0, None)
    Circle(d) => Variant(1, Some(to_value_as(d)))
    Rect(w, h) => Variant(2, Some(to_value_as((w, h))))
  }
}

///|
pub impl FromSchema for Shape with from_value(v) {
  let (case, payload) = expect_variant(v, "Shape")
  check_case_index(case, 3, "Shape")
  match case {
    0 => {
      expect_no_variant_payload(payload, "Shape.Empty")
      Empty
    }
    1 => Circle(from_value_as(expect_variant_payload(payload, "Shape.Circle")))
    2 => {
      let (w, h) : (Int, String) = from_value_as(
        expect_variant_payload(payload, "Shape.Rect"),
      )
      Rect(w, h)
    }
    _ => raise FromSchemaError::custom("unreachable variant case for Shape")
  }
}

///|
test "variant round-trip" {
  assert_eq(rt(Empty), Empty)
  assert_eq(rt(Circle(2.5)), Circle(2.5))
  assert_eq(rt(Rect(3, "wide")), Rect(3, "wide"))
}

///|
test "unit variant case rejects an unexpected payload" {
  let v : @model.SchemaValue = Variant(0, Some(String("oops")))
  let r : Result[Shape, FromSchemaError] = try? from_value_as(v)
  guard r is Err(Custom(msg)) else { fail("expected Custom, got \{r}") }
  assert_true(msg.contains("expected no payload"))
}

// ============================================================
// Generic nominal record: Boxed[T] { inner: T }
// ============================================================

///|
struct Boxed[T] {
  inner : T
} derive(Eq, Show)

///|
pub impl[T : IntoSchema] IntoSchema for Boxed[T] with type_id() {
  type_id_with_args("test.Boxed", [T::type_id()])
}

///|
pub impl[T : IntoSchema] IntoSchema for Boxed[T] with register_in(b) {
  b.register(
    type_id_with_args("test.Boxed", [T::type_id()]),
    () => {
      @model.schema_type(
        Record([
          @model.field(
            "inner",
            register_in_with(b, (TypeTag::{  } : TypeTag[T])),
          ),
        ]),
      )
    },
    Some("Boxed"),
  )
}

///|
pub impl[T : IntoSchema] IntoSchema for Boxed[T] with to_value(self) {
  Record([to_value_as(self.inner)])
}

///|
pub impl[T : FromSchema] FromSchema for Boxed[T] with from_value(v) {
  let fields = expect_record(v, "Boxed")
  let inner : T = from_value_as(record_field(fields, 0, "Boxed"))
  { inner, }
}

///|
test "generic nominal record round-trip + per-instantiation def id" {
  let bi : Boxed[Int] = { inner: 42 }
  assert_eq(rt(bi), bi)
  let bs : Boxed[String] = { inner: "hi" }
  assert_eq(rt(bs), bs)
  // Each instantiation gets its own definition keyed by the resolved args.
  let ti : TypeTag[Boxed[Int]] = type_tag()
  let gi = into_schema_graph(ti)
  assert_eq(gi.defs.length(), 1)
  assert_eq(gi.defs[0].id, "test.Boxed<i32>")
  let ts : TypeTag[Boxed[String]] = type_tag()
  let gs = into_schema_graph(ts)
  assert_eq(gs.defs[0].id, "test.Boxed<alloc.string.String>")
}

// ============================================================
// Recursive record: Tree { value, children: Array[Tree] }
// ============================================================

///|
struct Tree {
  value : Int
  children : Array[Tree]
} derive(Eq, Show)

///|
pub impl IntoSchema for Tree with type_id() {
  "test.Tree"
}

///|
pub impl IntoSchema for Tree with register_in(b) {
  b.register(
    "test.Tree",
    () => {
      @model.schema_type(
        Record([
          @model.field(
            "value",
            register_in_with(b, (TypeTag::{  } : TypeTag[Int])),
          ),
          @model.field(
            "children",
            register_in_with(b, (TypeTag::{  } : TypeTag[Array[Tree]])),
          ),
        ]),
      )
    },
    Some("Tree"),
  )
}

///|
pub impl IntoSchema for Tree with to_value(self) {
  Record([to_value_as(self.value), to_value_as(self.children)])
}

///|
pub impl FromSchema for Tree with from_value(v) {
  let fields = expect_record(v, "Tree")
  let value : Int = from_value_as(record_field(fields, 0, "Tree"))
  let children : Array[Tree] = from_value_as(record_field(fields, 1, "Tree"))
  { value, children }
}

///|
test "recursive record round-trip + single def via reserve/commit" {
  let t = {
    value: 1,
    children: [
      { value: 2, children: [] },
      { value: 3, children: [{ value: 4, children: [] }] },
    ],
  }
  assert_eq(rt(t), t)
  // The recursive type closes through a single named definition.
  let tag : TypeTag[Tree] = type_tag()
  let g = into_schema_graph(tag)
  assert_eq(g.defs.length(), 1)
  assert_eq(g.defs[0].id, "test.Tree")
}

// ============================================================
// into_schema_graph wires in well-formedness validation (Phase 2b)
// ============================================================

///|
/// A type whose `register_in` deliberately produces a malformed graph (an empty
/// enum) so we can assert `into_schema_graph` validates and raises.
struct BadEnum {} derive(Eq, Show)

///|
pub impl IntoSchema for BadEnum with type_id() {
  "test.BadEnum"
}

///|
pub impl IntoSchema for BadEnum with register_in(b) {
  b.register(
    "test.BadEnum",
    () => @model.schema_type(Enum([])),
    Some("BadEnum"),
  )
}

///|
pub impl IntoSchema for BadEnum with to_value(_self) {
  Enum(0)
}

///|
test "into_schema_graph raises the first well-formedness error" {
  let tag : TypeTag[BadEnum] = type_tag()
  let raised = try {
    let _ = into_schema_graph(tag)
    "no error"
  } catch {
    @model.EmptyEnum => "EmptyEnum"
    e => "other: \{e}"
  }
  inspect(raised, content="EmptyEnum")
}
