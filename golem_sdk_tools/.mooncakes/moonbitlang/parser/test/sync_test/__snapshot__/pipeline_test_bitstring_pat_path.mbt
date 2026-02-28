///|
pub fn g(x : Bytes) -> Unit {
  match x {
    [u8be(0x40..=0x7F), ..] => println(1)
    [0x00, u8be(_), ..] => println(2)
    _ => println(3)
  }
}

///|
pub fn f(x : Bytes) -> Unit {
  match x {
    [u8be(0x01..=0xFF)] => println(1)
    [i8be(0x00)] => println(2)
    _ => println(3)
  }
}

///|
fn main {
  f([0])
  g([0, 0])
}
