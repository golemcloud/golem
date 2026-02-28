///|
fn[A] f(xs : ArrayView[A], ys : Array[Int]) -> @list.List[(Int, A)] {
  l1~: loop (0, xs, @list.empty()) {
    (_, [], acc) => acc
    (i, [x, .. rest], acc) =>
      for j in ys.iter() {
        if j == i {
          continue l1~ (i + 1, rest, @list.cons((j, x), acc))
        }
        if j + i == 7 {
          break l1~ acc
        }
      } else {
        continue (i - 1, rest, acc)
      }
  }
}

///|
fn[A] g(xs : ArrayView[A], ys : Array[Int]) -> @list.List[(Int, A)] {
  l1~: loop (0, xs, @list.empty()) {
    (_, [], acc) => acc
    (i, [x, .. rest], acc) =>
      for j in ys.iter() {
        if j == i {
          continue l1~ (i + 1, rest, @list.cons((j, x), acc))
        }
        if j + i == 7 {
          break acc
        }
      } else {
        continue (i - 1, rest, acc)
      }
  }
}

fn main {
  let r1 = f(["u", "v", "w", "x", "y", "z"][:], [0, 1, 2])
  println(r1)
  let r2 = f(["u", "v", "w", "x", "y", "z"][:], [0, 1, 2, 3, 4, 5])
  println(r2)
  let r3 = g(["u", "v", "w", "x", "y", "z"][:], [0, 1, 2])
  println(r3)
  let r4 = g(["u", "v", "w", "x", "y", "z"][:], [0, 1, 2, 3, 4, 5])
  println(r4)
}
