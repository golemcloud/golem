///|
fn error_to_json(e : Error) -> Json = "%error.to_json"

///|
suberror E1 {
  A(Int)
  B(String)
} derive(ToJson(style="legacy"))

///|
suberror E2 derive(ToJson(style="legacy"))

///|
suberror E3 {
  E3(Double?)
} derive(ToJson(style="legacy"))

///|
suberror E4 {
  C(Int)
  D(E3)
}

///|
fn f(x : Int) -> Unit raise Error {
  if x == 1 {
    raise A(x)
  } else if x == 2 {
    raise B("hello")
  } else if x == 3 {
    raise E2
  } else if x == 4 {
    raise E3(Some(42.0))
  } else if x == 5 {
    raise E3(None)
  } else if x == 6 {
    raise C(42)
  } else if x == 7 {
    raise D(E3(Some(42.0)))
  } else if x == 8 {
    raise D(E3(None))
  }
}

///|
fn main {
  for i in 1..=8 {
    try f(i) catch {
      e => println(error_to_json(e))
    }
  }
}
