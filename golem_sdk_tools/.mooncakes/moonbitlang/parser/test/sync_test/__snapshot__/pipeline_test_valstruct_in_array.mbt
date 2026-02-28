///|
#valtype
struct E {
  a : Int
  b : Int64
  c : Double
} derive(Show)

///|
#valtype
struct F {
  a : Double
  b : Int
  c : Int64
} derive(Show)

///|
fn f(e : E) -> String {
  "{\{e.a}, \{e.b}, \{e.c}}"
}

///|
fn g(e1 : E, e2 : E) -> Unit {
  h([e1, e2, e2, e2]) // EverySingleElem
}

///|
fn h(es : FixedArray[E]) -> Unit {
  match es {
    [a, .., b, c] => println("h: \{a}, \{b}, \{c}")
    _ => ()
  }
}

///|
fn k(e : E) -> Unit {
  let arr = []
  for i in 0..=10 {
    arr.push({ ..e, a: i })
  }
  for i in 0..=8 {
    let _ = arr.pop()

  }
  println("k: \{arr[0]}")
  println("k: \{arr.unsafe_get(1)}")
}

///|
fn main {
  let e1 : E = { a: 1, b: 2, c: 3.14 }
  let e2 : E = { a: 3, b: 4, c: 5.14 }
  let f1 : F = { a: 1.1, b: 2, c: 3 }
  println(f(e1))
  g(e1, e2)
  println(FixedArray::makei(5, i => { ..f1, b: i }))
  k(e2)

  println(Point2::{ x: 1, y: 2 })  
  println(RefPoint3::{ d2: Point2::{ x: 3, y: 4 }, z: 5 })

}


#valtype
struct Point2 {
  x : Int
  y : Int
} derive(Show)

struct RefPoint3 {
  d2 : Point2
  z : Int
} derive(Show)

