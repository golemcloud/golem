///|
#valtype
enum E {
  A
  B(Int)
  C(Double, Double)
} derive(Show)

///|
fn g(xs : FixedArray[E]) -> Unit {
  println(xs)
  match xs {
    [A, .., b] => { // unsafe_get and unsafe_get_rev
      println("case a")
      println(b)
    }
    [B(x), .., B(y)] => {
      println("case b")
      println(x)
      println(y)
    }
    [C(x, y), .., C(z, w)] => {
      println("case c")
      println(x)
      println(y)
      println(z)
      println(w)
    }
    _ => println("case _")
  }
}

///|
fn main {
  let e = E::A
  let e2 = E::B(1)
  let e3 = E::C(1.0, 2.0)
  g([e, e2])
  g([e2, e2])
  g([e3, e3])
  g([e, e2, e3])
  let arr2 = []
  for i in 0..<10 {
    if i % 3 == 0 {
      arr2.push(e)
    } else if i % 3 == 1 {
      arr2.push(E::B(i))
    } else {
      arr2.push(E::C(i.to_double(), i.to_double()))
    }
  }
  println(arr2)
  for i in 0..<5 {
    let _ = arr2.pop()

  }
  println(arr2)
}
