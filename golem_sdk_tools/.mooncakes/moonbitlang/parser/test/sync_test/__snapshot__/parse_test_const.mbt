const A = 1
const B = 2
pub const C = "abc"

fn init {
  let _ = 1 + A
  match C {
    A => ()
    B => ()
    _ => ()
  }
}
