
fn f1(map : @hashmap.HashMap[Int, Bool]) -> Int {
  match map {
    { 42: true, .. } => 1
    _ => 2
  }
}

fn init {
  let map = @hashmap.new()
  println(f1(map))
  map.set(42, true)
  println(f1(map))
}

fn f2(map : @hashmap.HashMap[Int, Bool?]) -> Int {
  match map {
    { 42: Some(true), .. } => 1
    { 42: Some(false), .. } => 2
    { 42: None, .. } => 3
    _ => 4
  }
}

fn init {
  let map = @hashmap.new()
  println(f2(map))
  map.set(42, None)
  println(f2(map))
  map.set(42, Some(true))
  println(f2(map))
  map.set(42, Some(false))
  println(f2(map))
}

fn f3(map : @hashmap.HashMap[String, Bool]) -> Int {
  match map {
    { "x": _, "y": _, .. } => 1
    { "x": _, .. } => 2
    _ => 3
  }
}

fn init {
  let map = @hashmap.new()
  println(f3(map))
  map.set("x", true)
  println(f3(map))
  map.set("y", true)
  println(f3(map))
}

fn f4(map : @hashmap.HashMap[String, Bool]) -> Int {
  match map {
    { "x": true, .. } => 1
    { "x": _, "y": _, .. } => 2
    _ => 3
  }
}

fn init {
  let map = @hashmap.new()
  map.set("x", true)
  map.set("y", true)
  println(f4(map))
  map.set("x", false)
  println(f4(map))
}
