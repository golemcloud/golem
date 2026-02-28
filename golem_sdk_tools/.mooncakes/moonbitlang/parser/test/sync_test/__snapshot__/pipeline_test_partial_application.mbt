///|
fn f(a : Int, b : Bool, c : String, d : Double) -> Int {
  println("f(\{a}, \{b}, \{c}, \{d})")
  a
}

///|
fn f_err(a : Int, b : Bool, c : String, d : Double) -> Int raise Error {
  println("f(\{a}, \{b}, \{c}, \{d})")
  if false {
    fail("")
  }
  a
}

///|
fn f_async(a : Int, b : Bool, c : String, d : Double) -> Int raise {
  println("f(\{a}, \{b}, \{c}, \{d})")
  if false {
    fail("")
  }
  a
}

///|
enum Foo {
  C(Int, Bool, String, Double)
} derive(Show)

///|
let obj : Foo = C(1, false, "hello", 3.14)

///|
fn Foo::g(self : Foo, b : Bool, c : String) -> Unit {
  println("obj.g(\{b}, \{c})")
}

///|
fn[A, B] hof(a : A, f : (A) -> B) -> B {
  f(a)
}

///|
fn[A, B] hof_err(a : A, f : (A) -> B raise Error) -> B raise Error {
  f(a)
}

///|
async fn[A, B] hof_async(a : A, f : async (A) -> B raise) -> B raise {
  f(a)
}

///|
fn main {
  println("==== apply ====")
  let _ = f(_, false, "hello", 3.14)(1)
  let _ = f(1, false, _, 3.14)("hello")
  let _ = f(_, false, _, 3.14)(1, "hello")

  //
  println("==== dot apply ====")
  let _ = obj.g(_, "hello")(true)
  let _ = obj.g(true, _)("hello")
  let _ = obj.g(_, _)(true, "hello")

  //
  println("==== constructor ====")
  C(1, _, "hello", 3.14)(false) |> println
  C(1, false, "hello", _)(3.14) |> println
  C(1, _, "hello", _)(false, 3.14) |> println

  //
  println("==== constructor with type name ====")
  Foo::C(1, _, "hello", 3.14)(false) |> println
  Foo::C(1, false, "hello", _)(3.14) |> println
  Foo::C(1, _, "hello", _)(false, 3.14) |> println

  //
  println("==== in pipe expression ====")
  let _ = 1 |> f(_, false, "hello", 3.14) // apply
  let _ = "hello" |> f(1, false, _, 3.14)
  // let _ = true |> obj.g(_, "hello") // dot apply, not supported yet
  // let _ = "hello" |> obj.g(true, _)
  false |> C(1, _, "hello", 3.14) |> println // constructor
  3.14 |> C(1, false, "hello", _) |> println
  false |> Foo::C(1, _, "hello", 3.14) |> println // constructor with type name
  3.14 |> Foo::C(1, false, "hello", _) |> println

  // used as higher-order function in check mode
  hof(1, f(_, false, "hello", 3.14)) |> ignore
  hof(false, obj.g(_, "hello")) |> ignore
  hof(1, C(_, false, "hello", 3.14)) |> println
  hof(false, Foo::C(1, _, "hello", 3.14)) |> println

  // used as higher-order function in check mode, for error and async
  (try? hof_err(1, f_err(_, false, "hello", 3.14))) |> ignore
  let _ = async fn() noraise {
    (try? hof_async(1, f_async(_, false, "hello", 3.14))) |> ignore
  }

  // dot application
  [ C(1, true, "s", 2.0) ].each(_.g(false, "x"))

  // dot application x pipe
  obj |> _.g(true, "y")
}
