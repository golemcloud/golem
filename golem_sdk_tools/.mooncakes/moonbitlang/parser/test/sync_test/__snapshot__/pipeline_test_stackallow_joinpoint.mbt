///|
pub fn f(bytes : Bytes) -> Int {
  let mut tlen = 0
  loop bytes[:] {
    [0x00..=0x7F, 0x00..=0x7F, .. bytes] => {
      tlen += 2
      continue bytes
    }
    [0x00..=0x7F, .. bytes] => {
      tlen += 1
      continue bytes
    }
    _ => return tlen
  }
}

///|
fn main {
  println(f([0x00, 0x00, 0x00, 0x00]))
}
