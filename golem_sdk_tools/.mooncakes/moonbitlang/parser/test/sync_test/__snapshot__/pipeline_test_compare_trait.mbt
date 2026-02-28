trait MyCompare {
  compare(Self, Self) -> Int
  lt(Self, Self) -> Bool = _
  le(Self, Self) -> Bool = _
  gt(Self, Self) -> Bool = _
  ge(Self, Self) -> Bool = _
}

impl MyCompare with lt(x, y) {
  x.compare(y).is_neg()
}

impl MyCompare with le(x, y) {
  x.compare(y).is_non_pos()
}

impl MyCompare with gt(x, y) {
  x.compare(y).is_pos()
}

impl MyCompare with ge(x, y) {
  x.compare(y).is_non_neg()
}

impl MyCompare for Int with compare(x, y) = "%i32_compare"
impl MyCompare for Int with lt(x, y) = "%i32.lt"
impl MyCompare for Int with le(x, y) = "%i32.le"
impl MyCompare for Int with gt(x, y) = "%i32.gt"
impl MyCompare for Int with ge(x, y) = "%i32.ge"

impl MyCompare for UInt with compare(x, y) = "%u32.compare"
impl MyCompare for UInt with lt(x, y) = "%u32.lt"
impl MyCompare for UInt with le(x, y) = "%u32.le"
impl MyCompare for UInt with gt(x, y) = "%u32.gt"
impl MyCompare for UInt with ge(x, y) = "%u32.ge"

impl MyCompare for Int64 with compare(x, y) {
  Int64::compare(x, y)
}

#cfg(not(target="js"))
impl MyCompare for Int64 with lt(x, y) = "%i64.lt"
#cfg(not(target="js"))
impl MyCompare for Int64 with le(x, y) = "%i64.le"
#cfg(not(target="js"))
impl MyCompare for Int64 with gt(x, y) = "%i64.gt"
#cfg(not(target="js"))
impl MyCompare for Int64 with ge(x, y) = "%i64.ge"

impl MyCompare for UInt64 with compare(x, y) {
  UInt64::compare(x, y)
}

#cfg(not(target="js"))
impl MyCompare for UInt64 with lt(x, y) = "%u64.lt"
#cfg(not(target="js"))
impl MyCompare for UInt64 with le(x, y) = "%u64.le"
#cfg(not(target="js"))
impl MyCompare for UInt64 with gt(x, y) = "%u64.gt"
#cfg(not(target="js"))
impl MyCompare for UInt64 with ge(x, y) = "%u64.ge"

impl MyCompare for Float with compare(x, y) = "%f32.compare"
impl MyCompare for Float with lt(x, y) = "%f32.lt"
impl MyCompare for Float with le(x, y) = "%f32.le"
impl MyCompare for Float with gt(x, y) = "%f32.gt"
impl MyCompare for Float with ge(x, y) = "%f32.ge"

impl MyCompare for Double with compare(x, y) = "%f64_compare"
impl MyCompare for Double with lt(x, y) = "%f64.lt"
impl MyCompare for Double with le(x, y) = "%f64.le"
impl MyCompare for Double with gt(x, y) = "%f64.gt"
impl MyCompare for Double with ge(x, y) = "%f64.ge"

struct CustomType (Int) derive(Show)

impl MyCompare for CustomType with compare(x, y) {
  MyCompare::compare(x.0, y.0)
}

fn[A : MyCompare + Show] test_compare(x : A, y : A) -> Unit {
  println("\{x}.compare(\{y}) => \{x.compare(y)}")
  println("\{x} < \{y} => \{x.lt(y)}")
  println("\{x} <= \{y} => \{x.le(y)}")
  println("\{x} > \{y} => \{x.gt(y)}")
  println("\{x} >= \{y} => \{x.ge(y)}")
}

fn main {
  println("==== Int ====")
  test_compare(1, 2)
  test_compare(1, 1)
  println("==== UInt ====")
  test_compare((1 : UInt), (2 : UInt))
  test_compare((1 : UInt), (1 : UInt))
  println("==== Int64 ====")
  test_compare((1 : Int64), (2 : Int64))
  test_compare((1 : Int64), (1: Int64))
  println("==== UInt64 ====")
  test_compare((1 : UInt64), (2 : UInt64))
  test_compare((1 : UInt64), (1: UInt64))
  println("==== Float ====")
  test_compare((1 : Float), (2 : Float))
  test_compare((1 : Float), (1: Float))
  println("==== Double ====")
  test_compare((1 : Double), (2 : Double))
  test_compare((1 : Double), (1: Double))
  println("==== custom type ====")
  test_compare(CustomType(1), CustomType(2))
  test_compare(CustomType(1), CustomType(1))
}
