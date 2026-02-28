fn f(x: Int) -> Iter[Int] {
  [x, x].iter()
}

fn main {
  [1, 2, 3].iter().flat_map(f).each(x => println(x))
}
