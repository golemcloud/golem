//!build: -w -a
///|
// test missing return type diagnostics in different syntax
trait T {
  f(Self) -> 
  g(Int) -> Self
}

fn f(x~ : Int = 5) -> = "module" "function"
fn g(x~ : Int = 5) -> {}
extern "js" fn h(x~ : Int = 5) -> = "code"

impl T1 for T2 with f(self,a,b) -> {}

///|
// test missing return type diagnostics before first set of impl.
// (without the token based segemnt)
      fn f1() -> 
      let a = 1
      fn f2() ->
      impl T1 for T2
      fn f3() ->
      fn f4() -> 
      test {}
      fn f5() ->
      typealias T1 as T2
      fn f6() ->
      async fn f7() ->
      const CONST = 5
      fn f8() ->
      trait AnotherTrait {}
      fn f9() ->
      struct MyRcd {}
      fn f10() ->
      enum MyEnum {}

///|
// test missing return type diagnostics before first set of impl.
// (with the token based segemnt)
fn f1() -> 
let a = 1
fn f2() ->
impl T1 for T2
fn f3() ->
fn f4() -> 
test {}
fn f5() ->
typealias T1 as T2
fn f6() ->
async fn f7() ->
const CONST = 5
fn f8() ->
trait AnotherTrait {}
fn f9() ->
struct MyRcd {}
fn f10() ->
enum MyEnum {}

///| 
// missing return type in local function
fn main {
  fn f() -> {
    
  }
  let g = fn() -> {
    
  }
}