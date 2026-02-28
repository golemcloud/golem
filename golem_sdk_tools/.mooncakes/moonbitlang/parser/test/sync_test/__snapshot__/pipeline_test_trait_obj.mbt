trait T {
  f(Self, Int) -> Int
  g(Self, String) -> String
}

struct S {
  x: Int
  s: String
}

impl T for S with f(self: S, a: Int) -> Int {
  self.x + a
}

impl T for S with g(self: S, a: String) -> String {
  self.s + a.to_string()
}

fn use_T(a: &T) -> Int {
  a.f(432) + a.g("object").length()
}

fn init {
  let s = S::{x: 234, s: "trait"} as &T
  println(use_T(s))
}

fn test_show(x: &Show) -> Unit {
  println(x.to_string())
}
fn init {
  test_show(42 as &Show)
  test_show("xxx" as &Show)
  test_show(3.14 as &Show)
}

fn test_array_show(x: Array[&Show]) -> Unit {
  x.each(println)
}
fn init {
  let shows = [42 as &Show, "xxx" as &Show, 3.14 as &Show]
  test_array_show(shows)
  let debugs = shows as &Show
  println(debugs)
}

fn init {
  let x = Option::Some(Option::Some(Option::Some(42))) as &Show
  test_show(x)
  generic_show(x)
}

fn[T: Show] generic_show(x: T) -> Unit {
  println(x)
}
