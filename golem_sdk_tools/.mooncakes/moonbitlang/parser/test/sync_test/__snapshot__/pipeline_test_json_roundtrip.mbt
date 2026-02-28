//!build: -w -27
///|
// Comprehensive JSON roundtrip tests focusing on option types and complex data structures
// This test ensures that JSON serialization followed by deserialization produces identical values

// Basic types for roundtrip testing
struct BasicStruct {
  int_field : Int
  string_field : String
  bool_field : Bool
  double_field : Double
} derive(ToJson, FromJson, Eq, Show)

///|
// Option types at various nesting levels
struct OptionStruct {
  basic_option : Int?
  nested_option : Int??
  triple_option : Int???
  string_option : String?
  nested_string_option : String??
} derive(ToJson, FromJson, Eq, Show)

///|
// Enums with option payloads
enum OptionEnum {
  NoPayload
  SimplePayload(Int)
  OptionPayload(Int?)
  NestedOptionPayload(String??)
  MixedPayload(Int, String?, Bool)
} derive(ToJson(style="flat"), FromJson(style="flat"), Eq, Show)

///|
// Tuple structs for edge case testing
struct TupleStruct (Int, String, Bool) derive(ToJson, FromJson, Eq, Show)

///|
struct NestedTupleStruct (Int, TupleStruct, String?) derive(ToJson, FromJson, Eq, Show)

///|
struct EmptyStruct {} derive(ToJson, FromJson, Eq, Show)

///|
// Single variant enum - edge case
enum SingleVariantEnum {
  OnlyVariant(Int, String)
} derive(ToJson(style="flat"), FromJson(style="flat"), Eq, Show)

///|
// Enums with named fields
enum NamedFieldEnum {
  NoArgs
  NamedArgs(x~ : Int, y~ : String, z~ : Bool?)
  MixedArgs(Int, named~ : String?, Bool)
  OnlyNamed(a~ : Double, b~ : String, c~ : Int?)
} derive(ToJson(style="flat"), FromJson(style="flat"), Eq, Show)

///|
// Enum with different tagging styles
enum InternallyTaggedEnum {
  Variant1(Int)
  Variant2(String, Bool?)
} derive(ToJson(repr(tag="type")), FromJson(repr(tag="type")), Eq, Show)

///|
enum ExternallyTaggedEnum {
  ExtVariant1(Int)
  ExtVariant2(String, Bool)
} derive(ToJson(repr(ext_tagged)), FromJson(repr(ext_tagged)), Eq, Show)

///|
enum AdjacentlyTaggedEnum {
  AdjVariant1(Int)
  AdjVariant2(String, Bool)
} derive(ToJson(repr(tag="tag", contents="content")), FromJson(repr(tag="tag", contents="content")), Eq, Show)

///|
// Deeply nested structures
struct Level1 {
  value : Int
  level2 : Level2?
} derive(ToJson, FromJson, Eq, Show)

///|
struct Level2 {
  name : String
  level3 : Level3?
} derive(ToJson, FromJson, Eq, Show)

///|
struct Level3 {
  nested_enum : DeepEnum
  options : Array[Int?]
} derive(ToJson, FromJson, Eq, Show)

///|
enum DeepEnum {
  DeepVariant1
  DeepVariant2(Level1)
  DeepVariant3(x~ : Int, nested~ : Level2?)
} derive(ToJson(style="flat"), FromJson(style="flat"), Eq, Show)

///|
// Generic types for complex testing
struct GenericStruct[T, U] {
  first : T
  second : U
  optional_first : T?
  optional_second : U?
} derive(ToJson, FromJson, Eq, Show)

///|
enum GenericEnum[T] {
  Single(T)
  Pair(T, T)
  OptionalPair(T?, T?)
  Named(value~ : T, backup~ : T?)
} derive(ToJson(style="flat"), FromJson(style="flat"), Eq, Show)

///|
// Nested generic structures
struct NestedGeneric[T] {
  container : GenericStruct[T, String]
  enum_value : GenericEnum[T]
  nested_array : Array[GenericEnum[T?]]
} derive(ToJson, FromJson, Eq, Show)

///|
// Collections with complex types
struct ComplexCollections {
  tuple_array : Array[(Int, String?)]
  nested_options : Array[Array[Int?]?]
  enum_map : Array[(String, OptionEnum)]
  mixed_tuple : (OptionEnum, TupleStruct?, Int)
} derive(ToJson, FromJson, Eq, Show)

