fn f1(x? : Int, y? : Int) -> Int {
}

fn init {
  f1()
  f1(x=1)
  f1(y=2)
  let x = 42
  f1(x~, y?=None)
  let y = Some(42)
  f1(y?)
}

fn f2(x? : Int = Some(0)) -> Unit {}

fn init {
  f(g?=x) // forward optional argument
}

trait I {
  f(label? : Int, Ty?) -> Unit
}
