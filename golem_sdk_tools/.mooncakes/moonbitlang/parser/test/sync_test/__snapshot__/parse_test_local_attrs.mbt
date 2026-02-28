///|
enum MyEnum {
  #attr.tag1
  Bar1
  #attr.tag2
  #attr.tag3
  Bar2(Int,Bool)
}

///|
struct MyStruct {
  #attr.tag1
  field1 : Int
  #attr.tag2
  #attr.tag3
  field2 : Bool
}

///|
enum MyEnum2 {
  /// doc for bar1
  #attr.tag1
  Bar1
  /// doc for bar2
  #attr.tag2
  #attr.tag3
  Bar2(Int,Bool)
}

///|
struct MyStruct2 {
  /// doc for field1
  #attr.tag1
  field1 : Int
  /// doc for field2
  #attr.tag2
  #attr.tag3
  field2 : Bool
}

///|
trait MyTrait {
  #attr.tag1
  f() -> Self
  #attr.tag2
  #attr.tag3
  g(Self, Self) -> Unit
}

