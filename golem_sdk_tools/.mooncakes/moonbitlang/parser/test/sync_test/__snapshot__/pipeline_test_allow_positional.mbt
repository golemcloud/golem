//!build: -w -20

///|
#label_migration(a, allow_positional=true)
#label_migration(d, allow_positional=true)
pub fn allow_positional(a~ : Int, b : Int, c? : Int = a, d~ : Int) -> Unit {
  println("a=\{a}, b=\{b}, c=\{c}, d=\{d}")
}

///|
fn main {
  allow_positional(2, 3, 4)
  allow_positional(2, a=3, 4)
  allow_positional(2, a=3, d=4, c=1)
  allow_positional(a=3, d=4, c=1, 2)
}
