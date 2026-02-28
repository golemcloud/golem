fn worker(x~ : Int, y~ : Int?) -> Unit {
  println(x + y.unwrap_or(0))
}

fn f(x : @hashmap.HashMap[String, Int]) -> Unit {
  match x {
    { "x": x, "y"? : y, .. } => worker(x~, y~)
    { "x"? : None, .. } => println("oops!")
  }
}

fn init {
  let map = @hashmap.new()
  f(map)
  map.set("x", 2)
  f(map)
  map.set("y", 3)
  f(map)
}
