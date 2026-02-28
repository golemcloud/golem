///|
fn init {
  let x = -19
  let a = -x
  println(a)
  let c = -(-(-a))
  println(c)
  let d : Ref[Int] = { val: -2147483648 }
  f(d)
  println(d.val)
  let e : Ref[Int16] = { val: -32768 }
  g(e)
  println(e.val)
}

///|
#inline(never)
fn f(x : Ref[Int]) -> Unit {
  x.val = -x.val
}

///|
#inline(never)
fn g(x : Ref[Int16]) -> Unit {
  x.val = -x.val
}