///|
// Edge case enums
enum EdgeCaseEnum {
  Empty
  SingleInt(Int)
  SingleOptional(Int?)
  DoubleNested(Int??)
  TripleNested(String???)
  ComplexNested(x~ : Int?, y~ : String??, z~ : Bool)
} derive(ToJson(style="flat"), FromJson(style="flat"), Eq, Show)

///|
// Helper function to test roundtrip with detailed output
fn[T : ToJson + FromJson + Eq + Show] test_roundtrip(value : T, test_name : String) -> Unit {
  let json = value.to_json()
  let result : Result[T, @json.JsonDecodeError] = try? @json.from_json(json)
  match result {
    Ok(decoded) => {
      if decoded == value {
        println("PASSED: " + test_name)
      } else {
        println("FAILED: " + test_name)
        println("MISMATCH in " + test_name + ":")
        println("  Original value: " + value.to_string())
        println("  Serialized JSON: " + json.to_string())
        let decoded_json = decoded.to_json()
        println("  Deserialized value: " + decoded.to_string())
        println("  Re-serialized JSON: " + decoded_json.to_string())
        println("")
      }
    }
    Err(error) => {
      println("FAILED: " + test_name)
      println("ERROR in " + test_name + ":")
      println("  Original value: " + value.to_string())
      println("  Serialized JSON: " + json.to_string())
      println("  Decode error: " + error.to_string())
      println("")
    }
  }
}

