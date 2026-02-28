struct MyBigInt(BigInt)
fn f(x : MyBigInt) -> Unit {
  match x.0 {
    0 => println("zero")
    _ => ()
  }
}

fn init {
  f(0)
}
