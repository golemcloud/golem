enum T[M] {
  A (M)
}

let x:T[Int] = A(1)

let y:T[T[Int]] = A(x)

