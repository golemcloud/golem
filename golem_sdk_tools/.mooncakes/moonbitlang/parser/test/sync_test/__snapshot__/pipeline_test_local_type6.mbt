//!build: -alert -deprecated -w -27
///|
traitalias @json.FromJson

///|
struct X0(Int) derive(Show)

///|
impl Default for X0 with default() {
  0
}

///|
test {
  let a = X0::default() // Type::default and Default::default both works
  println(a)
}

///|
fn[T : Show + ToJson + Default + @json.FromJson] id(_x : T) -> Unit {
  type U T derive(Show, ToJson, Default, FromJson)
  let v = U::default() // U[_'a]
  println(Show::to_string(v))
  println(ToJson::to_json(v))
  println(U::inner(v))
  let u = fn(x) raise { @json.from_json(x) }
  println(try? u([1]))
}

///|
fn[T : Show + ToJson + Default + @json.FromJson] id2(x : T) -> Unit {
  struct U(T) derive(Show, ToJson, Default, FromJson)
  let _ = U::default() // currently it is U[_]
  let _ : U = Default::default() // currently it is U[_]
  let _ = U::to_string

}

///|
fn main {
  id(1)
  id2(1)
}
