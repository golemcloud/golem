///|
fn init {
  let t = T(42)
  t.f(() => println(21))
  t.g((x) => println(x))
  t.g(x => println(x))
  t.g(x => x + 1)
  t.g((x: Int) => println(x))
  t.h((x, y) => println(x+y))
  t.h((x: Int, y) => println(x+y))
  t.h((x, y: Int) => println(x+y))
  t.h((x: Int, y: Int) => println(x+y))
}

// trailing comma
fn init {
  t.g((x,) => println(x))
  t.g((x: Int,) => println(x))
  t.h((x, y,) => println(x+y))
  t.h((x: Int, y,) => println(x+y))
  t.h((x, y: Int,) => println(x+y))
  t.h((x: Int, y: Int,) => println(x+y))
}

fn init {
  let t = T(42)
  t.f(_ => println(21))
  t.g((_) => println(x))
  t.g(_ => println(x))
  t.g(_ => x + 1)
  t.g((_: Int) => println(x))
  t.h((_, _) => println(x+y))
  t.h((_: Int, _) => println(x+y))
  t.h((_, _: Int) => println(x+y))
  t.h((_: Int, _: Int) => println(x+y))
}

// trailing comma
fn init {
  t.g((_,) => println(x))
  t.g((_: Int,) => println(x))
  t.h((_, _,) => println(x+y))
  t.h((_: Int, _,) => println(x+y))
  t.h((_, _: Int,) => println(x+y))
  t.h((_: Int, _: Int,) => println(x+y))
}

fn init {
  let t = _ => _ => 42
  let t = (_: Int) => (_: Int, _: Int) => 42
}