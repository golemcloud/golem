pub fn fib(n : Int) -> Int {
  if n == 0 || n == 1 {
    1
  } else {
    fib(n - 1) + fib(n - 2)
  }
}

pub(all) struct TestStruct {
  field_unit : Unit
  field_bool : Bool
  field_char : Char
  field_string : String
  field_int : Int
  field_uint : UInt
  field_float : Float
  field_double : Double
  field_int64 : Int64
  field_uint64 : UInt64
  field_bigint : BigInt
  field_bytes : Bytes
  field_stringbuilder : StringBuilder
  field_array : Array[TestStruct?]
  field_array_view : ArrayView[TestStruct]
  field_option_unit : Unit?
  field_option_bool : Bool?
  field_option_char : Char?
  field_option_string : String?
  field_option_int : Int?
  field_option_uint : UInt?
  field_option_float : Float?
  field_option_double : Double?
  field_option_int64 : Int64?
  field_option_uint64 : UInt64?
  field_option_bigint : BigInt?
  field_option_bytes : Bytes?
  field_tuple : (
    Int,
    String,
    Bool,
    Unit,
    Char,
    Float,
    Double,
    Int64,
    UInt64,
    BigInt,
    Bytes,
  )
}

pub fn test_struct(r : TestStruct) -> Unit {
  ignore(r)
}

pub fn test_args(
  arg_unit~ : Unit,
  arg_bool~ : Bool,
  arg_char~ : Char,
  arg_string~ : String,
  arg_int~ : Int,
  arg_uint~ : UInt,
  arg_float~ : Float,
  arg_double~ : Double,
  arg_int64~ : Int64,
  arg_uint64~ : UInt64,
  arg_bigint~ : BigInt,
  arg_bytes~ : Bytes,
  arg_stringbuilder~ : StringBuilder,
  arg_array~ : Array[TestStruct?],
  arg_array_view~ : ArrayView[TestStruct],
  arg_option_unit~ : Unit?,
  arg_option_bool~ : Bool?,
  arg_option_char~ : Char?,
  arg_option_string~ : String? = None,
  arg_option_int~ : Int?,
  arg_option_uint~ : UInt?,
  arg_option_float~ : Float?,
  arg_option_double~ : Double? = None,
  arg_option_int64~ : Int64?,
  arg_option_uint64~ : UInt64?,
  arg_option_bigint~ : BigInt?,
  arg_option_bytes~ : Bytes?,
  arg_tuple~ : (
    Int,
    String,
    Bool,
    Unit,
    Char,
    Float,
    Double,
    Int64,
    UInt64,
    BigInt,
    Bytes,
  )
) -> Unit {
  ignore(
    (
      arg_unit, arg_bool, arg_char, arg_string, arg_int, arg_uint, arg_float, arg_double,
      arg_int64, arg_uint64, arg_bigint, arg_bytes, arg_stringbuilder, arg_array,
      arg_array_view, arg_option_unit, arg_option_bool, arg_option_char, arg_option_string,
      arg_option_int, arg_option_uint, arg_option_float, arg_option_double, arg_option_int64,
      arg_option_uint64, arg_option_bigint, arg_option_bytes, arg_tuple,
    ),
  )
}

pub(all) enum TestEnum {
  A(Int)
  B(a~ : Int, mut b~ : Int?)
  C
}

pub fn test_enum() -> TestEnum raise {
  fail("not implemented")
}

pub fn[T] generic_fn(x : T) -> T {
  x
}

pub(open) trait Disposable {
  dispose(Self) -> Unit
}

struct DisposableImpl {
  f : () -> Unit
}

impl Disposable for DisposableImpl with dispose(self) {
  (self.f)()
}

pub fn newDisposable(f : () -> Unit) -> &Disposable {
  DisposableImpl::{ f, }
}

pub async fn testAsync(f : async () -> Int noraise, g : async () -> Int noraise) -> Int noraise {
  f() + g()
}
