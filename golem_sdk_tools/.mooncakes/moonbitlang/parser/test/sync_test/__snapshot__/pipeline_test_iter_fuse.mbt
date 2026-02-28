
pub fn[A] to_array(it: Iter[A]) -> Array[A] {
  let res = Array::new()
  it.each(x => res.push(x))
  res
}

fn init {
  [1, 2, 3, 4, 5, 6].iter().map(x => x + 1).take(4).map(x => x * x).take(2).each(x => println(x))
}

fn less_than_4(x: Int) -> Bool {
  x < 4
}

fn add_one(x: Int) -> Int {
  x + 1
}

fn init {
  let sum = [1, 2, 3, 4, 5, 6].iter().filter(less_than_4).map(add_one).map(x => x * x).fold((x, y) => x + y, init=0)
  let _sum = [1, 2, 3, 4, 5, 6].iter().filter(less_than_4).map(add_one).map(x => x * x).fold(init=0, (x, y) => x + y)
  println(sum)
}

fn init {
  Iter::repeat(10).take(5).each(x => println(x))
}

fn init {
  let v = Array::new()
  [1, 2, 3].iter().flat_map(x => Iter::repeat(x).take(2)).each(x => v.push(x))
  println(v)
}

fn init {
  let v= to_array([1, 2, 3].iter().flat_map(x => Iter::repeat(x).take(2)))
  println(v)
}

fn run(x: FixedArray[String]) -> String {
  x.iter().filter(s => s.length() >= 5).map(x => x + "!").take(3).fold(fn (x, y) { x + y }, init="")
}

fn init {
  println(run(["hello", "world", "this", "is", "a", "test", "of", "the", "emergency", "broadcast", "system"]))
}

fn init {
  println("== test concat ==")
  Iter::repeat(8).take(2).concat([4, 5, 6].iter().map(x => x + 1)).concat([1, 2, 3].iter().filter(x => x % 2 != 0)).each(x => println(x))
}

fn init {
  println("== test find ==")
  let res = [1,2,3].iter().concat([4, 5, 6].iter()).find_first(x => x % 2 == 0)
  println(res)
}

fn init {
  println("== test take(n) with n >= len ==")
  [1, 2, 3].iter().take(10).concat([4, 5, 6].iter().map(x => x + 1)).concat([1, 2, 3].iter().filter(x => x % 2 != 0)).each(x => println(x))
}

fn init {
  println("== find should run once ==")
  let t = [1, 2, 3].iter().take(1).concat([1, 1, 1].iter()).find_first(x => {println("should print only once"); x == 1 })
  println(t)
}

fn init {
  println("== test flat_map ==")
  let t = [1, 2, 3].iter().flat_map(x => Iter::repeat(x).take(x)).find_first(x => x > 1)
  println(t)
}
