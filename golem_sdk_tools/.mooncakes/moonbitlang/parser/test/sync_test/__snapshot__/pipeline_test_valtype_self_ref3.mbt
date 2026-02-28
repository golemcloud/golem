///|
#valtype
pub(all) struct R {
  p : (R, R)
  i : Int
}

///|
pub fn m(r : R) -> R {
  r.p.0
}
