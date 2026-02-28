///|
struct A(Int) derive(Show, ToJson, FromJson)

///|
suberror E derive(Show, ToJson, FromJson)

///|
suberror EI {
  EI(Int)
} derive(Show, ToJson, FromJson)

///|
suberror ConnectionError {
  BrokenPipe(Int, String)
  ConnectionReset
} derive(Show, ToJson, FromJson)

///|
struct S {
  x : Int
  y : (Int, Bool)
  z : String
} derive(Show, ToJson, FromJson)

///|
struct Nested1 {
  x : Int
  y : Int
} derive(Show, ToJson, FromJson)

///|
struct Nested2 {
  x : Nested1
  y : Int
} derive(Show, ToJson, FromJson)

///|
struct Nested3 {
  x : Nested2
  y : Int
} derive(Show, ToJson, FromJson)

///|
enum ESmall {
  C1
} derive(Show, ToJson, FromJson)

///|
enum ESmall2 {
  C1(Int, Bool)
} derive(Show, ToJson, FromJson)

///|
enum EE {
  C1
  C2
} derive(Show, ToJson, FromJson)

///|
enum EEE {
  C1(Int, Bool)
  C2(S)
} derive(Show, ToJson, FromJson)

///|
struct T1[X] {
  x : Nested3
  y : EEE
  z : EE
  w : X
  q : E
  c : ConnectionError
} derive(Show, ToJson, FromJson)

///|
struct NT[X](Array[X]) derive(Show, ToJson, FromJson)

///|
enum E0 {
  C0
  C1(S)
  C2(x~ : Int, String, y~ : Double)
  C4(a~ : Bool, (Double, String))
  C3((Int, Int))
  C5(((Int, Int), Int), Int)
} derive(Show, ToJson, FromJson)

