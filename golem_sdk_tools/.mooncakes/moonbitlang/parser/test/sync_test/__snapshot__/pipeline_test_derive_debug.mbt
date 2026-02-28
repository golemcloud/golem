///|
enum MyEnum[A] {
  C0
  C1(A, b~ : Int)
} derive(Debug)

///|
struct MyStruct[A] {
  x : A
  y : Int
} derive(Debug)

struct NestedDataStructure[A] {
  a : MyEnum[A]
  b : MyStruct[A]
} derive(Debug)

///|
suberror MySuberror {
  Ctor1(Int, label~ : Bool)
  Ctor2(Int, Bool)
  Ctor3
} derive(Debug)

///|
struct MyTupleStruct[A](A, Bool) derive(Debug)

///|
struct MyStructWithFunction {
  f1 : () -> Unit
  f2 : (Int) -> Unit
} derive(Debug)

///|
struct DeprecatedNewtype(Bool) derive(Debug)

///|
struct RecordWithTuples {
  tuple2 : (Int, Bool)
  tuple3 : (Int, Int, Bool)
  tuple4 : (Int, Int, Int, Bool)
} derive(Debug)

///|
fn main {
  debug(MyEnum::C0)
  debug(MyEnum::C1(false, b=10))
  debug(MyStruct::{ x: true, y: 11 })
  debug(NestedDataStructure::{
    a: MyEnum::C1(false, b=10),
    b: MyStruct::{ x: true, y: 11 }
  })
  debug(MySuberror::Ctor1(10, label=false))
  debug(MySuberror::Ctor2(11, true))
  debug(MySuberror::Ctor3)
  debug(MyTupleStruct((), true))
  debug(MyStructWithFunction::{ f1: () => println(1), f2: x => println(x) })
  debug(DeprecatedNewtype(false))
  debug(RecordWithTuples::{ tuple2: (1, false), tuple3: (1, 2, false), tuple4: (1, 2, 3, false) })
}
