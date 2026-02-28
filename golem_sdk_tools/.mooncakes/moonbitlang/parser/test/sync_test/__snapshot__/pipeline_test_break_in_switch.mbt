enum E {
  A
  B
  C(Int)
}

// 1. last case has payload
fn f1(x : E) -> Int {
  (for {
    match x {
      A => break 1
      B => break 2
      C(x) => break x
    }
  })
  // make the loop non-tail, so that js backend generates [break]
  |> Int::add(0)
}

// 2. last case no payload
fn f2(x : E) -> Int {
  (for {
    match x {
      C(x) => break x
      A => break 1
      B => break 2
    }
  })
  |> Int::add(0)
}

// 3. has default branch
fn f3(x : E) -> Int {
  (for {
    match x {
      A => break 1
      B => break 2
      _ => break 42
    }
  })
  |> Int::add(0)
}

fn init {
  println(([ f1(A), f1(B), f1(C(3)) ]: FixedArray[_]))
  println(([ f2(A), f2(B), f2(C(3)) ]: FixedArray[_]))
  println(([ f3(A), f3(B), f3(C(3)) ]: FixedArray[_]))
}
