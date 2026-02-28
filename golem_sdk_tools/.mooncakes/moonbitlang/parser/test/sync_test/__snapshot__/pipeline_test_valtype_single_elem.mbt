///|
#valtype
struct R {
  p : Int
} derive(Show)

///|
fn m(r : R) -> Int {
  r.p
}

///|
fn n(r : R) -> R {
  { p: r.p + 1 }
}

///|
suberror E {
  E(R)
} derive(Show)

///|
fn o(i : Int) -> R raise E {
  if i < 0 {
    raise E({ p: i })
  } else {
    { p: i }
  }
}

///|
fn main {
  let r = R::{ p: 1 }
  println(r |> m)
  println(n(r))
  println(try? o(42))
  println(try? o(-1))
}
