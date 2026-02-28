fn f1(x: Array[Int]) -> Int {
  guard x is [a, b, .. _] && a == b else { -1 }
  a + b
}

fn f2(x: Json) -> Unit {
  guard x is { "key": String(s), .. } && s is [ 'a'..='z', .. _ ]
  println(s)
}

///|
fn main {
  println(f1([1, 2, 3]))
  println(f1([1, 1, 3]))
  f2({ "key": "hello" })
  f3("👍")
}

fn f3(x : String) -> Unit {
  // guard x is [ ', .. ]
  // x is emoji
  guard x is [ '👍', .. _ ] else {
    println("not an emoji")
  }
  println(x)
}


