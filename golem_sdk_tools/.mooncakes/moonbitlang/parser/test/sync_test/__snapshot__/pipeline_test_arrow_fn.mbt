///|
fn init {
  let t = T(42)
  t.f(() => println(21))
  t.g((x) => println(x))
  t.g((x: Int) => println(x))
  t.h((x, y) => println(x+y))
  t.h((x: Int, y) => println(x+y))
  t.h((x, y: Int) => println(x+y))
  t.h((x: Int, y: Int) => println(x+y))
}

///|
struct T(Int)

///|
fn T::f(self : T, cb : () -> Unit) -> Unit {
  ignore(self)
  cb()
}

///|
fn T::g(self : T, cb : (Int) -> Unit) -> Unit {
  cb(self.0)
}

///|
fn T::h(self : T, cb : (Int, Int) -> Unit) -> Unit {
  cb(self.0, self.0)
}

fn init {
  let f = _ => println(42)
  f(())  
}

fn init {
  let f = ( _ : Unit) => println(42)
  f(())
}
