///|
#valtype
struct S {
  a : Int
  b : Int64?
  c : Int64?
  d : Bool
} derive(Show)

///|
fn g(xs : FixedArray[S]) -> Unit {
  println(xs)
  match xs {
    [a, .., b] => { // unsafe_get and unsafe_get_rev
      println(a)
      println(b)
    }
    _ => ()
  }
}

///|
fn main {
  let s1 = S::{ a: 1, b: Some(2), c: Some(3), d: true }
  let s2 = S::{ a: 4, b: None, c: Some(5), d: false }
  let arr : FixedArray[S] = [s1, s2, s2]
  g(arr)
  let arr2 = []
  for i in 0..<10 {
    arr2.push(S::{ a: i, b: None, c: Some(i.to_int64()), d: false })
  }
  for i in 0..<5 {
    let _ = arr2.pop()

  }
  println(arr2)
}