///|
fn testErrEnum(j : Json) -> Result[ConnectionError, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testE0(j : Json) -> Result[E0, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testT1(j : Json) -> Result[T1[Array[Int]], @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testNT(j : Json) -> Result[NT[T1[Array[Int]]], @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testEE(j : Json) -> Result[EE, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testS(j : Json) -> Result[S, @json.JsonDecodeError] {
  // let serialized : S = @json.from_json!(j)
  try? @json.from_json(j)
}

///|
fn testEEE(j : Json) -> Result[EEE, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testA(j : Json) -> Result[A, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testE(j : Json) -> Result[E, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testEI(j : Json) -> Result[EI, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn testNested(j : Json) -> Result[Nested3, @json.JsonDecodeError] {
  try? @json.from_json(j)
}

///|
fn init {
  let ee = EE::C2
  println(testEE(ee.to_json()))
  let s = { x: 42, y: (42, true), z: "str" }
  println(testS(s.to_json()))
  let eee = EEE::C2(s)
  println(testEEE(eee.to_json()))
  let a = A(42)
  println(testA(a.to_json()))
  let e = E
  println(testE(e.to_json()))
  let ei = EI(42)
  println(testEI(ei.to_json()))
  let e0 = E0::C1(s)
  println(testE0(e0.to_json()))
  let e0 = E0::C5(((1, 2), 3), 4)
  println(testE0(e0.to_json()))
  let esmall = ESmall::C1
  println(testEE(esmall.to_json()))
  let esmall2 = ESmall2::C1(1, true)
  println(testEEE(esmall2.to_json()))
  let e0 = E0::C2(x=1, "str", y=2)
  println(testE0(e0.to_json()))
  let nested : Nested3 = { x: { x: { x: 42, y: 42 }, y: 42 }, y: 42 }
  println(testNested(nested.to_json()))
  let conErr = ConnectionError::BrokenPipe(42, "str")
  println(testErrEnum(conErr.to_json()))
  let t1 = { x: nested, y: eee, z: ee, w: [0], q: e, c: conErr }
  println(testT1(t1.to_json()))
  println(testNT(NT([t1]).to_json()))
  println("Err:")
  println(testT1(NT([t1]).to_json()))
  println(testNT(t1.to_json()))
  println(testE0(t1.to_json()))
  println(testS(t1.to_json()))
  println(testEEE(ee.to_json()))
  println(testE0(ee.to_json()))
  let nested : Nested2 = { x: { x: 42, y: 42 }, y: 42 }
  println(testNested(nested.to_json()))
  println(testErrEnum(ee.to_json()))
}

///|
struct StructWithOptionalFields {
  number : Int
  string : String
  optional_number : Int?
  nested_optional_number : Int??
  optional_string : String?
  nested_optional_string : String??
  number2 : Int
} derive(FromJson, Eq, Show)

///|
fn init {
  println("struct with optional fields:")
  //
  println("All fields present")
  let expected = Result::Ok({
    number: 42,
    string: "str",
    optional_number: Some(42),
    nested_optional_number: Some(Some(42)),
    optional_string: Some("str"),
    nested_optional_string: Some(Some("str")),
    number2: 42,
  })
  let input : Json = {
    "number": 42,
    "string": "str",
    "optional_number": 42,
    "nested_optional_number": [42],
    "optional_string": "str",
    "nested_optional_string": ["str"],
    "number2": 42,
  }
  let s : Result[StructWithOptionalFields, _] = try? @json.from_json(input)
  println("\{s == expected}, struct:\{s}")
  //
  println("Optional fields missing")
  let expected = Result::Ok({
    number: 42,
    string: "str",
    optional_number: None,
    nested_optional_number: None,
    optional_string: None,
    nested_optional_string: None,
    number2: 42,
  })
  let input : Json = { "number": 42, "string": "str", "number2": 42 }
  let s : Result[StructWithOptionalFields, _] = try? @json.from_json(input)
  println("\{s == expected}, struct:\{s}")
  //
  println("Some(None) for nested optional fields")
  let expected = Result::Ok({
    number: 42,
    string: "str",
    optional_number: Some(42),
    nested_optional_number: Some(None),
    optional_string: Some("str"),
    nested_optional_string: Some(None),
    number2: 42,
  })
  let input : Json = {
    "number": 42,
    "string": "str",
    "optional_number": 42,
    "nested_optional_number": Json::null(),
    "optional_string": "str",
    "nested_optional_string": Json::null(),
    "number2": 42,
  }
  let s : Result[StructWithOptionalFields, _] = try? @json.from_json(input)
  println("\{s == expected}, struct:\{s}")
}

// Field and variant renames

///|
struct FieldRename {
  a : Int
} derive(FromJson(fields(a(rename="b"))), Show)

///|
enum VariantRename {
  RenameMeToD1(Int)
  C2(String)
} derive(FromJson(cases(RenameMeToD1(rename="D1"))), Show)

///|
struct FieldRenameAllCamel {
  my_field : Int
} derive(FromJson(rename_fields="camelCase"), Show)

///|
struct FieldRenameAllScreamingSnake {
  my_field : Int
} derive(FromJson(rename_fields="SCREAMING_SNAKE_CASE"), Show)

///|
enum VariantRenameAllSnake {
  MyVariant(Int)
} derive(FromJson(rename_cases="snake_case"), Show)

///|
fn init {
  println("")
  println("Field and variant renames")
  let input : Json = { "b": 42 }
  let s : Result[FieldRename, _] = try? @json.from_json(input)
  println(s)
  let input : Json = ["D1", 42]
  let s : Result[VariantRename, _] = try? @json.from_json(input)
  println(s)
  let input : Json = { "myField": 42 }
  let s : Result[FieldRenameAllCamel, _] = try? @json.from_json(input)
  println(s)
  let input : Json = { "MY_FIELD": 42 }
  let s : Result[FieldRenameAllScreamingSnake, _] = try? @json.from_json(input)
  println(s)
  let input : Json = ["my_variant", 42]
  let s : Result[VariantRenameAllSnake, _] = try? @json.from_json(input)
  println(s)
}

// ==== The new enum styling options ====

///|
enum NewStyleLegacy {
  C1
  C2(String)
  C3(Int, Bool)
} derive(FromJson(style="legacy"), Show)

///|
enum NewStyleFlat {
  C1
  C2(String)
  C3(Int, Bool)
  C4(Int, x~ : Bool, Int, y~ : String)
} derive(FromJson(style="flat"), Show)

///|
fn init {
  println("")
  println("New enum style options")
  //
  println("Legacy style")
  let input : Json = { "$tag": "C1" }
  let s : Result[NewStyleLegacy, _] = try? @json.from_json(input)
  println(s)
  let input : Json = { "$tag": "C2", "0": "str" }
  let s : Result[NewStyleLegacy, _] = try? @json.from_json(input)
  println(s)
  let input : Json = { "$tag": "C3", "0": 42, "1": true }
  let s : Result[NewStyleLegacy, _] = try? @json.from_json(input)
  println(s)
  //
  println("Flat style")
  let input : Json = "C1"
  let s : Result[NewStyleFlat, _] = try? @json.from_json(input)
  println(s)
  let input : Json = ["C2", "str"]
  let s : Result[NewStyleFlat, _] = try? @json.from_json(input)
  println(s)
  let input : Json = ["C3", 42, true]
  let s : Result[NewStyleFlat, _] = try? @json.from_json(input)
  println(s)
  let input : Json = ["C4", 42, 100, { "x": true, "y": "str" }]
  let s : Result[NewStyleFlat, _] = try? @json.from_json(input)
  println(s)
}

///|
enum Shadow_Builtin {
  Some
  None
  Ok
  Err
  Null
}

// ==== Additional comprehensive tests ====

///|
struct TupleStruct(Int,String,Bool) derive(FromJson, ToJson, Show, Eq)

///|
struct EmptyStruct {} derive(FromJson, ToJson, Show, Eq)

///|
enum SingleVariantEnum {
  OnlyVariant(Int, String)
} derive(FromJson(style="flat"), ToJson(style="flat"), Show, Eq)

///|
enum MixedEnum {
  NoArgs
  OneArg(Int)
  TwoArgs(String, Bool)
  NamedArgs(x~ : Int, y~ : String)
  MixedArgs(Int, y~ : String, Bool)
} derive(FromJson(style="flat"), ToJson(style="flat"), Show, Eq)

///|
enum DeepNestedEnum {
  Level1(Level2)
} derive(FromJson(style="flat"), ToJson(style="flat"), Show, Eq)

///|
enum Level2 {
  Level2Variant(Level3)
} derive(FromJson(style="flat"), ToJson(style="flat"), Show, Eq)

///|
struct Level3 {
  value : Int
  nested : Level4?
} derive(FromJson, ToJson, Show, Eq)

///|
enum Level4 {
  FinalLevel(String)
} derive(FromJson(style="flat"), ToJson(style="flat"), Show, Eq)

///|
struct GenericStruct[T, U] {
  first : T
  second : U
  optional : T?
} derive(FromJson, ToJson, Show, Eq)

///|
enum GenericEnum[T] {
  Value(T)
  Pair(T, T)
} derive(FromJson(style="flat"), ToJson(style="flat"), Show, Eq)

///|
fn init {
  println("")
  println("=== Additional comprehensive tests ===")

  // Test tuple structs
  println("Tuple struct tests:")
  let ts = TupleStruct(42, "hello", true)
  let ts_json = ts.to_json()
  println("Original: \{ts}")
  println("JSON: \{ts_json.stringify()}")
  let ts_parsed : Result[TupleStruct, _] = try? @json.from_json(ts_json)
  println("Round-trip: \{ts_parsed}")
  println("Equal: \{ts_parsed == Result::Ok(ts)}")

  // Test empty struct
  println("Empty struct tests:")
  let es = EmptyStruct::{  }
  let es_json = es.to_json()
  println("Original: \{es}")
  println("JSON: \{es_json.stringify()}")
  let es_parsed : Result[EmptyStruct, _] = try? @json.from_json(es_json)
  println("Round-trip: \{es_parsed}")
  println("Equal: \{es_parsed == Result::Ok(es)}")
}
