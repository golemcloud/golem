//!build: -w -27

fn add(x : Int, y : Int) -> Int {
  x + y
}

fnalias add as plus
fnalias plus as plus2

struct Counter {
  mut value : Int
}

fn Counter::bump(self : Counter, step? : Int = 1) -> Int {
  self.value += step
  self.value
}

fnalias Counter::bump as bump_counter

fn init {
  let c = Counter::{ value: 0 }
  let _ = plus2(1, 2)
  let _ = bump_counter(c, step=3)
  ()
}
