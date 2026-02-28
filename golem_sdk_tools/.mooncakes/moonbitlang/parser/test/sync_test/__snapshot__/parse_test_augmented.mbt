struct T {
  mut x : Int
} derive(Show)

fn init {
  let mut a = 10
  a += 10
  println(a)
  a -= 5
  println(a)
  a *= 3
  println(a)
  a /= 5
  println(a)
  a %= 5
  println(a)
  let a = { x: 10 }
  a.x += 2 * 5
  println(a)
  a.x -= 2 + 3
  println(a)
  a.x *= 5 - 2
  println(a)
  a.x /= 5 % 9
  println(a)
  a.x %= 10 / 2
  println(a)
  let a = [10]
  a[0]
  +=
  ((2 * 5))
  println(a)
  a[0]
  -=
  ((2 + 3))
  println(a)
  a[0]
  *=
  ((5 - 2))
  println(a)
  a[0]
  /=
  ((5 % 9))
  println(a)
  a[0]
  %=
  ((10 / 2))
  println(a)
}
