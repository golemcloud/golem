///|
#valtype
enum E {
  A
  B(Int)
} derive(Show)

///|
#valtype
struct S {
  a : Int
  b : (Int, Int)
} derive(Show)

///|
fn g(xs : FixedArray[&Show]) -> Unit {
  println(xs)
  match xs {
    [a, .., b] => {
      println(a)
      println(b)
    }
    _ => println("other")
  }
}

///|
fn main {
  let e = E::B(1) as &Show
  let s = S::{ a: 1, b: (2, 3) } as &Show
  g([e, 42, s])
  g([(1, 2), s, e])
  let arr2 = []
  for i in 0..<10 {
    if i % 3 == 0 {
      arr2.push(e)
    } else if i % 3 == 1 {
      arr2.push(s)
    } else {
      arr2.push((i, i + 1))
    }
  }
  println(arr2)
  for i in 0..<5 {
    let _ = arr2.pop()

  }
  println(arr2)
  let arr = FixedArray::make(5, (1, 2) as &Show)
  arr[1] = e
  arr[2] = s
  println(arr)
}
