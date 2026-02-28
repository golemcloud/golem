///|
fn err_poly_tail_rec(count : Int, f : (Int) -> Unit raise?) -> Unit raise? {
  if count > 0 {
    f(count)
    err_poly_tail_rec(count - 1, f)
  }
}

///|
fn main {
  let mut x = 0
  err_poly_tail_rec(100000, _ => x += 1)
  println(x)

  let result = try? err_poly_tail_rec(1000000, x => {
    if x == 999999 {
      raise Failure::Failure("err")
    }
  })
  println(result)
}
