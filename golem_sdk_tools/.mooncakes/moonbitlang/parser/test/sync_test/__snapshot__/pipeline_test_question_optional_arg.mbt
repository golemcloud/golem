fn f1(x? : Int, y? : Option[Int]) -> Unit {
  println("f1(\{x}, \{y})")
}

fn init {
  f1()
  f1(x=1, y=None)
  f1(x=1, y?=None)
  let x = 1
  let y = Some(42)
  f1(x~, y~)
  let x = Some(42)
  let y = Some(Some(43))
  f1(x?, y?)
}
