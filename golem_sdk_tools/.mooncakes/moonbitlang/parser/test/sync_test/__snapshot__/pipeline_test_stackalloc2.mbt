fn incr(x : Ref[Int]) -> Int {
  x.val = x.val + 1
  x.val
}

// escape'ed use in mutate
fn init {
  let x : Ref[_] = { val: 1 }
  let mut y = 2
  // `x` escapes here, so it cannot be unboxed
  y = incr(x)
}
