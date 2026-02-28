///|
fn main {
  enum LocalEnum {
    A(Int)
    B(String)
  }
  fn f(x : LocalEnum) {
    match x {
      A(i) => println(i)
      LocalEnum::B(s) => println(s)
    }
  }

  f(A(42))
  f(B("hello"))
}
