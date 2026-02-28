
///| docs for f1
/// some docs
/// @alert category3 "reason"
#alert(category1,"reason")
#alert(category2,"reason")
pub fn f1(x : Int) -> Int {
  5 + x
}

#alert(deprecated1,"reason")
#alert(unsafe1,"reason")
#alert(unsafe2,"reason")
pub fn f3(x : Int) -> Int {
  5 + x
}

#alert(category1,"reason")
#alert(category2,"reason")
pub extern "js" fn f4(x : Int) -> Int = "js"

///| docs for f5
/// some docs
/// @alert category3 "reason"
#alert(category1,"reason")
#alert(category2,"reason")
pub fn f5(x : Int) -> Int = "%primitive"

#alert(category1,"reason")
#alert(category2,"reason")
test "test" { ... }

#alert(category1,"reason")
#alert(category2,"reason")
pub enum MyEnum {}
 
///|
#alert(category1,"reason")
#alert(category2,"reason")
pub struct MyStruct { x : Int }

#alert(category1,"reason")
#alert(category2,"reason")
pub type MyType Int

#alert(category1,"reason")
#alert(category2,"reason")
pub type MyTypeAlias = Int

#alert(category1,"reason")
#alert(category2,"reason")
pub suberror MyErr Int

#alert(category1,"reason")
#alert(category2,"reason")
pub suberror MyErr2 {
  ErrCase1
  ErrCase2
}

#alert(category1,"reason")
#alert(category2,"reason")
 pub trait MyTrait {
   f1(Int) -> Int
 }

#coverage.skip("string")
#custom1.name(custom1.name, key=custom1.name2, "reason")
 pub impl MyTrait for MyStruct with f1(x) {
   ...
 }

trait I {
  #deprecated
  f(Self) -> Unit

  // should not report duplicate here
  #deprecated
  g(Self) -> Unit

  // should report duplicate here
  #deprecated
  #deprecated
  h(Self) -> Unit
}
