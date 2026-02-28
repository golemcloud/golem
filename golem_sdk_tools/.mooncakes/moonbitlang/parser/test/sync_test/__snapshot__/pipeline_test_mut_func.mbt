fn init {
  let mut f = fn (x: Int) { println(x) }
  let g = fn (x: Int) { println(x+1) }
  f( { f = g; 3} )
}

fn may_capture(a: Int, b: Int64) -> Int {
  let mut f = fn (x: Int) { x + a }
  let g = fn (x: Int) { x + 1 + b.to_int() }
  f( { f = g;3 } )
}

fn init {
  if may_capture(10, 20L) == 13 {
    println("ok")
  } else {
    abort("error")
  }
}
