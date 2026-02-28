///|
fn main {
  let val = Some("a")
  fn b() -> Ref[_] {
    { val, }
  }

  ignore(b)
}