///|
fn init {
  println("=== JSON Roundtrip Tests ===")
  
  // Test basic structures
  let basic = BasicStruct::{
    int_field: 42,
    string_field: "hello",
    bool_field: true,
    double_field: 3.14
  }
  test_roundtrip(basic, "BasicStruct")
  
  // Test None values
  let opts_none = OptionStruct::{
    basic_option: None,
    nested_option: None,
    triple_option: None,
    string_option: None,
    nested_string_option: None
  }
  test_roundtrip(opts_none, "OptionStruct all None")
  
  // Test Some values
  let opts_some = OptionStruct::{
    basic_option: Some(42),
    nested_option: Some(Some(84)),
    triple_option: Some(Some(Some(126))),
    string_option: Some("test"),
    nested_string_option: Some(Some("nested"))
  }
  test_roundtrip(opts_some, "OptionStruct all Some")
  
  // Test mixed Some/None
  let opts_mixed = OptionStruct::{
    basic_option: Some(99),
    nested_option: None,
    triple_option: Some(None),
    string_option: None,
    nested_string_option: Some(None)
  }
  test_roundtrip(opts_mixed, "OptionStruct mixed Some/None")
  
  // Test Some(Some(None)) - important edge case
  let opts_some_none = OptionStruct::{
    basic_option: Some(1),
    nested_option: Some(None),
    triple_option: Some(Some(None)),
    string_option: Some("value"),
    nested_string_option: Some(Some("deep"))
  }
  test_roundtrip(opts_some_none, "OptionStruct Some(None) patterns")
  
  // Test enum with options
  test_roundtrip(OptionEnum::NoPayload, "OptionEnum::NoPayload")
  test_roundtrip(OptionEnum::SimplePayload(42), "OptionEnum::SimplePayload")
  test_roundtrip(OptionEnum::OptionPayload(Some(42)), "OptionEnum::OptionPayload(Some)")
  test_roundtrip(OptionEnum::OptionPayload(None), "OptionEnum::OptionPayload(None)")
  test_roundtrip(OptionEnum::NestedOptionPayload(Some(Some("nested"))), "OptionEnum::NestedOptionPayload(Some(Some))")
  test_roundtrip(OptionEnum::NestedOptionPayload(Some(None)), "OptionEnum::NestedOptionPayload(Some(None))")
  test_roundtrip(OptionEnum::NestedOptionPayload(None), "OptionEnum::NestedOptionPayload(None)")
  test_roundtrip(OptionEnum::MixedPayload(10, Some("mixed"), true), "OptionEnum::MixedPayload with Some")
  test_roundtrip(OptionEnum::MixedPayload(20, None, false), "OptionEnum::MixedPayload with None")
  
  // Test tuple structs
  test_roundtrip(TupleStruct(42, "hello", true), "TupleStruct")
  test_roundtrip(TupleStruct(-1, "", false), "TupleStruct edge case")
  test_roundtrip(NestedTupleStruct(1, TupleStruct(2, "nested", true), Some("opt")), "NestedTupleStruct with Some")
  test_roundtrip(NestedTupleStruct(3, TupleStruct(4, "test", false), None), "NestedTupleStruct with None")
  
  // Test empty struct
  test_roundtrip(EmptyStruct::{}, "EmptyStruct")
  
  // Test single variant enum
  test_roundtrip(SingleVariantEnum::OnlyVariant(99, "single"), "SingleVariantEnum")
  
  // Test enums with named fields
  test_roundtrip(NamedFieldEnum::NoArgs, "NamedFieldEnum::NoArgs")
  test_roundtrip(NamedFieldEnum::NamedArgs(x=10, y="named", z=Some(true)), "NamedFieldEnum::NamedArgs with Some")
  test_roundtrip(NamedFieldEnum::NamedArgs(x=20, y="test", z=None), "NamedFieldEnum::NamedArgs with None")
  test_roundtrip(NamedFieldEnum::MixedArgs(30, named=Some("mixed"), false), "NamedFieldEnum::MixedArgs with Some")
  test_roundtrip(NamedFieldEnum::MixedArgs(40, named=None, true), "NamedFieldEnum::MixedArgs with None")
  test_roundtrip(NamedFieldEnum::OnlyNamed(a=3.14, b="only", c=Some(42)), "NamedFieldEnum::OnlyNamed with Some")
  test_roundtrip(NamedFieldEnum::OnlyNamed(a=2.71, b="named", c=None), "NamedFieldEnum::OnlyNamed with None")
  
  // Test different tagging styles
  test_roundtrip(InternallyTaggedEnum::Variant1(100), "InternallyTaggedEnum::Variant1")
  test_roundtrip(InternallyTaggedEnum::Variant2("internal", Some(true)), "InternallyTaggedEnum::Variant2 with Some")
  test_roundtrip(InternallyTaggedEnum::Variant2("test", None), "InternallyTaggedEnum::Variant2 with None")
  
  test_roundtrip(ExternallyTaggedEnum::ExtVariant1(200), "ExternallyTaggedEnum::ExtVariant1")
  test_roundtrip(ExternallyTaggedEnum::ExtVariant2("external", true), "ExternallyTaggedEnum::ExtVariant2")
  
  test_roundtrip(AdjacentlyTaggedEnum::AdjVariant1(300), "AdjacentlyTaggedEnum::AdjVariant1")
  test_roundtrip(AdjacentlyTaggedEnum::AdjVariant2("adjacent", false), "AdjacentlyTaggedEnum::AdjVariant2")
  
  // Test deep nesting
  let deep_level3 = Level3::{
    nested_enum: DeepEnum::DeepVariant1,
    options: [Some(1), None, Some(2)]
  }
  let deep_level2 = Level2::{
    name: "level2",
    level3: Some(deep_level3)
  }
  let deep_level1 = Level1::{
    value: 42,
    level2: Some(deep_level2)
  }
  test_roundtrip(deep_level1, "Deep nesting Level1")
  
  let deep_with_enum = Level3::{
    nested_enum: DeepEnum::DeepVariant2(Level1::{ value: 99, level2: None }),
    options: [None, None]
  }
  test_roundtrip(deep_with_enum, "Deep nesting with enum containing struct")
  
  let deep_named_enum = DeepEnum::DeepVariant3(x=123, nested=Some(Level2::{ name: "deep", level3: None }))
  test_roundtrip(deep_named_enum, "Deep enum with named fields")
  
  // Test generic types
  let generic_struct = GenericStruct::{
    first: 42,
    second: "generic",
    optional_first: Some(84),
    optional_second: None
  }
  test_roundtrip(generic_struct, "GenericStruct[Int, String]")
  
  let generic_enum = GenericEnum::Single(42)
  test_roundtrip(generic_enum, "GenericEnum::Single")
  test_roundtrip(GenericEnum::Pair("a", "b"), "GenericEnum::Pair")
  test_roundtrip(GenericEnum::OptionalPair(Some(1), None), "GenericEnum::OptionalPair mixed")
  test_roundtrip(GenericEnum::Named(value=100, backup=Some(200)), "GenericEnum::Named with backup")
  test_roundtrip(GenericEnum::Named(value="test", backup=None), "GenericEnum::Named without backup")
  
  // Test nested generics
  let nested_generic = NestedGeneric::{
    container: GenericStruct::{
      first: 1,
      second: "nested",
      optional_first: None,
      optional_second: Some("opt")
    },
    enum_value: GenericEnum::Pair(2, 3),
    nested_array: [
      GenericEnum::Single(Some(4)),
      GenericEnum::OptionalPair(None, Some(Some(5)))
    ]
  }
  test_roundtrip(nested_generic, "NestedGeneric complex")
  
  // Test complex collections
  let complex_collections = ComplexCollections::{
    tuple_array: [(1, Some("a")), (2, None), (3, Some("c"))],
    nested_options: [Some([Some(1), None]), None, Some([])],
    enum_map: [
      ("first", OptionEnum::NoPayload),
      ("second", OptionEnum::OptionPayload(Some(42))),
      ("third", OptionEnum::MixedPayload(10, None, true))
    ],
    mixed_tuple: (OptionEnum::SimplePayload(99), Some(TupleStruct(0, "tuple", false)), 123)
  }
  test_roundtrip(complex_collections, "ComplexCollections")
  
  // Test edge case enums
  test_roundtrip(EdgeCaseEnum::Empty, "EdgeCaseEnum::Empty")
  test_roundtrip(EdgeCaseEnum::SingleInt(42), "EdgeCaseEnum::SingleInt")
  test_roundtrip(EdgeCaseEnum::SingleOptional(Some(42)), "EdgeCaseEnum::SingleOptional Some")
  test_roundtrip(EdgeCaseEnum::SingleOptional(None), "EdgeCaseEnum::SingleOptional None")
  test_roundtrip(EdgeCaseEnum::DoubleNested(Some(Some(42))), "EdgeCaseEnum::DoubleNested Some(Some)")
  test_roundtrip(EdgeCaseEnum::DoubleNested(Some(None)), "EdgeCaseEnum::DoubleNested Some(None)")
  test_roundtrip(EdgeCaseEnum::DoubleNested(None), "EdgeCaseEnum::DoubleNested None")
  test_roundtrip(EdgeCaseEnum::TripleNested(Some(Some(Some("deep")))), "EdgeCaseEnum::TripleNested deep Some")
  test_roundtrip(EdgeCaseEnum::TripleNested(Some(Some(None))), "EdgeCaseEnum::TripleNested Some(Some(None))")
  test_roundtrip(EdgeCaseEnum::TripleNested(Some(None)), "EdgeCaseEnum::TripleNested Some(None)")
  test_roundtrip(EdgeCaseEnum::TripleNested(None), "EdgeCaseEnum::TripleNested None")
  test_roundtrip(EdgeCaseEnum::ComplexNested(x=Some(1), y=Some(Some("complex")), z=true), "EdgeCaseEnum::ComplexNested all Some")
  test_roundtrip(EdgeCaseEnum::ComplexNested(x=None, y=Some(None), z=false), "EdgeCaseEnum::ComplexNested mixed None")
  test_roundtrip(EdgeCaseEnum::ComplexNested(x=Some(2), y=None, z=true), "EdgeCaseEnum::ComplexNested partial None")
  
  // Test edge cases
  let empty_opts : Array[Int?] = []
  test_roundtrip(empty_opts, "Empty Array of Options")
  
  let all_none_array : Array[String?] = [None, None, None]
  test_roundtrip(all_none_array, "Array of all None options")
  
  let deeply_nested_options : Array[Array[String?]?] = [
    Some([Some("a"), None]),
    None,
    Some([])
  ]
  test_roundtrip(deeply_nested_options, "Deeply nested option arrays")
  
  // Test extreme nesting levels
  let extreme_option : String?????? = Some(Some(Some(Some(Some(Some("extreme"))))))
  test_roundtrip(extreme_option, "Extremely nested option")
  
  let extreme_none : Int?????? = None
  test_roundtrip(extreme_none, "Extremely nested None")
  
  let extreme_mixed : Bool?????? = Some(Some(None))
  test_roundtrip(extreme_mixed, "Extremely nested mixed")
  
  println("JSON roundtrip tests completed")
}
