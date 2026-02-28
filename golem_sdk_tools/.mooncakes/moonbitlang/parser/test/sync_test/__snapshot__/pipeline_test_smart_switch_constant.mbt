///|
fn main {
  let c : Char = 'a'
  match c {
    'A'..='Z' => println("UpperCase")
    'a'..='z' => println("LowerCase")
    '0'..='9' => println("Digit")
    _ => println("Special")
  }
  let a = 10
  match a {
    0..<10 => println("not ok")
    _..<_ => println("ok")
  }
  match a {
    _..<5 => println("not ok")
    _..<20 => println("ok")
    _ => println("not ok")
  }
  match a {
    20..<_ => println("not ok")
    10..<_ => println("ok")
    _ => println("not ok")
  }
  let b : Byte = 97
  match b {
    0..<50 => println("not ok")
    50..<100 => println("ok")
    100..<150 => println("not ok")
    _ => println("not ok")
  }
  let i : Int64 = 42
  match i {
    0 => println("not ok")
    _..<10 => println("not ok")
    10..<_ => println("ok")
  }
  let u : UInt = 200
  match u {
    0..=100 => println("not ok")
    100..<500 => println("ok")
    _..<1000 => println("not ok")
    _ => println("not ok")
  }
}
