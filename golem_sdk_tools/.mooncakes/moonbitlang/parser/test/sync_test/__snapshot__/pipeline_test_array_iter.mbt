//!build: -w -52
fn f(xs: FixedArray[FixedArray[String]]) -> Unit {
  for i = 0, len = xs.length(); i < len; i = i + 1 {
    xs[i].each(fn(s) {
      println(s)
    })
  }
}

fn init {
  f([["xx", "yy"], ["zz", "ww"]])
}
