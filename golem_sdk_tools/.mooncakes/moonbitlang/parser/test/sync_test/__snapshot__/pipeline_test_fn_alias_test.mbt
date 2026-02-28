///|
trait I {
  #as_free_fn(obj_meth)
  obj_meth(Self) -> Int
}

///|
type Obj = &I

///|
struct MyInt(Int)

///|
impl I for MyInt with obj_meth(s) {
  s.0 * 1000
}

///|
fn main {
  obj_meth(MyInt(1)) |> println
  let _ : (&I) -> Int = obj_meth
}
